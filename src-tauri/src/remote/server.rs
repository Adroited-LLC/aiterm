use super::auth::{set_private_permissions, write_private_file, DeviceStore, PairingOutcome};
use super::model::{RemoteEvent, RemoteRequest, TerminalSize, PROTOCOL_VERSION};
use super::terminal::{
    chunk_diff_for_attachment, chunk_scrollback_for_attachment, chunk_snapshot_for_attachment,
    RemoteTerminal, RemoteTerminalEvents, TerminalEvent, TransferChunk, MAX_WIRE_FRAME_BYTES,
};
use crate::tabs::{AttachmentId, TabId, TabLaunch, TabRegistry};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::routing::get;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::StreamExt;
use rand_core::{OsRng, RngCore};
use rcgen::PublicKeyData;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const CERT_FILE: &str = "gateway-cert.der";
const KEY_FILE: &str = "gateway-key.der";
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUESTS_PER_SECOND: f64 = 120.0;
const OUTBOUND_EVENT_QUEUE: usize = 64;

#[derive(Clone)]
pub struct TlsIdentity {
    certificate_der: Vec<u8>,
    private_key_der: Vec<u8>,
    spki_fingerprint: String,
}

impl TlsIdentity {
    pub fn load_or_create(
        root: impl AsRef<Path>,
        subject_alt_ips: &[IpAddr],
    ) -> Result<Self, GatewayError> {
        let root = root.as_ref();
        std::fs::create_dir_all(root).map_err(GatewayError::io)?;
        set_private_permissions(root, 0o700).map_err(GatewayError::io)?;
        let cert_path = root.join(CERT_FILE);
        let key_path = root.join(KEY_FILE);
        match (cert_path.exists(), key_path.exists()) {
            (true, true) => {
                let certificate_der = std::fs::read(cert_path).map_err(GatewayError::io)?;
                let private_key_der = std::fs::read(key_path).map_err(GatewayError::io)?;
                Self::from_parts(certificate_der, private_key_der)
            }
            (false, false) => Self::generate(root, subject_alt_ips),
            _ => Err(GatewayError::new(
                "gateway.incomplete_identity",
                "gateway certificate and private key must both exist",
            )),
        }
    }

    fn generate(root: &Path, subject_alt_ips: &[IpAddr]) -> Result<Self, GatewayError> {
        let mut names = vec!["localhost".to_string()];
        names.extend(subject_alt_ips.iter().map(ToString::to_string));
        names.sort();
        names.dedup();
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(names).map_err(GatewayError::tls)?;
        let certificate_der = cert.der().to_vec();
        let private_key_der = signing_key.serialize_der();
        write_private_file(&root.join(CERT_FILE), &certificate_der).map_err(GatewayError::io)?;
        write_private_file(&root.join(KEY_FILE), &private_key_der).map_err(GatewayError::io)?;
        Self::from_parts(certificate_der, private_key_der)
    }

    fn from_parts(
        certificate_der: Vec<u8>,
        private_key_der: Vec<u8>,
    ) -> Result<Self, GatewayError> {
        let key_pair =
            rcgen::KeyPair::try_from(private_key_der.as_slice()).map_err(GatewayError::tls)?;
        let spki_fingerprint =
            URL_SAFE_NO_PAD.encode(Sha256::digest(key_pair.subject_public_key_info()));
        // Building the config validates both DER values and that the key
        // actually belongs to the certificate before the identity is exposed.
        build_server_config(&certificate_der, &private_key_der)?;
        Ok(Self {
            certificate_der,
            private_key_der,
            spki_fingerprint,
        })
    }

    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    pub fn spki_fingerprint(&self) -> &str {
        &self.spki_fingerprint
    }

    fn rustls_config(&self) -> Result<RustlsConfig, GatewayError> {
        Ok(RustlsConfig::from_config(Arc::new(build_server_config(
            &self.certificate_der,
            &self.private_key_der,
        )?)))
    }
}

