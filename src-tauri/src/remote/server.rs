use super::auth::{set_private_permissions, write_private_file, DeviceStore, PairingOutcome};
use super::model::{
    decode_exact, encode_terminal_frame, RemoteEvent, RemoteRequest, TerminalSize, PROTOCOL_VERSION,
};
use super::terminal::{
    plan_diff_for_attachment, plan_scrollback_for_attachment, plan_shared_snapshot_for_attachment,
    plan_snapshot_for_attachment, RemoteTerminal, RemoteTerminalEvents, TerminalEvent,
    TransferChunk, TransferPlan,
};
use crate::tabs::{
    AttachmentId, RecoveryBoundary, TabAttachmentCancellation, TabDescriptor, TabId, TabLaunch,
    TabRegistry, TabState,
};
use crate::terminal::model::Revision;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::routing::get;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use rand_core::{OsRng, RngCore};
use rcgen::PublicKeyData;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
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
const MAX_CONNECTIONS: usize = 64;
const MAX_ATTACHMENTS_PER_CONNECTION: usize = 8;
pub const MAX_SCROLLBACK_PAGE_ROWS: usize = 256;
pub const MAX_TERMINAL_INPUT_BYTES: usize = 64 * 1024;
const MAX_TITLE_BYTES: usize = 4 * 1024;
const MAX_PATH_BYTES: usize = 4 * 1024;
const MAX_COMMAND_BYTES: usize = 32 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 4 * 1024;
const ATTACHMENT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const ATTACHMENT_REAP_INTERVAL: Duration = Duration::from_millis(100);
const EGRESS_SEND_TIMEOUT: Duration = Duration::from_secs(1);
const EGRESS_CONTROL_QUEUE: usize = 64;
const EGRESS_TRANSFER_QUEUE: usize = 1;

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
    connections: Arc<tokio::sync::Semaphore>,
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
        let state = GatewayState {
            devices,
            services,
            connections: Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS)),
        };
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
        .on_upgrade(move |mut socket| async move {
            let Ok(permit) = state.connections.clone().try_acquire_owned() else {
                close_socket(&mut socket).await;
                return;
            };
            authenticate_socket(socket, state).await;
            drop(permit);
        })
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
        Ok(Some(Ok(Message::Binary(bytes)))) if bytes.len() < MAX_MESSAGE_SIZE => bytes,
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
    let proof: AuthProof = match decode_exact(message.as_ref()) {
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
    cancellation: TabAttachmentCancellation,
    commands: tokio::sync::mpsc::Sender<AttachmentCommand>,
    task: tokio::task::JoinHandle<()>,
}

struct StartedAttachment {
    tab_id: TabId,
    attachment_id: AttachmentId,
    events: RemoteTerminalEvents,
    cancellation: TabAttachmentCancellation,
    revision: Revision,
}

#[derive(Clone)]
struct EgressHandle {
    controls: tokio::sync::mpsc::Sender<EgressControl>,
    transfers: tokio::sync::mpsc::Sender<TaggedTransfer>,
}

struct TaggedTransfer {
    attachment_id: Option<AttachmentId>,
    plan: TransferPlan,
}

enum EgressControl {
    Message(Message),
    Resume {
        attachment_id: AttachmentId,
        reply: Vec<u8>,
        transfer: TransferPlan,
        done: tokio::sync::oneshot::Sender<Result<(), ()>>,
    },
    Detach {
        attachment_id: AttachmentId,
        reply: Vec<u8>,
        done: tokio::sync::oneshot::Sender<Result<(), ()>>,
    },
    Close,
}

enum AttachmentCommand {
    Resume {
        request_id: u64,
        tab_id: TabId,
        attachment_id: AttachmentId,
        requested_revision: Revision,
        done: tokio::sync::oneshot::Sender<Result<(), ()>>,
    },
    Detach {
        frames: Vec<RemoteEvent>,
        done: tokio::sync::oneshot::Sender<Result<(), ()>>,
    },
    Shutdown,
}

enum SequencedAction {
    Resume {
        request_id: u64,
        tab_id: TabId,
        attachment_id: AttachmentId,
        requested_revision: Revision,
    },
    Detach {
        attachment_id: AttachmentId,
    },
}

struct DispatchOutcome {
    frames: Vec<RemoteEvent>,
    transfers: Vec<TransferPlan>,
    started: Option<StartedAttachment>,
    tab_id: Option<TabId>,
    sequenced: Option<SequencedAction>,
}