fn build_server_config(
    certificate_der: &[u8],
    private_key_der: &[u8],
) -> Result<rustls::ServerConfig, GatewayError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(GatewayError::tls)?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate_der.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec())),
        )
        .map_err(GatewayError::tls)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

#[derive(Clone)]
struct GatewayState {
    devices: Arc<DeviceStore>,
    services: RemoteServices,
}

#[derive(Clone)]
pub struct RemoteServices {
    registry: Arc<TabRegistry>,
    terminal: RemoteTerminal,
}

impl RemoteServices {
    pub fn new(registry: Arc<TabRegistry>) -> Self {
        Self {
            terminal: RemoteTerminal::new(registry.clone()),
            registry,
        }
    }

    pub fn registry(&self) -> &Arc<TabRegistry> {
        &self.registry
    }
}

pub struct RemoteGateway;

impl RemoteGateway {
    pub async fn start(
        bind: SocketAddr,
        devices: Arc<DeviceStore>,
        identity: TlsIdentity,
        services: RemoteServices,
    ) -> Result<GatewayHandle, GatewayError> {
        let listener = std::net::TcpListener::bind(bind).map_err(GatewayError::io)?;
        listener.set_nonblocking(true).map_err(GatewayError::io)?;
        let local_addr = listener.local_addr().map_err(GatewayError::io)?;
        let certificate_der = identity.certificate_der.clone();
        let spki_fingerprint = identity.spki_fingerprint.clone();
        let tls = identity.rustls_config()?;
        let state = GatewayState { devices, services };
        let router = Router::new()
            .route("/v1/ws", get(websocket_upgrade))
            .with_state(state);
        let server_handle = axum_server::Handle::new();
        let run_handle = server_handle.clone();
        let server = axum_server::from_tcp_rustls(listener, tls)
            .map_err(GatewayError::io)?
            .handle(run_handle)
            .serve(router.into_make_service());
        let task = tokio::spawn(server);
        Ok(GatewayHandle {
            local_addr,
            certificate_der,
            spki_fingerprint,
            server_handle,
            task: Some(task),
        })
    }
}

pub struct GatewayHandle {
    local_addr: SocketAddr,
    certificate_der: Vec<u8>,
    spki_fingerprint: String,
    server_handle: axum_server::Handle<SocketAddr>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl GatewayHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    pub fn spki_fingerprint(&self) -> &str {
        &self.spki_fingerprint
    }

    pub async fn stop(mut self) -> Result<(), GatewayError> {
        self.server_handle.shutdown();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        task.await
            .map_err(|error| GatewayError::new("gateway.task_failed", error.to_string()))?
            .map_err(GatewayError::io)
    }
}

impl Drop for GatewayHandle {
    fn drop(&mut self) {
        self.server_handle.shutdown();
    }
}

async fn websocket_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<GatewayState>,
) -> impl axum::response::IntoResponse {
    ws.max_message_size(MAX_MESSAGE_SIZE)
        .max_frame_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| authenticate_socket(socket, state))
}

#[derive(Serialize)]
struct AuthChallenge {
    kind: &'static str,
    nonce: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthProof {
    kind: String,
    device_id: String,
    signature_der: Vec<u8>,
}

#[derive(Deserialize)]
struct ClientFrameKind {
    kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairRequest {
    kind: String,
    enrollment_secret: Vec<u8>,
    device_name: String,
    public_key: Vec<u8>,
}

#[derive(Serialize)]
struct AuthReply {
    kind: &'static str,
}

#[derive(Serialize)]
struct PairPendingReply<'a> {
    kind: &'static str,
    request_id: &'a str,
}

#[derive(Serialize)]
struct PairApprovedReply<'a> {
    kind: &'static str,
    device_id: &'a str,
}

#[derive(Serialize)]
struct PairDeniedReply {
    kind: &'static str,
}