impl DispatchOutcome {
    fn frames(frames: Vec<RemoteEvent>) -> Self {
        Self {
            frames,
            transfers: Vec::new(),
            started: None,
            tab_id: None,
            sequenced: None,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumePayload {
    tab_id: TabId,
    attachment_id: AttachmentId,
    revision: Revision,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TabOpenPayload {
    title: String,
    cwd: Option<String>,
    command: Option<String>,
    session_id: Option<String>,
    resumed_id: Option<String>,
    agent_id: Option<String>,
    slot_id: String,
    #[serde(default)]
    fresh: bool,
    env_provider: Option<String>,
    env_model: Option<String>,
    size: TerminalSize,
}

#[derive(Serialize)]
struct TabListPayload {
    tabs: Vec<RemoteTabDescriptor>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteTabDescriptor {
    id: TabId,
    title: String,
    cwd: Option<String>,
    command: Option<String>,
    session_id: Option<String>,
    resumed_id: Option<String>,
    agent_id: Option<String>,
    slot_id: String,
    fresh: bool,
    env_provider: Option<String>,
    env_model: Option<String>,
    size: TerminalSize,
    focus: RemoteFocusState,
    state: TabState,
    exit: Option<crate::tabs::TabExit>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum RemoteFocusState {
    #[serde(rename = "self")]
    Self_,
    Other,
    Unowned,
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
    title: &'a str,
}

#[derive(Serialize)]
struct ResumeReplyPayload<'a> {
    tab_id: &'a TabId,
    attachment_id: &'a AttachmentId,
    requested_revision: Revision,
    current_revision: Revision,
    recovery_required: bool,
    title: &'a str,
    focus: RemoteFocusState,
    size: TerminalSize,
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
    focus: RemoteFocusState,
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
    decode_exact(request.payload()).map_err(|_| "protocol.invalid_payload")
}

fn payload<T: Serialize>(value: &T) -> Result<Vec<u8>, &'static str> {
    encode_terminal_frame(value).map_err(|error| {
        if error.code() == "protocol.frame_too_large" {
            "protocol.response_too_large"
        } else {
            "protocol.invalid_response"
        }
    })
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

fn remote_focus(
    owner: Option<&AttachmentId>,
    own_attachment: Option<&AttachmentId>,
) -> RemoteFocusState {
    match owner {
        None => RemoteFocusState::Unowned,
        Some(owner) if Some(owner) == own_attachment => RemoteFocusState::Self_,
        Some(_) => RemoteFocusState::Other,
    }
}

fn remote_descriptor(
    descriptor: TabDescriptor,
    attachments: &HashMap<AttachmentId, TabId>,
) -> RemoteTabDescriptor {
    let own_attachment = descriptor.input_owner().filter(|owner| {
        attachments
            .get(*owner)
            .is_some_and(|tab_id| tab_id == descriptor.id())
    });
    RemoteTabDescriptor {
        id: descriptor.id().clone(),
        title: descriptor.title().to_owned(),
        cwd: descriptor.cwd().map(str::to_owned),
        command: descriptor.command().map(str::to_owned),
        session_id: descriptor.session_id().map(str::to_owned),
        resumed_id: descriptor.resumed_id().map(str::to_owned),
        agent_id: descriptor.agent_id().map(str::to_owned),
        slot_id: descriptor.slot_id().to_owned(),
        fresh: descriptor.fresh(),
        env_provider: descriptor.env_provider().map(str::to_owned),
        env_model: descriptor.env_model().map(str::to_owned),
        size: descriptor.size(),
        focus: remote_focus(descriptor.input_owner(), own_attachment),
        state: descriptor.state().clone(),
        exit: descriptor.exit().cloned(),
    }
}

fn bounded(value: &str, max: usize) -> Result<(), &'static str> {
    if value.len() > max {
        Err("protocol.value_too_large")
    } else {
        Ok(())
    }
}

fn launch_from_wire(wire: TabOpenPayload) -> Result<TabLaunch, &'static str> {
    bounded(&wire.title, MAX_TITLE_BYTES)?;
    bounded(&wire.slot_id, MAX_IDENTIFIER_BYTES)?;
    if let Some(value) = wire.cwd.as_deref() {
        bounded(value, MAX_PATH_BYTES)?;
    }
    if let Some(value) = wire.command.as_deref() {
        bounded(value, MAX_COMMAND_BYTES)?;
    }
    for value in [
        wire.session_id.as_deref(),
        wire.resumed_id.as_deref(),
        wire.agent_id.as_deref(),
        wire.env_provider.as_deref(),
        wire.env_model.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        bounded(value, MAX_IDENTIFIER_BYTES)?;
    }
    let mut launch = TabLaunch::new(wire.title, wire.slot_id, wire.size).with_fresh(wire.fresh);
    if let Some(value) = wire.cwd {
        launch = launch.with_cwd(value);
    }
    if let Some(value) = wire.command {
        launch = launch.with_command(value);
    }
    if let Some(value) = wire.session_id {
        launch = launch.with_session_id(value);
    }
    if let Some(value) = wire.resumed_id {
        launch = launch.with_resumed_id(value);
    }
    if let Some(value) = wire.agent_id {
        launch = launch.with_agent_id(value);
    }
    if let (Some(provider), Some(model)) = (wire.env_provider, wire.env_model) {
        launch = launch.with_environment(provider, model);
    }
    Ok(launch)
}

impl RemoteServices {
    fn dispatch(
        &self,
        request: &RemoteRequest,
        attachments: &HashMap<AttachmentId, TabId>,
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
        attachments: &HashMap<AttachmentId, TabId>,
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
            "tab.list" => {
                if !request.payload().is_empty() {
                    return Err("protocol.invalid_payload");
                }
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "tab.list",
                    &TabListPayload {
                        tabs: self
                            .registry
                            .list()
                            .into_iter()
                            .map(|descriptor| remote_descriptor(descriptor, attachments))
                            .collect(),
                    },
                )?]))
            }
            "tab.open" => {
                let launch = launch_from_wire(decode_payload::<TabOpenPayload>(request)?)?;
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
                let tab_id = attached.tab_id().clone();
                let attachment_id = attached.attachment_id().clone();
                let has_focus = attached.has_focus();
                let title = attached.title().to_owned();
                let snapshot = attached.into_snapshot();
                let revision = snapshot.revision();
                let frames = vec![response(
                    request_id,
                    "terminal.attach",
                    &AttachedPayload {
                        tab_id: &tab_id,
                        attachment_id: &attachment_id,
                        has_focus,
                        title: &title,
                    },
                )?];
                let transfer = plan_snapshot_for_attachment(
                    request_id,
                    &tab_id,
                    Some(&attachment_id),
                    snapshot,
                )
                .map_err(|error| error.code())?;
                Ok(DispatchOutcome {
                    frames,
                    transfers: vec![transfer],
                    tab_id: Some(tab_id.clone()),
                    started: Some(StartedAttachment {
                        tab_id,
                        attachment_id,
                        cancellation: events.cancellation(),
                        revision,
                        events,
                    }),
                    sequenced: None,
                })
            }
            "terminal.input" => {
                let request: InputPayload = decode_payload(request)?;
                authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                if request.data.len() > MAX_TERMINAL_INPUT_BYTES {
                    return Err("terminal.input_too_large");
                }
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
                Ok(DispatchOutcome {
                    frames: vec![response(
                        request_id,
                        "terminal.detach",
                        &SuccessPayload { ok: true },
                    )?],
                    transfers: Vec::new(),
                    started: None,
                    tab_id: None,
                    sequenced: Some(SequencedAction::Detach {
                        attachment_id: request.attachment_id,
                    }),
                })
            }
            "terminal.scrollback" => {
                let request: ScrollbackPayload = decode_payload(request)?;
                authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                if request.count == 0
                    || request.count > MAX_SCROLLBACK_PAGE_ROWS
                    || request.offset > crate::terminal::MAX_SCROLLBACK_ROWS
                {
                    return Err("terminal.invalid_scrollback_page");
                }
                let page = self
                    .registry
                    .scrollback_page(&request.tab_id, request.offset, request.count)
                    .map_err(|error| error.code())?;
                let transfer = plan_scrollback_for_attachment(
                    request_id,
                    &request.tab_id,
                    Some(&request.attachment_id),
                    page.revision(),
                    page.into_rows(),
                )
                .map_err(|error| error.code())?;
                let mut outcome = DispatchOutcome::frames(Vec::new());
                outcome.transfers.push(transfer);
                Ok(outcome)
            }
            "terminal.resume" => {
                let request: ResumePayload = decode_payload(request)?;
                authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                let mut outcome = DispatchOutcome::frames(Vec::new());
                outcome.sequenced = Some(SequencedAction::Resume {
                    request_id,
                    tab_id: request.tab_id,
                    attachment_id: request.attachment_id,
                    requested_revision: request.revision,
                });
                Ok(outcome)
            }
            _ => Err("protocol.unsupported_request"),
        }
    }
}