async fn authenticate_socket(mut socket: WebSocket, state: GatewayState) {
    let mut nonce = vec![0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    if send_cbor(
        &mut socket,
        &AuthChallenge {
            kind: "auth.challenge",
            nonce: nonce.clone(),
        },
    )
    .await
    .is_err()
    {
        return;
    }

    let message = match tokio::time::timeout(AUTH_TIMEOUT, socket.next()).await {
        Ok(Some(Ok(Message::Binary(bytes)))) => bytes,
        _ => {
            close_socket(&mut socket).await;
            return;
        }
    };
    let frame_kind: ClientFrameKind = match ciborium::from_reader(message.as_ref()) {
        Ok(frame) => frame,
        Err(_) => {
            close_socket(&mut socket).await;
            return;
        }
    };
    if frame_kind.kind == "pair.request" {
        handle_pairing(socket, state, &message).await;
        return;
    }
    let proof: AuthProof = match ciborium::from_reader(message.as_ref()) {
        Ok(proof) => proof,
        Err(_) => {
            close_socket(&mut socket).await;
            return;
        }
    };
    if proof.kind != "auth.proof"
        || state
            .devices
            .verify_proof(&proof.device_id, &nonce, &proof.signature_der)
            .is_err()
    {
        close_socket(&mut socket).await;
        return;
    }
    if send_cbor(&mut socket, &AuthReply { kind: "auth.ok" })
        .await
        .is_err()
    {
        return;
    }

    run_authenticated_socket(socket, state.services).await;
}

/// Per-connection admission control for authenticated requests.
///
/// The websocket delivers frames in order, so a request id that does not
/// advance can only be a replay; tracking the highest id costs one word
/// instead of an unbounded set of seen ids. A token bucket caps the request
/// rate. Both faults are fatal: a well-behaved client cannot produce either,
/// so the connection closes rather than answering.
struct RequestGuard {
    highest_request_id: Option<u64>,
    tokens: f64,
    refilled_at: Instant,
}

impl RequestGuard {
    fn new(now: Instant) -> Self {
        Self {
            highest_request_id: None,
            tokens: MAX_REQUESTS_PER_SECOND,
            refilled_at: now,
        }
    }

    fn admit(&mut self, request_id: u64, now: Instant) -> Result<(), &'static str> {
        if self
            .highest_request_id
            .is_some_and(|highest| request_id <= highest)
        {
            return Err("protocol.replayed_request_id");
        }
        let elapsed = now
            .saturating_duration_since(self.refilled_at)
            .as_secs_f64();
        self.refilled_at = now;
        self.tokens =
            (self.tokens + elapsed * MAX_REQUESTS_PER_SECOND).min(MAX_REQUESTS_PER_SECOND);
        if self.tokens < 1.0 {
            return Err("protocol.rate_limited");
        }
        self.tokens -= 1.0;
        self.highest_request_id = Some(request_id);
        Ok(())
    }
}

struct ConnectionAttachment {
    tab_id: TabId,
    task: tokio::task::JoinHandle<()>,
}

struct StartedAttachment {
    tab_id: TabId,
    attachment_id: AttachmentId,
    events: RemoteTerminalEvents,
}

struct DispatchOutcome {
    frames: Vec<RemoteEvent>,
    started: Option<StartedAttachment>,
    tab_id: Option<TabId>,
}

impl DispatchOutcome {
    fn frames(frames: Vec<RemoteEvent>) -> Self {
        Self {
            frames,
            started: None,
            tab_id: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TabIdPayload {
    tab_id: TabId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachmentPayload {
    tab_id: TabId,
    attachment_id: AttachmentId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputPayload {
    tab_id: TabId,
    attachment_id: AttachmentId,
    data: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SizedAttachmentPayload {
    tab_id: TabId,
    attachment_id: AttachmentId,
    size: TerminalSize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScrollbackPayload {
    tab_id: TabId,
    attachment_id: AttachmentId,
    offset: usize,
    count: usize,
}

#[derive(Serialize)]
struct TabListPayload {
    tabs: Vec<crate::tabs::TabDescriptor>,
}

#[derive(Serialize)]
struct TabOpenedPayload<'a> {
    tab_id: &'a TabId,
}

#[derive(Serialize)]
struct AttachedPayload<'a> {
    tab_id: &'a TabId,
    attachment_id: &'a AttachmentId,
    has_focus: bool,
}

#[derive(Serialize)]
struct SuccessPayload {
    ok: bool,
}

#[derive(Serialize)]
struct ErrorPayload<'a> {
    code: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
struct FocusEventPayload<'a> {
    tab_id: &'a TabId,
    attachment_id: &'a AttachmentId,
    owner: &'a Option<AttachmentId>,
    size: TerminalSize,
}

#[derive(Serialize)]
struct TitleEventPayload<'a> {
    tab_id: &'a TabId,
    attachment_id: &'a AttachmentId,
    title: &'a str,
}

#[derive(Serialize)]
struct ExitEventPayload<'a> {
    tab_id: &'a TabId,
    attachment_id: &'a AttachmentId,
    exit: &'a crate::tabs::TabExit,
}

fn decode_payload<T: for<'de> Deserialize<'de>>(
    request: &RemoteRequest,
) -> Result<T, &'static str> {
    ciborium::from_reader(request.payload()).map_err(|_| "protocol.invalid_payload")
}

fn payload<T: Serialize>(value: &T) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).map_err(|_| "protocol.invalid_response")?;
    Ok(bytes)
}

fn response<T: Serialize>(
    request_id: u64,
    kind: &str,
    value: &T,
) -> Result<RemoteEvent, &'static str> {
    Ok(RemoteEvent {
        version: PROTOCOL_VERSION,
        request_id,
        kind: kind.to_owned(),
        payload: payload(value)?,
    })
}

fn error_response(request_id: u64, code: &str, message: &str) -> RemoteEvent {
    response(request_id, "error", &ErrorPayload { code, message })
        .expect("the fixed protocol error envelope is serializable")
}

fn chunk_event(chunk: TransferChunk) -> RemoteEvent {
    let kind = match chunk.kind {
        super::terminal::TransferKind::Snapshot => "terminal.snapshot",
        super::terminal::TransferKind::Diff => "terminal.diff",
        super::terminal::TransferKind::Scrollback => "terminal.scrollback",
    };
    RemoteEvent {
        version: PROTOCOL_VERSION,
        request_id: chunk.request_id,
        kind: kind.to_owned(),
        payload: payload(&chunk).expect("a validated transfer chunk is serializable"),
    }
}

impl RemoteServices {
    fn dispatch(
        &self,
        request: &RemoteRequest,
        attachments: &mut HashMap<AttachmentId, ConnectionAttachment>,
    ) -> DispatchOutcome {
        let result = self.dispatch_authorized(request, attachments);
        match result {
            Ok(outcome) => outcome,
            Err(code) => DispatchOutcome::frames(vec![error_response(
                request.request_id(),
                code,
                "the authenticated request could not be completed",
            )]),
        }
    }