fn authorize_attachment(
    attachments: &HashMap<AttachmentId, TabId>,
    tab_id: &TabId,
    attachment_id: &AttachmentId,
) -> Result<(), &'static str> {
    if attachments
        .get(attachment_id)
        .is_some_and(|attachment_tab| attachment_tab == tab_id)
    {
        Ok(())
    } else {
        Err("terminal.attachment_not_found")
    }
}

async fn enqueue_event(outbound: &EgressHandle, event: RemoteEvent) -> Result<(), ()> {
    let bytes = tokio::task::spawn_blocking(move || encode_terminal_frame(&event))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    outbound
        .controls
        .send(EgressControl::Message(Message::Binary(bytes.into())))
        .await
        .map_err(|_| ())
}

async fn enqueue_transfer(outbound: &EgressHandle, transfer: TransferPlan) -> Result<(), ()> {
    let attachment_id = transfer.attachment_id().cloned();
    outbound
        .transfers
        .send(TaggedTransfer {
            attachment_id,
            plan: transfer,
        })
        .await
        .map_err(|_| ())
}

async fn enqueue_outcome(
    outbound: &EgressHandle,
    frames: Vec<RemoteEvent>,
    transfers: Vec<TransferPlan>,
) -> Result<(), ()> {
    for frame in frames {
        enqueue_event(outbound, frame).await?;
    }
    for transfer in transfers {
        enqueue_transfer(outbound, transfer).await?;
    }
    Ok(())
}