    fn dispatch_authorized(
        &self,
        request: &RemoteRequest,
        attachments: &mut HashMap<AttachmentId, ConnectionAttachment>,
    ) -> Result<DispatchOutcome, &'static str> {
        let request_id = request.request_id();
        match request.kind() {
            kind if kind.starts_with("session.") || kind.starts_with("agent.") => {
                Ok(DispatchOutcome::frames(vec![error_response(
                    request_id,
                    "protocol.unsupported_request",
                    "this service is not available through the remote gateway yet",
                )]))
            }
            "tab.list" => Ok(DispatchOutcome::frames(vec![response(
                request_id,
                "tab.list",
                &TabListPayload {
                    tabs: self.registry.list(),
                },
            )?])),
            "tab.open" => {
                let launch: TabLaunch = decode_payload(request)?;
                let tab_id = self.terminal.open(launch).map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "tab.open",
                    &TabOpenedPayload { tab_id: &tab_id },
                )?]))
            }
            "tab.close" => {
                let request: TabIdPayload = decode_payload(request)?;
                self.terminal
                    .close(&request.tab_id)
                    .map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "tab.close",
                    &SuccessPayload { ok: true },
                )?]))
            }
            "terminal.attach" => {
                let request: TabIdPayload = decode_payload(request)?;
                let (attached, events) = self
                    .terminal
                    .attach(&request.tab_id)
                    .map_err(|error| error.code())?;
                let mut frames = vec![response(
                    request_id,
                    "terminal.attach",
                    &AttachedPayload {
                        tab_id: attached.tab_id(),
                        attachment_id: attached.attachment_id(),
                        has_focus: attached.has_focus(),
                    },
                )?];
                let chunks = chunk_snapshot_for_attachment(
                    request_id,
                    attached.tab_id(),
                    Some(attached.attachment_id()),
                    attached.snapshot(),
                )
                .map_err(|error| error.code())?;
                frames.extend(chunks.into_iter().map(chunk_event));
                Ok(DispatchOutcome {
                    frames,
                    tab_id: Some(attached.tab_id().clone()),
                    started: Some(StartedAttachment {
                        tab_id: attached.tab_id().clone(),
                        attachment_id: attached.attachment_id().clone(),
                        events,
                    }),
                })
            }
            "terminal.input" => {
                let request: InputPayload = decode_payload(request)?;
                authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                self.terminal
                    .input(&request.tab_id, &request.attachment_id, &request.data)
                    .map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "terminal.input",
                    &SuccessPayload { ok: true },
                )?]))
            }
            "terminal.resize" | "terminal.focus" => {
                let kind = request.kind();
                let body: SizedAttachmentPayload = decode_payload(request)?;
                authorize_attachment(attachments, &body.tab_id, &body.attachment_id)?;
                let result = if kind == "terminal.focus" {
                    self.terminal
                        .focus(&body.tab_id, &body.attachment_id, body.size)
                } else {
                    self.terminal
                        .resize(&body.tab_id, &body.attachment_id, body.size)
                };
                result.map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    kind,
                    &SuccessPayload { ok: true },
                )?]))
            }
            "terminal.detach" => {
                let request: AttachmentPayload = decode_payload(request)?;
                authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                self.terminal
                    .detach(&request.tab_id, &request.attachment_id)
                    .map_err(|error| error.code())?;
                if let Some(attachment) = attachments.remove(&request.attachment_id) {
                    attachment.task.abort();
                }
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "terminal.detach",
                    &SuccessPayload { ok: true },
                )?]))
            }
            "terminal.scrollback" => {
                let request: ScrollbackPayload = decode_payload(request)?;
                authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                let rows = self
                    .terminal
                    .scrollback(&request.tab_id, request.offset, request.count)
                    .map_err(|error| error.code())?;
                let revision = self
                    .registry
                    .snapshot(&request.tab_id)
                    .map_err(|error| error.code())?
                    .revision();
                let chunks = chunk_scrollback_for_attachment(
                    request_id,
                    &request.tab_id,
                    Some(&request.attachment_id),
                    revision,
                    rows,
                )
                .map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(
                    chunks.into_iter().map(chunk_event).collect(),
                ))
            }
            _ => Err("protocol.unsupported_request"),
        }
    }
}

fn authorize_attachment(
    attachments: &HashMap<AttachmentId, ConnectionAttachment>,
    tab_id: &TabId,
    attachment_id: &AttachmentId,
) -> Result<(), &'static str> {
    if attachments
        .get(attachment_id)
        .is_some_and(|attachment| &attachment.tab_id == tab_id)
    {
        Ok(())
    } else {
        Err("terminal.attachment_not_found")
    }
}

async fn forward_attachment_events(
    mut attachment: StartedAttachment,
    outbound: tokio::sync::mpsc::Sender<RemoteEvent>,
) {
    while let Some(event) = attachment.events.next().await {
        let frames = match event {
            TerminalEvent::Snapshot(snapshot) => chunk_snapshot_for_attachment(
                0,
                &attachment.tab_id,
                Some(&attachment.attachment_id),
                &snapshot,
            )
            .map(|chunks| chunks.into_iter().map(chunk_event).collect())
            .map_err(|_| ()),
            TerminalEvent::Diff(diff) => chunk_diff_for_attachment(
                0,
                &attachment.tab_id,
                Some(&attachment.attachment_id),
                &diff,
            )
            .map(|chunks| chunks.into_iter().map(chunk_event).collect())
            .map_err(|_| ()),
            TerminalEvent::FocusChanged { owner, size } => response(
                0,
                "terminal.focus_changed",
                &FocusEventPayload {
                    tab_id: &attachment.tab_id,
                    attachment_id: &attachment.attachment_id,
                    owner: &owner,
                    size,
                },
            )
            .map(|event| vec![event])
            .map_err(|_| ()),
            TerminalEvent::Title(title) => response(
                0,
                "terminal.title",
                &TitleEventPayload {
                    tab_id: &attachment.tab_id,
                    attachment_id: &attachment.attachment_id,
                    title: &title,
                },
            )
            .map(|event| vec![event])
            .map_err(|_| ()),
            TerminalEvent::Exited(exit) => response(
                0,
                "terminal.exited",
                &ExitEventPayload {
                    tab_id: &attachment.tab_id,
                    attachment_id: &attachment.attachment_id,
                    exit: &exit,
                },
            )
            .map(|event| vec![event])
            .map_err(|_| ()),
            TerminalEvent::Bell => continue,
        };
        let frames = match frames {
            Ok(frames) => frames,
            Err(_) => vec![error_response(
                0,
                "terminal.recovery_required",
                "terminal state could not be represented; request a fresh snapshot",
            )],
        };
        for frame in frames {
            if outbound.send(bound_event(frame)).await.is_err() {
                return;
            }
        }
    }
}

async fn run_authenticated_socket(mut socket: WebSocket, services: RemoteServices) {
    let mut guard = RequestGuard::new(Instant::now());
    let (outbound, mut outbound_events) = tokio::sync::mpsc::channel(OUTBOUND_EVENT_QUEUE);
    let mut attachments = HashMap::<AttachmentId, ConnectionAttachment>::new();
    'socket: loop {
        let message = tokio::select! {
            message = socket.next() => message,
            event = outbound_events.recv() => {
                let Some(event) = event else { break; };
                if send_remote_event(&mut socket, &event).await.is_err() {
                    break;
                }
                continue;
            }
        };
        let Some(message) = message else {
            break;
        };
        match message {
            Ok(Message::Binary(bytes)) => {
                let Ok(request) = RemoteRequest::decode(&bytes) else {
                    close_socket(&mut socket).await;
                    break;
                };
                if guard.admit(request.request_id(), Instant::now()).is_err() {
                    close_socket(&mut socket).await;
                    break;
                }
                let outcome = services.dispatch(&request, &mut attachments);
                for event in outcome.frames.into_iter().map(bound_event) {
                    if send_remote_event(&mut socket, &event).await.is_err() {
                        break 'socket;
                    }
                }
                if let Some(started) = outcome.started {
                    let id = started.attachment_id.clone();
                    let task = tokio::spawn(forward_attachment_events(started, outbound.clone()));
                    attachments.insert(
                        id,
                        ConnectionAttachment {
                            tab_id: outcome.tab_id.expect("started attachments name their tab"),
                            task,
                        },
                    );
                }
            }
            Ok(Message::Ping(bytes)) => {
                if socket.send(Message::Pong(bytes)).await.is_err() {
                    break;
                }
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {
                close_socket(&mut socket).await;
                break;
            }
        }
    }
    for (_, attachment) in attachments {
        attachment.task.abort();
    }
}

#[cfg(test)]
mod request_guard_tests {
    use super::*;

    #[test]
    fn a_repeated_request_id_is_refused() {
        let start = Instant::now();
        let mut guard = RequestGuard::new(start);
        guard.admit(7, start).unwrap();
        assert_eq!(guard.admit(7, start), Err("protocol.replayed_request_id"));
    }

    #[test]
    fn a_lower_request_id_is_refused() {
        let start = Instant::now();
        let mut guard = RequestGuard::new(start);
        guard.admit(9, start).unwrap();
        assert_eq!(guard.admit(8, start), Err("protocol.replayed_request_id"));
    }