async fn encode_event(event: RemoteEvent) -> Result<Vec<u8>, ()> {
    tokio::task::spawn_blocking(move || encode_terminal_frame(&event))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn send_egress(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: Message,
) -> Result<(), ()> {
    tokio::time::timeout(EGRESS_SEND_TIMEOUT, sink.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn egress_arbiter(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut controls: tokio::sync::mpsc::Receiver<EgressControl>,
    mut transfers: tokio::sync::mpsc::Receiver<TaggedTransfer>,
    failed: tokio::sync::watch::Sender<bool>,
) {
    let mut active: Option<TaggedTransfer> = None;
    let mut queued = VecDeque::<TaggedTransfer>::new();
    let mut detached = HashSet::<AttachmentId>::new();
    let mut pending_control = None;
    let result = async {
        loop {
            while let Some(command) = pending_control.take().or_else(|| controls.try_recv().ok()) {
                match command {
                    EgressControl::Message(message) => send_egress(&mut sink, message).await?,
                    EgressControl::Resume {
                        attachment_id,
                        reply,
                        transfer,
                        done,
                    } => {
                        while let Ok(pending) = transfers.try_recv() {
                            if pending.attachment_id.as_ref() != Some(&attachment_id) {
                                queued.push_back(pending);
                            }
                        }
                        if active
                            .as_ref()
                            .and_then(|current| current.attachment_id.as_ref())
                            == Some(&attachment_id)
                        {
                            active = None;
                        }
                        queued.retain(|current| {
                            current.attachment_id.as_ref() != Some(&attachment_id)
                        });
                        detached.remove(&attachment_id);
                        let sent = send_egress(&mut sink, Message::Binary(reply.into())).await;
                        let send_failed = sent.is_err();
                        if sent.is_ok() {
                            let replacement = TaggedTransfer {
                                attachment_id: Some(attachment_id),
                                plan: transfer,
                            };
                            if active.is_none() {
                                active = Some(replacement);
                            } else {
                                queued.push_front(replacement);
                            }
                        }
                        let _ = done.send(sent);
                        if send_failed {
                            return Err(());
                        }
                    }
                    EgressControl::Detach {
                        attachment_id,
                        reply,
                        done,
                    } => {
                        while let Ok(pending) = transfers.try_recv() {
                            if pending.attachment_id.as_ref() != Some(&attachment_id) {
                                queued.push_back(pending);
                            }
                        }
                        if active
                            .as_ref()
                            .and_then(|current| current.attachment_id.as_ref())
                            == Some(&attachment_id)
                        {
                            active = None;
                        }
                        queued.retain(|current| {
                            current.attachment_id.as_ref() != Some(&attachment_id)
                        });
                        detached.insert(attachment_id);
                        let sent = send_egress(&mut sink, Message::Binary(reply.into())).await;
                        let send_failed = sent.is_err();
                        let _ = done.send(sent);
                        if send_failed {
                            return Err(());
                        }
                    }
                    EgressControl::Close => {
                        let _ = send_egress(&mut sink, Message::Close(None)).await;
                        return Ok::<(), ()>(());
                    }
                }
            }
            while let Ok(transfer) = transfers.try_recv() {
                if transfer
                    .attachment_id
                    .as_ref()
                    .is_some_and(|id| detached.contains(id))
                {
                    continue;
                }
                if active.is_none() {
                    active = Some(transfer);
                } else {
                    queued.push_back(transfer);
                }
            }
            if active.is_none() {
                active = queued.pop_front();
            }
            if let Some(mut current) = active.take() {
                let (returned, encoded) = tokio::task::spawn_blocking(move || {
                    let encoded = match current.plan.next_chunk() {
                        Ok(Some(chunk)) => encode_terminal_frame(&chunk_event(chunk))
                            .map(Some)
                            .map_err(|_| ()),
                        Ok(None) => Ok(None),
                        Err(_) => Err(()),
                    };
                    (current, encoded)
                })
                .await
                .map_err(|_| ())?;
                match encoded? {
                    Some(bytes) => {
                        send_egress(&mut sink, Message::Binary(bytes.into())).await?;
                        active = Some(returned);
                    }
                    None => {}
                }
                continue;
            }
            tokio::select! {
                biased;
                command = controls.recv() => match command {
                    Some(command) => pending_control = Some(command),
                    None if transfers.is_closed() => return Ok(()),
                    None => {}
                },
                transfer = transfers.recv() => match transfer {
                    Some(transfer) => active = Some(transfer),
                    None if controls.is_closed() => return Ok(()),
                    None => {}
                },
            }
        }
    }
    .await;
    if result.is_err() {
        let _ = failed.send(true);
    }
}

async fn encode_control_event(
    tab_id: TabId,
    attachment_id: AttachmentId,
    event: TerminalEvent,
) -> Result<Option<RemoteEvent>, ()> {
    tokio::task::spawn_blocking(move || match event {
        TerminalEvent::FocusChanged { owner, size } => response(
            0,
            "terminal.focus_changed",
            &FocusEventPayload {
                tab_id: &tab_id,
                attachment_id: &attachment_id,
                focus: remote_focus(owner.as_ref(), Some(&attachment_id)),
                size,
            },
        )
        .map(Some)
        .map_err(|_| ()),
        TerminalEvent::Title(title) => response(
            0,
            "terminal.title",
            &TitleEventPayload {
                tab_id: &tab_id,
                attachment_id: &attachment_id,
                title: &title,
            },
        )
        .map(Some)
        .map_err(|_| ()),
        TerminalEvent::Exited(exit) => response(
            0,
            "terminal.exited",
            &ExitEventPayload {
                tab_id: &tab_id,
                attachment_id: &attachment_id,
                exit: &exit,
            },
        )
        .map(Some)
        .map_err(|_| ()),
        TerminalEvent::Bell => Ok(None),
        TerminalEvent::Snapshot(_) | TerminalEvent::SharedSnapshot(_) | TerminalEvent::Diff(_) => {
            Err(())
        }
    })
    .await
    .map_err(|_| ())?
}

struct ResumeEmission {
    boundary: RecoveryBoundary,
    revision: Revision,
    frames: Vec<RemoteEvent>,
    transfers: Vec<TransferPlan>,
}

fn build_resume_emission(
    terminal: RemoteTerminal,
    request_id: u64,
    tab_id: TabId,
    attachment_id: AttachmentId,
    requested_revision: Revision,
) -> Result<ResumeEmission, RemoteEvent> {
    let recovery = terminal
        .resume(&tab_id, &attachment_id, requested_revision)
        .map_err(|error| {
            error_response(
                request_id,
                error.code(),
                "the authenticated recovery request could not be completed",
            )
        })?;
    let (snapshot, boundary, title, owner, size) = recovery.into_parts();
    let revision = snapshot.revision();
    let frame = response(
        request_id,
        "terminal.resume",
        &ResumeReplyPayload {
            tab_id: &tab_id,
            attachment_id: &attachment_id,
            requested_revision,
            current_revision: revision,
            recovery_required: requested_revision != revision,
            title: &title,
            focus: remote_focus(owner.as_ref(), Some(&attachment_id)),
            size,
        },
    )
    .map_err(|code| {
        error_response(
            request_id,
            code,
            "the recovery reply could not be represented",
        )
    })?;
    let transfer =
        plan_snapshot_for_attachment(request_id, &tab_id, Some(&attachment_id), snapshot).map_err(
            |error| {
                error_response(
                    request_id,
                    error.code(),
                    "the recovery snapshot could not be represented",
                )
            },
        )?;
    Ok(ResumeEmission {
        boundary,
        revision,
        frames: vec![frame],
        transfers: vec![transfer],
    })
}

async fn attachment_actor(
    mut attachment: StartedAttachment,
    mut commands: tokio::sync::mpsc::Receiver<AttachmentCommand>,
    outbound: EgressHandle,
    terminal: RemoteTerminal,
) {
    let mut live_revision = Some(attachment.revision);
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(AttachmentCommand::Resume {
                    request_id,
                    tab_id,
                    attachment_id,
                    requested_revision,
                    done,
                }) => {
                    let resume_terminal = terminal.clone();
                    let emission = tokio::task::spawn_blocking(move || build_resume_emission(
                        resume_terminal,
                        request_id,
                        tab_id,
                        attachment_id,
                        requested_revision,
                    )).await;
                    let result = match emission {
                        Ok(Ok(emission)) => {
                            attachment.events.apply_recovery_boundary(emission.boundary);
                            live_revision = Some(emission.revision);
                            let mut frames = emission.frames;
                            let mut transfers = emission.transfers;
                            let encoded = match (frames.pop(), transfers.pop()) {
                                (Some(frame), Some(transfer)) => encode_event(frame)
                                    .await
                                    .map(|reply| (reply, transfer)),
                                _ => Err(()),
                            };
                            match encoded {
                                Ok((reply, transfer)) => {
                                    let (sent, completed) = tokio::sync::oneshot::channel();
                                    if outbound.controls.send(EgressControl::Resume {
                                        attachment_id: attachment.attachment_id.clone(),
                                        reply,
                                        transfer,
                                        done: sent,
                                    }).await.is_err() {
                                        Err(())
                                    } else {
                                        completed.await.unwrap_or(Err(()))
                                    }
                                }
                                Err(()) => Err(()),
                            }
                        }
                        Ok(Err(error)) => enqueue_event(&outbound, error).await,
                        Err(_) => Err(()),
                    };
                    let failed = result.is_err();
                    let _ = done.send(result);
                    if failed { return; }
                }
                Some(AttachmentCommand::Detach { frames, done }) => {
                    let cancellation = attachment.events.cancellation();
                    cancellation.close_mailbox();
                    let result = match frames.into_iter().next() {
                        Some(frame) => match encode_event(frame).await {
                            Ok(reply) => {
                                let (sent, completed) = tokio::sync::oneshot::channel();
                                if outbound.controls.send(EgressControl::Detach {
                                    attachment_id: attachment.attachment_id.clone(),
                                    reply,
                                    done: sent,
                                }).await.is_err() {
                                    Err(())
                                } else {
                                    completed.await.unwrap_or(Err(()))
                                }
                            }
                            Err(()) => Err(()),
                        },
                        None => Err(()),
                    };
                    detach_attachment_bounded(cancellation).await;
                    let _ = done.send(result);
                    return;
                }
                Some(AttachmentCommand::Shutdown) | None => {
                    let cancellation = attachment.events.cancellation();
                    cancellation.close_mailbox();
                    detach_attachment_bounded(cancellation).await;
                    return;
                }
            },
            event = attachment.events.next() => {
                let Some(event) = event else { return; };
                let result = match event {
                    TerminalEvent::Snapshot(snapshot) => {
                        live_revision = Some(snapshot.revision());
                        let tab_id = attachment.tab_id.clone();
                        let attachment_id = attachment.attachment_id.clone();
                        match tokio::task::spawn_blocking(move || {
                            plan_snapshot_for_attachment(0, &tab_id, Some(&attachment_id), snapshot)
                        }).await {
                            Ok(Ok(transfer)) => enqueue_transfer(&outbound, transfer).await,
                            _ => enqueue_event(&outbound, recovery_error()).await,
                        }
                    }
                    TerminalEvent::SharedSnapshot(snapshot) => {
                        live_revision = Some(snapshot.revision());
                        let tab_id = attachment.tab_id.clone();
                        let attachment_id = attachment.attachment_id.clone();
                        match tokio::task::spawn_blocking(move || {
                            plan_shared_snapshot_for_attachment(
                                0,
                                &tab_id,
                                Some(&attachment_id),
                                snapshot,
                            )
                        }).await {
                            Ok(Ok(transfer)) => enqueue_transfer(&outbound, transfer).await,
                            _ => enqueue_event(&outbound, recovery_error()).await,
                        }
                    }
                    TerminalEvent::Diff(diff) => {
                        if live_revision != Some(diff.base_revision()) {
                            live_revision = None;
                            enqueue_event(&outbound, recovery_error()).await
                        } else {
                            live_revision = Some(diff.revision());
                            let tab_id = attachment.tab_id.clone();
                            let attachment_id = attachment.attachment_id.clone();
                            match tokio::task::spawn_blocking(move || {
                                plan_diff_for_attachment(0, &tab_id, Some(&attachment_id), diff)
                            }).await {
                                Ok(Ok(transfer)) => enqueue_transfer(&outbound, transfer).await,
                                _ => enqueue_event(&outbound, recovery_error()).await,
                            }
                        }
                    }
                    control => match encode_control_event(
                        attachment.tab_id.clone(),
                        attachment.attachment_id.clone(),
                        control,
                    ).await {
                        Ok(Some(event)) => enqueue_event(&outbound, event).await,
                        Ok(None) => Ok(()),
                        Err(()) => Err(()),
                    },
                };
                if result.is_err() { return; }
            }
        }
    }
}