    #[test]
    fn the_bucket_allows_one_hundred_and_twenty_immediate_requests() {
        let start = Instant::now();
        let mut guard = RequestGuard::new(start);
        for request_id in 1..=120 {
            guard.admit(request_id, start).unwrap();
        }
        assert_eq!(guard.admit(121, start), Err("protocol.rate_limited"));
    }

    #[test]
    fn the_bucket_refills_over_time() {
        let start = Instant::now();
        let mut guard = RequestGuard::new(start);
        for request_id in 1..=120 {
            guard.admit(request_id, start).unwrap();
        }
        let later = start + Duration::from_millis(100);
        for request_id in 121..=132 {
            guard.admit(request_id, later).unwrap();
        }
        assert_eq!(guard.admit(133, later), Err("protocol.rate_limited"));
    }
}

async fn handle_pairing(mut socket: WebSocket, state: GatewayState, bytes: &[u8]) {
    let request: PairRequest = match ciborium::from_reader::<PairRequest, _>(bytes) {
        Ok(request) if request.kind == "pair.request" => request,
        _ => {
            close_socket(&mut socket).await;
            return;
        }
    };
    let pending = match state.devices.submit_pairing_at(
        &request.enrollment_secret,
        &request.device_name,
        &request.public_key,
        SystemTime::now(),
    ) {
        Ok(pending) => pending,
        Err(_) => {
            close_socket(&mut socket).await;
            return;
        }
    };
    if send_cbor(
        &mut socket,
        &PairPendingReply {
            kind: "pair.pending",
            request_id: &pending.id,
        },
    )
    .await
    .is_err()
    {
        state.devices.deny_pairing(&pending.id).ok();
        return;
    }

    let outcome = tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            if let Some(outcome) = state.devices.take_pairing_outcome(&pending.id) {
                return outcome;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    match outcome {
        Ok(PairingOutcome::Approved(device)) => {
            send_cbor(
                &mut socket,
                &PairApprovedReply {
                    kind: "pair.approved",
                    device_id: &device.id,
                },
            )
            .await
            .ok();
        }
        Ok(PairingOutcome::Denied) => {
            send_cbor(
                &mut socket,
                &PairDeniedReply {
                    kind: "pair.denied",
                },
            )
            .await
            .ok();
        }
        Err(_) => {
            state.devices.deny_pairing(&pending.id).ok();
            send_cbor(
                &mut socket,
                &PairDeniedReply {
                    kind: "pair.expired",
                },
            )
            .await
            .ok();
        }
    }
}

async fn send_cbor<T: Serialize>(socket: &mut WebSocket, value: &T) -> Result<(), ()> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).map_err(|_| ())?;
    socket
        .send(Message::Binary(bytes.into()))
        .await
        .map_err(|_| ())
}

async fn send_remote_event(socket: &mut WebSocket, event: &RemoteEvent) -> Result<(), ()> {
    let mut bytes = Vec::new();
    ciborium::into_writer(event, &mut bytes).map_err(|_| ())?;
    if bytes.len() >= MAX_WIRE_FRAME_BYTES {
        return Err(());
    }
    socket
        .send(Message::Binary(bytes.into()))
        .await
        .map_err(|_| ())
}

fn bound_event(event: RemoteEvent) -> RemoteEvent {
    let mut bytes = Vec::new();
    if ciborium::into_writer(&event, &mut bytes).is_ok() && bytes.len() < MAX_WIRE_FRAME_BYTES {
        return event;
    }
    error_response(
        event.request_id,
        "protocol.frame_too_large",
        "the typed response cannot fit in one wire frame",
    )
}

async fn close_socket(socket: &mut WebSocket) {
    socket.send(Message::Close(None)).await.ok();
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayError {
    code: &'static str,
    message: String,
}

impl GatewayError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn io(error: std::io::Error) -> Self {
        Self::new("gateway.io_failed", error.to_string())
    }

    fn tls(error: impl fmt::Display) -> Self {
        Self::new("gateway.tls_failed", error.to_string())
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for GatewayError {}