fn recovery_error() -> RemoteEvent {
    error_response(
        0,
        "terminal.recovery_required",
        "terminal revision gap; request a fresh snapshot",
    )
}

async fn run_authenticated_socket(socket: WebSocket, services: RemoteServices) {
    let mut guard = RequestGuard::new(Instant::now());
    let (socket_sink, mut socket_stream) = socket.split();
    let (controls, control_receiver) = tokio::sync::mpsc::channel(EGRESS_CONTROL_QUEUE);
    let (transfers, transfer_receiver) = tokio::sync::mpsc::channel(EGRESS_TRANSFER_QUEUE);
    let outbound = EgressHandle {
        controls,
        transfers,
    };
    let (writer_failed, mut writer_failure) = tokio::sync::watch::channel(false);
    let mut writer = tokio::spawn(egress_arbiter(
        socket_sink,
        control_receiver,
        transfer_receiver,
        writer_failed,
    ));
    let mut attachments = HashMap::<AttachmentId, ConnectionAttachment>::new();
    let mut reap_tick = tokio::time::interval(ATTACHMENT_REAP_INTERVAL);
    reap_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    'socket: loop {
        reap_finished_attachments(&mut attachments).await;
        let message = tokio::select! {
            message = socket_stream.next() => message,
            changed = writer_failure.changed() => {
                if changed.is_err() || *writer_failure.borrow() { break; }
                continue;
            }
            _ = reap_tick.tick() => continue,
        };
        let Some(message) = message else {
            break;
        };
        match message {
            Ok(Message::Binary(bytes)) => {
                if bytes.len() >= MAX_MESSAGE_SIZE {
                    let _ = outbound.controls.send(EgressControl::Close).await;
                    break;
                }
                let request = match RemoteRequest::decode(&bytes) {
                    Ok(request) => request,
                    Err(error) => {
                        let Some(request_id) = error.request_id() else {
                            let _ = outbound.controls.send(EgressControl::Close).await;
                            break;
                        };
                        if guard.admit(request_id, Instant::now()).is_err() {
                            let _ = outbound.controls.send(EgressControl::Close).await;
                            break;
                        }
                        if enqueue_event(
                            &outbound,
                            error_response(request_id, error.code(), error.message()),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                };
                if guard.admit(request.request_id(), Instant::now()).is_err() {
                    let _ = outbound.controls.send(EgressControl::Close).await;
                    break;
                }
                if request.kind() == "terminal.attach"
                    && attachments.len() >= MAX_ATTACHMENTS_PER_CONNECTION
                {
                    if enqueue_event(
                        &outbound,
                        error_response(
                            request.request_id(),
                            "terminal.too_many_attachments",
                            "the connection attachment limit was reached",
                        ),
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                    continue;
                }
                let owned = attachments
                    .iter()
                    .map(|(id, attachment)| (id.clone(), attachment.tab_id.clone()))
                    .collect::<HashMap<_, _>>();
                let dispatch_services = services.clone();
                let dispatch_request = request.clone();
                let outcome = match tokio::task::spawn_blocking(move || {
                    dispatch_services.dispatch(&dispatch_request, &owned)
                })
                .await
                {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        let _ = outbound.controls.send(EgressControl::Close).await;
                        break;
                    }
                };
                let DispatchOutcome {
                    frames,
                    transfers,
                    started,
                    tab_id,
                    sequenced,
                } = outcome;
                match sequenced {
                    Some(SequencedAction::Resume {
                        request_id,
                        tab_id,
                        attachment_id,
                        requested_revision,
                    }) => {
                        let Some(attachment) = attachments.get(&attachment_id) else {
                            break 'socket;
                        };
                        let (done, completed) = tokio::sync::oneshot::channel();
                        if attachment
                            .commands
                            .send(AttachmentCommand::Resume {
                                request_id,
                                tab_id,
                                attachment_id,
                                requested_revision,
                                done,
                            })
                            .await
                            .is_err()
                            || completed.await.ok() != Some(Ok(()))
                        {
                            break 'socket;
                        }
                    }
                    Some(SequencedAction::Detach { attachment_id }) => {
                        let Some(mut attachment) = attachments.remove(&attachment_id) else {
                            break 'socket;
                        };
                        let (done, completed) = tokio::sync::oneshot::channel();
                        if attachment
                            .commands
                            .send(AttachmentCommand::Detach { frames, done })
                            .await
                            .is_err()
                            || completed.await.ok() != Some(Ok(()))
                        {
                            shutdown_attachment(attachment).await;
                            break 'socket;
                        }
                        let _ = (&mut attachment.task).await;
                    }
                    None => {
                        if enqueue_outcome(&outbound, frames, transfers).await.is_err() {
                            break 'socket;
                        }
                    }
                }
                if let Some(started) = started {
                    let id = started.attachment_id.clone();
                    let cancellation = started.cancellation.clone();
                    let (commands, command_receiver) = tokio::sync::mpsc::channel(8);
                    let task = tokio::spawn(attachment_actor(
                        started,
                        command_receiver,
                        outbound.clone(),
                        services.terminal.clone(),
                    ));
                    attachments.insert(
                        id,
                        ConnectionAttachment {
                            tab_id: tab_id.expect("started attachments name their tab"),
                            cancellation,
                            commands,
                            task,
                        },
                    );
                }
            }
            Ok(Message::Ping(bytes)) => {
                if bytes.len() >= MAX_MESSAGE_SIZE
                    || outbound
                        .controls
                        .send(EgressControl::Message(Message::Pong(bytes)))
                        .await
                        .is_err()
                {
                    break;
                }
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {
                let _ = outbound.controls.send(EgressControl::Close).await;
                break;
            }
        }
    }
    for (_, attachment) in attachments {
        shutdown_attachment(attachment).await;
    }
    let _ = tokio::time::timeout(
        ATTACHMENT_SHUTDOWN_TIMEOUT,
        outbound.controls.send(EgressControl::Close),
    )
    .await;
    drop(outbound);
    if tokio::time::timeout(ATTACHMENT_SHUTDOWN_TIMEOUT, &mut writer)
        .await
        .is_err()
    {
        writer.abort();
        let _ = writer.await;
    }
}

async fn shutdown_attachment(mut attachment: ConnectionAttachment) {
    attachment.cancellation.close_mailbox();
    let _ = attachment.commands.try_send(AttachmentCommand::Shutdown);
    detach_attachment_bounded(attachment.cancellation.clone()).await;
    if tokio::time::timeout(ATTACHMENT_SHUTDOWN_TIMEOUT, &mut attachment.task)
        .await
        .is_err()
    {
        attachment.task.abort();
        let _ = attachment.task.await;
    }
}

async fn detach_attachment_bounded(cancellation: TabAttachmentCancellation) {
    cancellation.close_mailbox();
    let mut detach = tokio::task::spawn_blocking(move || cancellation.detach_registry());
    if tokio::time::timeout(ATTACHMENT_SHUTDOWN_TIMEOUT, &mut detach)
        .await
        .is_err()
    {
        // spawn_blocking cannot be force-cancelled after it starts. Dropping
        // the handle keeps connection teardown bounded; the idempotent detach
        // completes once the ordered backend operation releases.
        drop(detach);
    }
}

async fn reap_finished_attachments(attachments: &mut HashMap<AttachmentId, ConnectionAttachment>) {
    let finished = attachments
        .iter()
        .filter_map(|(id, attachment)| attachment.task.is_finished().then(|| id.clone()))
        .collect::<Vec<_>>();
    for id in finished {
        if let Some(attachment) = attachments.remove(&id) {
            shutdown_attachment(attachment).await;
        }
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
    let request: PairRequest = match decode_exact::<PairRequest>(bytes) {
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
    let bytes = encode_terminal_frame(value).map_err(|_| ())?;
    socket
        .send(Message::Binary(bytes.into()))
        .await
        .map_err(|_| ())
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
