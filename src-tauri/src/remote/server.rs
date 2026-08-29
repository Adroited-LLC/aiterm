use super::auth::{set_private_permissions, write_private_file, DeviceStore, PairingOutcome};
use super::model::{
    decode_exact, encode_terminal_frame, RemoteEvent, RemoteRequest, TerminalSize, PROTOCOL_VERSION,
};
use super::terminal::{
    plan_diff_for_attachment, plan_scrollback_for_attachment, plan_shared_snapshot_for_attachment,
    plan_snapshot_for_attachment, RemoteTerminal, RemoteTerminalEvents, TerminalEvent,
    TransferChunk, TransferPlan,
};
use crate::launch::LaunchRequest;
use crate::services::agents::AgentService;
use crate::services::sessions::SessionService;
use crate::tabs::{
    AttachmentId, RecoveryBoundary, TabAttachmentCancellation, TabDescriptor, TabId, TabLaunch,
    TabRegistry, TabRegistryEvent, TabState,
};
use crate::terminal::model::{Revision, ScreenDiff, ScreenSnapshot};
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
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
const MAX_IDENTIFIER_BYTES: usize = 4 * 1024;
const SESSION_OPEN_LOCK_STRIPES: usize = 64;
const ATTACHMENT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const ATTACHMENT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const ATTACHMENT_REAP_INTERVAL: Duration = Duration::from_millis(100);
const EGRESS_SEND_TIMEOUT: Duration = Duration::from_secs(1);
const EGRESS_CONTROL_QUEUE: usize = 64;
const EGRESS_TRANSFER_QUEUE: usize = 1;
const EGRESS_CONTROL_BURST: usize = 16;
const REMOTE_BLOCKING_OPERATIONS: usize = 32;
const REMOTE_OPERATION_TIMEOUT: Duration = Duration::from_secs(120);
const INBOUND_QUEUE: usize = 16;
const CLOSED_ATTACHMENT_CACHE: usize = 16;

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
    sessions: SessionService,
    agents: AgentService,
    blocking_operations: Arc<tokio::sync::Semaphore>,
    session_open_locks: Arc<Vec<Mutex<()>>>,
}

impl RemoteServices {
    pub fn new(registry: Arc<TabRegistry>) -> Self {
        static BLOCKING_OPERATIONS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
        Self {
            terminal: RemoteTerminal::new(registry.clone()),
            registry,
            sessions: SessionService::desktop(),
            agents: AgentService::desktop(),
            blocking_operations: BLOCKING_OPERATIONS
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(REMOTE_BLOCKING_OPERATIONS)))
                .clone(),
            session_open_locks: Arc::new(
                (0..SESSION_OPEN_LOCK_STRIPES)
                    .map(|_| Mutex::new(()))
                    .collect(),
            ),
        }
    }

    pub fn with_application_services(
        registry: Arc<TabRegistry>,
        sessions: SessionService,
        agents: AgentService,
    ) -> Self {
        let mut services = Self::new(registry);
        services.sessions = sessions;
        services.agents = agents;
        services
    }

    pub fn from_application_services(
        registry: Arc<TabRegistry>,
        services: &crate::services::ApplicationServices,
    ) -> Self {
        Self::with_application_services(
            registry,
            services.sessions.clone(),
            services.agents.clone(),
        )
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
        || state.devices.verify_proof(&proof.device_id, &nonce, &proof.signature_der).is_err()
    {
        let _ = send_cbor(&mut socket, &AuthReply { kind: "auth.denied" }).await;
        close_socket(&mut socket).await;
        return;
    }
    let revocations = state.devices.subscribe_revocations();
    if !state.devices.is_trusted(&proof.device_id) {
        let _ = send_cbor(&mut socket, &AuthReply { kind: "auth.denied" }).await;
        close_socket(&mut socket).await;
        return;
    }
    if send_cbor(&mut socket, &AuthReply { kind: "auth.ok" })
        .await
        .is_err()
    {
        return;
    }

    run_authenticated_socket(
        socket,
        state.services,
        state.devices,
        proof.device_id,
        revocations,
    )
    .await;
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
    transfers: AttachmentTransferState,
    lifecycle: AttachmentLifecycle,
}

struct StartedAttachment {
    tab_id: TabId,
    attachment_id: AttachmentId,
    events: RemoteTerminalEvents,
    cancellation: TabAttachmentCancellation,
    revision: Revision,
    transfers: AttachmentTransferState,
    lifecycle: AttachmentLifecycle,
}

const ATTACHMENT_ACTIVE: u8 = 0;
const ATTACHMENT_FINALIZING: u8 = 1;
const ATTACHMENT_FINALIZED: u8 = 2;

#[derive(Clone)]
struct AttachmentLifecycle(Arc<AtomicU8>);

impl AttachmentLifecycle {
    fn new() -> Self {
        Self(Arc::new(AtomicU8::new(ATTACHMENT_ACTIVE)))
    }

    fn is_active(&self) -> bool {
        self.0.load(Ordering::Acquire) == ATTACHMENT_ACTIVE
    }

    fn begin_finalizing(&self) {
        let _ = self.0.compare_exchange(
            ATTACHMENT_ACTIVE,
            ATTACHMENT_FINALIZING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn finish(&self) {
        self.0.store(ATTACHMENT_FINALIZED, Ordering::Release);
    }
}

#[derive(Clone)]
struct OwnedAttachment {
    tab_id: TabId,
    lifecycle: AttachmentLifecycle,
    transfers: AttachmentTransferState,
}

#[derive(Default)]
struct ClosedAttachments {
    order: VecDeque<AttachmentId>,
    entries: HashMap<AttachmentId, OwnedAttachment>,
}

impl ClosedAttachments {
    fn insert(
        &mut self,
        id: AttachmentId,
        tab_id: TabId,
        lifecycle: AttachmentLifecycle,
        transfers: AttachmentTransferState,
    ) {
        lifecycle.finish();
        if !self.entries.contains_key(&id) {
            self.order.push_back(id.clone());
        }
        self.entries.insert(
            id,
            OwnedAttachment {
                tab_id,
                lifecycle,
                transfers,
            },
        );
        while self.entries.len() > CLOSED_ATTACHMENT_CACHE {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

const TRANSFER_ACTIVE: u8 = 0;
const TRANSFER_CANCELLED: u8 = 1;
const TRANSFER_ATTACHMENT_CLOSED: u8 = 2;

#[derive(Clone)]
struct TransferToken(Arc<AtomicU8>);

impl TransferToken {
    fn new() -> Self {
        Self(Arc::new(AtomicU8::new(TRANSFER_ACTIVE)))
    }

    fn cancel(&self) {
        let _ = self.0.compare_exchange(
            TRANSFER_ACTIVE,
            TRANSFER_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn close_attachment(&self) {
        self.0.store(TRANSFER_ATTACHMENT_CLOSED, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) != TRANSFER_ACTIVE
    }

    fn error_code(&self) -> &'static str {
        if self.0.load(Ordering::Acquire) == TRANSFER_ATTACHMENT_CLOSED {
            "terminal.attachment_closed"
        } else {
            "terminal.transfer_cancelled"
        }
    }
}

#[derive(Clone)]
struct AttachmentTransferState(Arc<Mutex<TransferToken>>);

impl AttachmentTransferState {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(TransferToken::new())))
    }

    fn current(&self) -> TransferToken {
        self.0.lock().unwrap().clone()
    }

    fn authorize(&self, lifecycle: &AttachmentLifecycle) -> Option<TransferToken> {
        let current = self.0.lock().unwrap();
        lifecycle.is_active().then(|| current.clone())
    }

    fn begin_finalizing(&self, lifecycle: &AttachmentLifecycle) {
        let current = self.0.lock().unwrap();
        lifecycle.begin_finalizing();
        current.close_attachment();
    }

    fn replace(&self) -> TransferToken {
        let mut current = self.0.lock().unwrap();
        current.cancel();
        *current = TransferToken::new();
        current.clone()
    }

    fn replace_for_close(&self, lifecycle: &AttachmentLifecycle) -> TransferToken {
        let mut current = self.0.lock().unwrap();
        lifecycle.begin_finalizing();
        current.close_attachment();
        *current = TransferToken::new();
        current.clone()
    }

    fn cancel(&self) {
        self.0.lock().unwrap().cancel();
    }
}

#[derive(Clone)]
struct EgressHandle {
    controls: tokio::sync::mpsc::Sender<EgressControl>,
    transfers: tokio::sync::mpsc::Sender<TaggedTransfer>,
    transfer_slots: Arc<tokio::sync::Semaphore>,
}

struct TaggedTransfer {
    attachment_id: Option<AttachmentId>,
    plan: TransferPlan,
    token: Option<TransferToken>,
    trailer: Option<Vec<u8>>,
    completion: Option<tokio::sync::oneshot::Sender<Result<(), ()>>>,
    ingress_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

enum EgressControl {
    Message(Message),
    Resume {
        attachment_id: AttachmentId,
        reply: Vec<u8>,
        done: tokio::sync::oneshot::Sender<Result<(), ()>>,
    },
    Finalize {
        attachment_id: AttachmentId,
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
        request_id: u64,
        attachment_id: AttachmentId,
    },
}

struct DispatchOutcome {
    frames: Vec<RemoteEvent>,
    transfers: Vec<TransferPlan>,
    transfer_token: Option<TransferToken>,
    started: Option<StartedAttachment>,
    tab_id: Option<TabId>,
    sequenced: Option<SequencedAction>,
}

impl DispatchOutcome {
    fn frames(frames: Vec<RemoteEvent>) -> Self {
        Self {
            frames,
            transfers: Vec::new(),
            transfer_token: None,
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
#[serde(deny_unknown_fields)]
struct SessionIdPayload {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionOpenPayload {
    session_id: String,
    size: TerminalSize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionClosePayload {
    session_id: String,
    tab_id: Option<TabId>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum AgentActionPayload {
    Start {
        agent_id: String,
        model: Option<String>,
        effort: Option<String>,
        cwd: String,
        title: String,
        size: TerminalSize,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TabOpenPayload {
    Shell {
        #[serde(default)]
        project_path: Option<String>,
        #[serde(default)]
        title: Option<String>,
        size: TerminalSize,
    },
}

#[derive(Serialize)]
struct TabListPayload {
    tabs: Vec<RemoteTabDescriptor>,
}

#[derive(Serialize)]
struct SessionListPayload {
    sessions: Vec<crate::sessions::Session>,
}

#[derive(Serialize)]
struct SessionPreviewPayload {
    messages: Vec<crate::sessions::PreviewMsg>,
}

#[derive(Serialize)]
struct SessionOpenedPayload<'a> {
    tab_id: &'a TabId,
    selected_existing: bool,
}

#[derive(Serialize)]
struct SessionClosedPayload<'a> {
    tab_id: &'a TabId,
    ok: bool,
}

#[derive(Serialize)]
struct SessionForkedPayload {
    session_id: String,
}

#[derive(Serialize)]
struct AgentListPayload {
    agents: Vec<crate::agents::AgentChoice>,
    caps: std::collections::HashMap<String, crate::agents::Caps>,
}

#[derive(Serialize)]
struct AgentStartedPayload<'a> {
    tab_id: &'a TabId,
    session_id: Option<&'a str>,
}

#[derive(Serialize)]
struct StateSnapshotPayload {
    transfer_id: String,
    revision: u64,
    index: u32,
    total: u32,
    tabs: Vec<RemoteTabDescriptor>,
}

#[derive(Serialize)]
struct TabChangedPayload {
    revision: u64,
    change: &'static str,
    tab_id: TabId,
    tab: Option<RemoteTabDescriptor>,
    requested: Option<bool>,
}

#[derive(Clone, Serialize)]
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
    let event = RemoteEvent {
        version: PROTOCOL_VERSION,
        request_id,
        kind: kind.to_owned(),
        payload: payload(value)?,
    };
    encode_terminal_frame(&event).map_err(|error| {
        if error.code() == "protocol.frame_too_large" {
            "protocol.response_too_large"
        } else {
            "protocol.invalid_response"
        }
    })?;
    Ok(event)
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
    attachments: &HashMap<AttachmentId, OwnedAttachment>,
) -> RemoteTabDescriptor {
    let owns_input = descriptor.input_owner().is_some_and(|owner| {
        attachments.get(owner).is_some_and(|attachment| {
            attachment.lifecycle.is_active() && &attachment.tab_id == descriptor.id()
        })
    });
    project_remote_descriptor(descriptor, owns_input)
}

fn live_remote_descriptor(
    descriptor: TabDescriptor,
    attachments: &HashMap<AttachmentId, ConnectionAttachment>,
) -> RemoteTabDescriptor {
    let owns_input = descriptor.input_owner().is_some_and(|owner| {
        attachments.get(owner).is_some_and(|attachment| {
            attachment.lifecycle.is_active() && &attachment.tab_id == descriptor.id()
        })
    });
    project_remote_descriptor(descriptor, owns_input)
}

fn project_remote_descriptor(descriptor: TabDescriptor, owns_input: bool) -> RemoteTabDescriptor {
    let focus = match descriptor.input_owner() {
        None => RemoteFocusState::Unowned,
        Some(_) if owns_input => RemoteFocusState::Self_,
        Some(_) => RemoteFocusState::Other,
    };
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
        focus,
        state: descriptor.state().clone(),
        exit: descriptor.exit().cloned(),
    }
}

const MAX_REGISTRY_EVENT_BURST: usize = 8;
const MAX_ROSTER_TRANSFER_BYTES: usize =
    crate::tabs::MAX_TAB_ROSTER_ENTRIES * crate::tabs::MAX_TAB_DESCRIPTOR_TEXT_BYTES
        + super::terminal::MAX_WIRE_FRAME_BYTES;

fn state_snapshot_responses(
    revision: u64,
    tabs: Vec<TabDescriptor>,
    attachments: &HashMap<AttachmentId, ConnectionAttachment>,
) -> Result<Vec<RemoteEvent>, &'static str> {
    if tabs.len() > crate::tabs::MAX_TAB_ROSTER_ENTRIES {
        return Err("protocol.response_too_large");
    }
    let tabs = tabs
        .into_iter()
        .map(|tab| live_remote_descriptor(tab, attachments))
        .collect::<Vec<_>>();
    let mut resident_bytes = 0usize;
    for tab in &tabs {
        resident_bytes = resident_bytes
            .checked_add(payload(tab)?.len())
            .ok_or("protocol.response_too_large")?;
        if resident_bytes > MAX_ROSTER_TRANSFER_BYTES {
            return Err("protocol.response_too_large");
        }
    }

    let transfer_id = uuid::Uuid::new_v4().to_string();
    let mut groups = Vec::<Vec<RemoteTabDescriptor>>::new();
    let mut current = Vec::new();
    for tab in tabs {
        current.push(tab);
        let probe = StateSnapshotPayload {
            transfer_id: transfer_id.clone(),
            revision,
            index: u32::MAX,
            total: u32::MAX,
            tabs: current.clone(),
        };
        match response(0, "state.snapshot", &probe) {
            Ok(_) => {}
            Err("protocol.response_too_large") if current.len() > 1 => {
                let last = current.pop().expect("the candidate contains a final tab");
                groups.push(std::mem::take(&mut current));
                current.push(last);
                let single = StateSnapshotPayload {
                    transfer_id: transfer_id.clone(),
                    revision,
                    index: u32::MAX,
                    total: u32::MAX,
                    tabs: current.clone(),
                };
                response(0, "state.snapshot", &single)?;
            }
            Err(error) => return Err(error),
        }
    }
    if !current.is_empty() || groups.is_empty() {
        groups.push(current);
    }
    let total: u32 = groups
        .len()
        .try_into()
        .map_err(|_| "protocol.response_too_large")?;
    groups
        .into_iter()
        .enumerate()
        .map(|(index, tabs)| {
            response(
                0,
                "state.snapshot",
                &StateSnapshotPayload {
                    transfer_id: transfer_id.clone(),
                    revision,
                    index: index
                        .try_into()
                        .map_err(|_| "protocol.response_too_large")?,
                    total,
                    tabs,
                },
            )
        })
        .collect()
}

fn registry_event_responses(
    event: TabRegistryEvent,
    attachments: &HashMap<AttachmentId, ConnectionAttachment>,
) -> Result<Vec<RemoteEvent>, &'static str> {
    match event {
        TabRegistryEvent::Snapshot { revision, tabs } => {
            state_snapshot_responses(revision, tabs, attachments)
        }
        TabRegistryEvent::Opened { revision, tab } => {
            let tab_id = tab.id().clone();
            response(
                0,
                "tab.changed",
                &TabChangedPayload {
                    revision,
                    change: "opened",
                    tab_id,
                    tab: Some(live_remote_descriptor(tab, attachments)),
                    requested: None,
                },
            ).map(|event| vec![event])
        }
        TabRegistryEvent::Changed { revision, tab } => {
            let tab_id = tab.id().clone();
            response(
                0,
                "tab.changed",
                &TabChangedPayload {
                    revision,
                    change: "changed",
                    tab_id,
                    tab: Some(live_remote_descriptor(tab, attachments)),
                    requested: None,
                },
            ).map(|event| vec![event])
        }
        TabRegistryEvent::Removed {
            revision,
            tab_id,
            requested,
        } => response(
            0,
            "tab.changed",
            &TabChangedPayload {
                revision,
                change: "removed",
                tab_id,
                tab: None,
                requested: Some(requested),
            },
        ).map(|event| vec![event]),
    }
}

fn bounded(value: &str, max: usize) -> Result<(), &'static str> {
    if value.len() > max {
        Err("protocol.value_too_large")
    } else {
        Ok(())
    }
}

fn session_open_lock_index(session_id: &str) -> usize {
    session_id.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(16777619) ^ usize::from(byte)
    }) % SESSION_OPEN_LOCK_STRIPES
}

fn tab_matches_session(tab: &TabDescriptor, session_id: &str) -> bool {
    tab.session_id() == Some(session_id)
        || tab.resumed_id() == Some(session_id)
        || tab.slot_id() == session_id
}

fn launch_from_plan(
    title: String,
    cwd: String,
    slot_id: String,
    plan: crate::launch::LaunchPlan,
    size: TerminalSize,
) -> TabLaunch {
    let mut launch = TabLaunch::new(title, slot_id, size)
        .with_cwd(cwd)
        .with_command(plan.command)
        .with_agent_id(plan.agent_id);
    if let Some(session_id) = plan.session_id {
        launch = launch
            .with_session_id(session_id.clone())
            .with_resumed_id(session_id);
    }
    if let (Some(provider), Some(model)) = (plan.env_provider, plan.env_model) {
        launch = launch.with_environment(provider, model);
    }
    launch
}

impl RemoteServices {
    fn dispatch(
        &self,
        request: &RemoteRequest,
        attachments: &HashMap<AttachmentId, OwnedAttachment>,
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
        attachments: &HashMap<AttachmentId, OwnedAttachment>,
    ) -> Result<DispatchOutcome, &'static str> {
        let request_id = request.request_id();
        match request.kind() {
            "session.list" => {
                if !request.payload().is_empty() {
                    return Err("protocol.invalid_payload");
                }
                let sessions = self.sessions.list().map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "session.list",
                    &SessionListPayload { sessions },
                )?]))
            }
            "session.preview" => {
                let payload: SessionIdPayload = decode_payload(request)?;
                let messages = self
                    .sessions
                    .preview(&payload.session_id)
                    .map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "session.preview",
                    &SessionPreviewPayload { messages },
                )?]))
            }
            "session.open" => {
                let payload: SessionOpenPayload = decode_payload(request)?;
                self.sessions
                    .validate_id(&payload.session_id)
                    .map_err(|error| error.code())?;
                let stripe = session_open_lock_index(&payload.session_id);
                let _open_guard = self.session_open_locks[stripe]
                    .lock()
                    .map_err(|_| "remote.operation_failed")?;
                let matches: Vec<_> = self
                    .registry
                    .list()
                    .into_iter()
                    .filter(|tab| {
                        tab.state() == &TabState::Running
                            && tab_matches_session(tab, &payload.session_id)
                    })
                    .collect();
                if matches.len() > 1 {
                    return Err("session.tab_ambiguous");
                }
                let (tab_id, selected_existing) = if let Some(existing) = matches.first() {
                    (existing.id().clone(), true)
                } else {
                    let session = self
                        .sessions
                        .find(&payload.session_id)
                        .map_err(|error| error.code())?;
                    let plan = self
                        .agents
                        .resolve(LaunchRequest::Resume {
                            session_id: payload.session_id.clone(),
                        })
                        .map_err(|error| error.code())?;
                    let slot_id = if self
                        .registry
                        .list()
                        .iter()
                        .any(|tab| tab.slot_id() == payload.session_id)
                    {
                        format!("remote-resume:{}", uuid::Uuid::new_v4())
                    } else {
                        payload.session_id.clone()
                    };
                    let launch = launch_from_plan(
                        session.title,
                        session.project_path,
                        slot_id,
                        plan,
                        payload.size,
                    );
                    (
                        self.terminal.open(launch).map_err(|error| error.code())?,
                        false,
                    )
                };
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "session.open",
                    &SessionOpenedPayload {
                        tab_id: &tab_id,
                        selected_existing,
                    },
                )?]))
            }
            "session.close" => {
                let payload: SessionClosePayload = decode_payload(request)?;
                self.sessions
                    .validate_id(&payload.session_id)
                    .map_err(|error| error.code())?;
                let matches: Vec<_> = self
                    .registry
                    .list()
                    .into_iter()
                    .filter(|tab| {
                        tab.state() == &TabState::Running
                            && tab_matches_session(tab, &payload.session_id)
                    })
                    .collect();
                let tab_id = match payload.tab_id {
                    Some(tab_id) => matches
                        .iter()
                        .find(|tab| tab.id() == &tab_id)
                        .map(|tab| tab.id().clone())
                        .ok_or("session.tab_mismatch")?,
                    None if matches.is_empty() => return Err("session.tab_not_found"),
                    None if matches.len() > 1 => return Err("session.tab_ambiguous"),
                    None => matches[0].id().clone(),
                };
                self.terminal
                    .close(&tab_id)
                    .map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "session.close",
                    &SessionClosedPayload {
                        tab_id: &tab_id,
                        ok: true,
                    },
                )?]))
            }
            "session.delete" => {
                let payload: SessionIdPayload = decode_payload(request)?;
                self.sessions
                    .delete(&payload.session_id)
                    .map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "session.delete",
                    &SuccessPayload { ok: true },
                )?]))
            }
            "session.fork" => {
                let payload: SessionIdPayload = decode_payload(request)?;
                let session_id = self
                    .sessions
                    .fork(&payload.session_id)
                    .map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "session.fork",
                    &SessionForkedPayload { session_id },
                )?]))
            }
            "session.stop" => {
                let payload: SessionIdPayload = decode_payload(request)?;
                self.sessions
                    .stop(&payload.session_id)
                    .map_err(|error| error.code())?;
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "session.stop",
                    &SuccessPayload { ok: true },
                )?]))
            }
            "agent.list" => {
                if !request.payload().is_empty() {
                    return Err("protocol.invalid_payload");
                }
                Ok(DispatchOutcome::frames(vec![response(
                    request_id,
                    "agent.list",
                    &AgentListPayload {
                        agents: self.agents.list(),
                        caps: self.agents.caps(),
                    },
                )?]))
            }
            "agent.action" => {
                let payload: AgentActionPayload = decode_payload(request)?;
                match payload {
                    AgentActionPayload::Start {
                        agent_id,
                        model,
                        effort,
                        cwd,
                        title,
                        size,
                    } => {
                        bounded(&agent_id, MAX_IDENTIFIER_BYTES)?;
                        bounded(&cwd, MAX_PATH_BYTES)?;
                        bounded(&title, MAX_TITLE_BYTES)?;
                        for value in [model.as_deref(), effort.as_deref()].into_iter().flatten() {
                            bounded(value, MAX_IDENTIFIER_BYTES)?;
                        }
                        let cwd_is_exposed = self
                            .sessions
                            .list()
                            .map_err(|error| error.code())?
                            .iter()
                            .any(|session| session.project_path == cwd);
                        if !cwd_is_exposed {
                            return Err("remote.path_not_allowed");
                        }
                        let plan = self
                            .agents
                            .resolve(LaunchRequest::Agent {
                                agent_id,
                                model,
                                effort,
                            })
                            .map_err(|error| error.code())?;
                        let slot_id = plan
                            .session_id
                            .clone()
                            .unwrap_or_else(|| format!("agent:{}", uuid::Uuid::new_v4()));
                        let launch = launch_from_plan(title, cwd, slot_id, plan.clone(), size);
                        let tab_id = self
                            .terminal
                            .open(launch)
                            .map_err(|error| error.code())?;
                        Ok(DispatchOutcome::frames(vec![response(
                            request_id,
                            "agent.action",
                            &AgentStartedPayload {
                                tab_id: &tab_id,
                                session_id: plan.session_id.as_deref(),
                            },
                        )?]))
                    }
                }
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
                let payload = decode_payload::<TabOpenPayload>(request)?;
                let (project_path, title, size) = match payload {
                    TabOpenPayload::Shell {
                        project_path,
                        title,
                        size,
                    } => (
                        project_path,
                        title.unwrap_or_else(|| "Terminal".to_owned()),
                        size,
                    ),
                };
                bounded(&title, MAX_TITLE_BYTES)?;
                if let Some(path) = project_path.as_deref() {
                    bounded(path, MAX_PATH_BYTES)?;
                    let exposed = self
                        .sessions
                        .list()
                        .map_err(|error| error.code())?
                        .iter()
                        .any(|session| session.project_path == path);
                    if !exposed {
                        return Err("remote.path_not_allowed");
                    }
                }
                let mut launch = TabLaunch::new(
                    title,
                    format!("remote-shell:{}", uuid::Uuid::new_v4()),
                    size,
                );
                if let Some(project_path) = project_path {
                    launch = launch.with_cwd(project_path);
                }
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
                    transfer_token: None,
                    tab_id: Some(tab_id.clone()),
                    started: Some(StartedAttachment {
                        tab_id,
                        attachment_id,
                        cancellation: events.cancellation(),
                        revision,
                        events,
                        transfers: AttachmentTransferState::new(),
                        lifecycle: AttachmentLifecycle::new(),
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
                let result =
                    self.terminal
                        .input(&request.tab_id, &request.attachment_id, &request.data);
                authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                result.map_err(|error| error.code())?;
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
                authorize_attachment(attachments, &body.tab_id, &body.attachment_id)?;
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
                    transfer_token: None,
                    started: None,
                    tab_id: None,
                    sequenced: Some(SequencedAction::Detach {
                        request_id,
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
                let page =
                    self.registry
                        .scrollback_page(&request.tab_id, request.offset, request.count);
                authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                let page = page.map_err(|error| error.code())?;
                let transfer = plan_scrollback_for_attachment(
                    request_id,
                    &request.tab_id,
                    Some(&request.attachment_id),
                    page.revision(),
                    page.into_rows(),
                )
                .map_err(|error| error.code())?;
                let transfer_token =
                    authorize_attachment(attachments, &request.tab_id, &request.attachment_id)?;
                let mut outcome = DispatchOutcome::frames(Vec::new());
                outcome.transfers.push(transfer);
                outcome.transfer_token = Some(transfer_token);
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
            _ => Err("remote.unsupported"),
        }
    }
}

fn authorize_attachment(
    attachments: &HashMap<AttachmentId, OwnedAttachment>,
    tab_id: &TabId,
    attachment_id: &AttachmentId,
) -> Result<TransferToken, &'static str> {
    match attachments.get(attachment_id) {
        Some(attachment) if &attachment.tab_id != tab_id => Err("terminal.attachment_not_found"),
        Some(attachment) => attachment
            .transfers
            .authorize(&attachment.lifecycle)
            .ok_or("terminal.attachment_closed"),
        None => Err("terminal.attachment_not_found"),
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

async fn enqueue_transfer(
    outbound: &EgressHandle,
    transfer: TransferPlan,
    token: Option<TransferToken>,
) -> Result<(), ()> {
    let attachment_id = transfer.attachment_id().cloned();
    let ingress_permit = outbound
        .transfer_slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ())?;
    outbound
        .transfers
        .send(TaggedTransfer {
            attachment_id,
            plan: transfer,
            token,
            trailer: None,
            completion: None,
            ingress_permit: Some(ingress_permit),
        })
        .await
        .map_err(|_| ())
}

async fn enqueue_outcome(
    outbound: &EgressHandle,
    frames: Vec<RemoteEvent>,
    transfers: Vec<TransferPlan>,
    token: Option<TransferToken>,
) -> Result<(), ()> {
    for frame in frames {
        enqueue_event(outbound, frame).await?;
    }
    for transfer in transfers {
        enqueue_transfer(outbound, transfer, token.clone()).await?;
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

async fn finish_tagged_transfer(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    transfer: TaggedTransfer,
) -> Result<(), ()> {
    let terminal = transfer.trailer.is_some();
    let sent = match transfer.trailer {
        Some(trailer) => send_egress(sink, Message::Binary(trailer.into())).await,
        None => Ok(()),
    };
    if terminal {
        if let Some(token) = transfer.token {
            token.cancel();
        }
    }
    let failed = sent.is_err();
    if let Some(completion) = transfer.completion {
        let _ = completion.send(sent);
    }
    if failed {
        Err(())
    } else {
        Ok(())
    }
}

fn cancel_attachment_transfers(
    active: &mut Option<TaggedTransfer>,
    pending: &mut Option<TaggedTransfer>,
    ingress: &mut tokio::sync::mpsc::Receiver<TaggedTransfer>,
    ingress_sender: &tokio::sync::mpsc::Sender<TaggedTransfer>,
    attachment_id: &AttachmentId,
) -> Vec<TaggedTransfer> {
    let mut cancelled = Vec::new();
    if active
        .as_ref()
        .and_then(|current| current.attachment_id.as_ref())
        == Some(attachment_id)
    {
        cancelled.extend(active.take());
    }
    if pending
        .as_ref()
        .and_then(|current| current.attachment_id.as_ref())
        == Some(attachment_id)
    {
        cancelled.extend(pending.take());
    }
    if active.is_none() {
        *active = pending.take();
    }

    // A producer can have one already-cancelled plan sitting in the
    // capacity-one ingress while both local slots are occupied. Drain that
    // slot before acknowledging the barrier so its replacement cannot wait
    // behind the work it invalidated. A live sibling plan is immediately put
    // back unless one of the two local slots is available.
    if let Ok(mut transfer) = ingress.try_recv() {
        if transfer
            .token
            .as_ref()
            .is_some_and(TransferToken::is_cancelled)
        {
            cancelled.push(transfer);
            return cancelled;
        }
        if active.is_none() {
            transfer.ingress_permit.take();
            *active = Some(transfer);
        } else if pending.is_none() {
            transfer.ingress_permit.take();
            *pending = Some(transfer);
        } else {
            ingress_sender
                .try_send(transfer)
                .expect("the retained ingress permit keeps its slot vacant");
        }
    }
    cancelled
}

async fn report_cancelled_transfers(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    transfers: Vec<TaggedTransfer>,
) -> Result<(), ()> {
    for transfer in transfers {
        if transfer.plan.request_id() != 0 {
            let code = transfer
                .token
                .as_ref()
                .map(TransferToken::error_code)
                .unwrap_or("terminal.transfer_cancelled");
            let event = error_response(
                transfer.plan.request_id(),
                code,
                "the correlated terminal transfer was cancelled",
            );
            let bytes = encode_event(event).await?;
            send_egress(sink, Message::Binary(bytes.into())).await?;
        }
        if let Some(completion) = transfer.completion {
            let _ = completion.send(Err(()));
        }
    }
    Ok(())
}

async fn egress_arbiter(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut controls: tokio::sync::mpsc::Receiver<EgressControl>,
    mut transfers: tokio::sync::mpsc::Receiver<TaggedTransfer>,
    transfer_sender: tokio::sync::mpsc::Sender<TaggedTransfer>,
    failed: tokio::sync::watch::Sender<bool>,
) {
    let mut active: Option<TaggedTransfer> = None;
    let mut pending: Option<TaggedTransfer> = None;
    let result = async {
        loop {
            for _ in 0..EGRESS_CONTROL_BURST {
                let Ok(command) = controls.try_recv() else {
                    break;
                };
                match command {
                    EgressControl::Message(message) => send_egress(&mut sink, message).await?,
                    EgressControl::Resume {
                        attachment_id,
                        reply,
                        done,
                    } => {
                        let cancelled = cancel_attachment_transfers(
                            &mut active,
                            &mut pending,
                            &mut transfers,
                            &transfer_sender,
                            &attachment_id,
                        );
                        report_cancelled_transfers(&mut sink, cancelled).await?;
                        let sent = send_egress(&mut sink, Message::Binary(reply.into())).await;
                        let send_failed = sent.is_err();
                        let _ = done.send(sent);
                        if send_failed {
                            return Err(());
                        }
                    }
                    EgressControl::Finalize {
                        attachment_id,
                        done,
                    } => {
                        let cancelled = cancel_attachment_transfers(
                            &mut active,
                            &mut pending,
                            &mut transfers,
                            &transfer_sender,
                            &attachment_id,
                        );
                        report_cancelled_transfers(&mut sink, cancelled).await?;
                        let _ = done.send(Ok(()));
                    }
                    EgressControl::Detach {
                        attachment_id,
                        reply,
                        done,
                    } => {
                        let cancelled = cancel_attachment_transfers(
                            &mut active,
                            &mut pending,
                            &mut transfers,
                            &transfer_sender,
                            &attachment_id,
                        );
                        report_cancelled_transfers(&mut sink, cancelled).await?;
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
            if active.is_none() {
                active = pending.take();
            }
            if pending.is_none() {
                while let Ok(mut transfer) = transfers.try_recv() {
                    if transfer.token.as_ref().is_some_and(TransferToken::is_cancelled) {
                        report_cancelled_transfers(&mut sink, vec![transfer]).await?;
                        continue;
                    }
                    transfer.ingress_permit.take();
                    if active.is_none() {
                        active = Some(transfer);
                    } else {
                        pending = Some(transfer);
                        break;
                    }
                }
            }
            if let Some(mut current) = active.take() {
                if current
                    .token
                    .as_ref()
                    .is_some_and(TransferToken::is_cancelled)
                {
                    report_cancelled_transfers(&mut sink, vec![current]).await?;
                    continue;
                }
                let (returned, encoded) = tokio::task::spawn_blocking(move || {
                    let encoded = match current.plan.next_chunk() {
                        Ok(Some(chunk)) => {
                            let final_chunk = chunk.index + 1 == chunk.total;
                            encode_terminal_frame(&chunk_event(chunk))
                                .map(|bytes| Some((bytes, final_chunk)))
                                .map_err(|_| ())
                        }
                        Ok(None) => Ok(None),
                        Err(_) => Err(()),
                    };
                    (current, encoded)
                })
                .await
                .map_err(|_| ())?;
                match encoded? {
                    Some((bytes, final_chunk)) => {
                        send_egress(&mut sink, Message::Binary(bytes.into())).await?;
                        if final_chunk {
                            // A terminal trailer is part of the same semantic
                            // transfer batch: once the last chunk is on the
                            // wire, no later control can overtake its exit.
                            finish_tagged_transfer(&mut sink, returned).await?;
                        } else {
                            active = Some(returned);
                        }
                    }
                    None => finish_tagged_transfer(&mut sink, returned).await?,
                }
                continue;
            }
            tokio::select! {
                biased;
                command = controls.recv() => match command {
                    Some(command) => {
                        match command {
                            EgressControl::Message(message) => send_egress(&mut sink, message).await?,
                            EgressControl::Close => {
                                let _ = send_egress(&mut sink, Message::Close(None)).await;
                                return Ok::<(), ()>(());
                            }
                            command => {
                                // No transfer is active, so put the bounded
                                // barrier command through the same handler on
                                // the next iteration.
                                match command {
                                    EgressControl::Resume { attachment_id, reply, done } => {
                                        let cancelled = cancel_attachment_transfers(
                                            &mut active,
                                            &mut pending,
                                            &mut transfers,
                                            &transfer_sender,
                                            &attachment_id,
                                        );
                                        report_cancelled_transfers(&mut sink, cancelled).await?;
                                        let sent = send_egress(&mut sink, Message::Binary(reply.into())).await;
                                        let failed = sent.is_err();
                                        let _ = done.send(sent);
                                        if failed { return Err(()); }
                                    }
                                    EgressControl::Finalize { attachment_id, done } => {
                                        let cancelled = cancel_attachment_transfers(
                                            &mut active,
                                            &mut pending,
                                            &mut transfers,
                                            &transfer_sender,
                                            &attachment_id,
                                        );
                                        report_cancelled_transfers(&mut sink, cancelled).await?;
                                        let _ = done.send(Ok(()));
                                    }
                                    EgressControl::Detach { attachment_id, reply, done } => {
                                        let cancelled = cancel_attachment_transfers(
                                            &mut active,
                                            &mut pending,
                                            &mut transfers,
                                            &transfer_sender,
                                            &attachment_id,
                                        );
                                        report_cancelled_transfers(&mut sink, cancelled).await?;
                                        let sent = send_egress(&mut sink, Message::Binary(reply.into())).await;
                                        let failed = sent.is_err();
                                        let _ = done.send(sent);
                                        if failed { return Err(()); }
                                    }
                                    _ => unreachable!(),
                                }
                            }
                        }
                    },
                    None if transfers.is_closed() => return Ok(()),
                    None => {}
                },
                transfer = transfers.recv(), if pending.is_none() => match transfer {
                    Some(transfer) if transfer.token.as_ref().is_some_and(TransferToken::is_cancelled) => {
                        report_cancelled_transfers(&mut sink, vec![transfer]).await?;
                    },
                    Some(mut transfer) => {
                        transfer.ingress_permit.take();
                        active = Some(transfer);
                    },
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
        TerminalEvent::Snapshot(_) | TerminalEvent::Finalized { .. } | TerminalEvent::Diff(_) => {
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

enum ActorWorkCompletion {
    Screen {
        revision: Revision,
        result: Result<(), ()>,
    },
    ResumePlanned {
        emission: Result<ResumeEmission, RemoteEvent>,
        token: TransferToken,
        done: tokio::sync::oneshot::Sender<Result<(), ()>>,
    },
    ResumeFinished {
        revision: Revision,
        result: Result<(), ()>,
    },
    DetachFinished,
    Finalized,
}

enum ScreenWork {
    Snapshot(ScreenSnapshot),
    Diff(ScreenDiff),
}

fn spawn_screen_work(
    work: ScreenWork,
    tab_id: TabId,
    attachment_id: AttachmentId,
    token: TransferToken,
    outbound: EgressHandle,
    services: RemoteServices,
    mut cancellation: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<ActorWorkCompletion> {
    tokio::spawn(async move {
        let revision = match &work {
            ScreenWork::Snapshot(snapshot) => snapshot.revision(),
            ScreenWork::Diff(diff) => diff.revision(),
        };
        let planned = remote_blocking(&services, &mut cancellation, move || match work {
            ScreenWork::Snapshot(snapshot) => {
                plan_snapshot_for_attachment(0, &tab_id, Some(&attachment_id), snapshot)
            }
            ScreenWork::Diff(diff) => {
                plan_diff_for_attachment(0, &tab_id, Some(&attachment_id), diff)
            }
        })
        .await;
        let result = match planned {
            Ok(Ok(transfer)) => enqueue_transfer(&outbound, transfer, Some(token)).await,
            _ => Err(()),
        };
        ActorWorkCompletion::Screen { revision, result }
    })
}

async fn abort_actor_work(pending: &mut Option<tokio::task::JoinHandle<ActorWorkCompletion>>) {
    if let Some(task) = pending.take() {
        task.abort();
        let _ = task.await;
    }
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
    services: RemoteServices,
    mut cancellation: tokio::sync::watch::Receiver<bool>,
    completed_attachments: tokio::sync::mpsc::Sender<AttachmentId>,
) {
    let mut live_revision = Some(attachment.revision);
    let finalization_signal = attachment.events.finalization_signal();
    let mut finalization_announced = false;
    let mut pending = None::<tokio::task::JoinHandle<ActorWorkCompletion>>;
    let mut pending_blocks_events = false;
    let mut deferred_screen = None::<TerminalEvent>;
    let mut recovery_needed = false;
    let mut events_closed = false;

    loop {
        if pending.is_none() {
            if recovery_needed {
                live_revision = None;
                deferred_screen = None;
                recovery_needed = false;
                if enqueue_event(&outbound, recovery_error()).await.is_err() {
                    return;
                }
            } else if let Some(event) = deferred_screen.take() {
                match event {
                    TerminalEvent::Snapshot(snapshot) => {
                        pending = Some(spawn_screen_work(
                            ScreenWork::Snapshot(snapshot),
                            attachment.tab_id.clone(),
                            attachment.attachment_id.clone(),
                            attachment.transfers.current(),
                            outbound.clone(),
                            services.clone(),
                            cancellation.clone(),
                        ));
                        pending_blocks_events = false;
                    }
                    TerminalEvent::Diff(diff) if live_revision == Some(diff.base_revision()) => {
                        pending = Some(spawn_screen_work(
                            ScreenWork::Diff(diff),
                            attachment.tab_id.clone(),
                            attachment.attachment_id.clone(),
                            attachment.transfers.current(),
                            outbound.clone(),
                            services.clone(),
                            cancellation.clone(),
                        ));
                        pending_blocks_events = false;
                    }
                    TerminalEvent::Diff(_) => {
                        recovery_needed = true;
                        continue;
                    }
                    _ => unreachable!("only screen events are deferred"),
                }
            }
        }

        tokio::select! {
            biased;
            changed = cancellation.changed() => {
                let _ = changed;
                abort_actor_work(&mut pending).await;
                attachment.events.cancellation().close_mailbox();
                attachment.transfers.cancel();
                attachment.lifecycle.finish();
                return;
            }
            finalized = finalization_signal.wait(), if !finalization_announced && !events_closed => {
                if finalized {
                    attachment
                        .transfers
                        .begin_finalizing(&attachment.lifecycle);
                    abort_actor_work(&mut pending).await;
                    pending_blocks_events = false;
                    deferred_screen = None;
                    recovery_needed = false;
                    finalization_announced = true;
                } else {
                    // An explicit mailbox close is not a natural exit and has
                    // no final snapshot to drain. Stop polling the permanently
                    // ready signal so Shutdown or pending cleanup can run.
                    events_closed = true;
                }
            }
            command = commands.recv() => match command {
                Some(AttachmentCommand::Resume {
                    request_id,
                    tab_id,
                    attachment_id,
                    requested_revision,
                    done,
                }) => {
                    if !attachment.lifecycle.is_active() {
                        let _ = done.send(Err(()));
                        continue;
                    }
                    abort_actor_work(&mut pending).await;
                    deferred_screen = None;
                    recovery_needed = false;
                    let token = attachment.transfers.replace();
                    let resume_terminal = services.terminal.clone();
                    let planning_services = services.clone();
                    let mut planning_cancellation = cancellation.clone();
                    pending = Some(tokio::spawn(async move {
                        let emission = remote_blocking(
                            &planning_services,
                            &mut planning_cancellation,
                            move || build_resume_emission(
                                resume_terminal,
                                request_id,
                                tab_id,
                                attachment_id,
                                requested_revision,
                            ),
                        )
                        .await
                        .unwrap_or_else(|_| {
                            Err(error_response(
                                request_id,
                                "terminal.transfer_cancelled",
                                "terminal recovery planning was cancelled",
                            ))
                        });
                        ActorWorkCompletion::ResumePlanned {
                            emission,
                            token,
                            done,
                        }
                    }));
                    pending_blocks_events = true;
                }
                Some(AttachmentCommand::Detach { frames, done }) => {
                    if !attachment.lifecycle.is_active() {
                        let _ = done.send(Err(()));
                        continue;
                    }
                    abort_actor_work(&mut pending).await;
                    deferred_screen = None;
                    recovery_needed = false;
                    events_closed = true;
                    attachment
                        .transfers
                        .begin_finalizing(&attachment.lifecycle);
                    let registry_cancellation = attachment.events.cancellation();
                    registry_cancellation.close_mailbox();
                    let detach_outbound = outbound.clone();
                    let detach_id = attachment.attachment_id.clone();
                    let lifecycle = attachment.lifecycle.clone();
                    pending = Some(tokio::spawn(async move {
                        detach_attachment_bounded(registry_cancellation).await;
                        let result = match frames.into_iter().next() {
                            Some(frame) => match encode_event(frame).await {
                                Ok(reply) => {
                                    let (sent, completed) = tokio::sync::oneshot::channel();
                                    if detach_outbound.controls.send(EgressControl::Detach {
                                        attachment_id: detach_id,
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
                        lifecycle.finish();
                        let failed = result.is_err();
                        let _ = done.send(result);
                        let _ = failed;
                        ActorWorkCompletion::DetachFinished
                    }));
                    pending_blocks_events = true;
                }
                Some(AttachmentCommand::Shutdown) | None => {
                    abort_actor_work(&mut pending).await;
                    let registry_cancellation = attachment.events.cancellation();
                    registry_cancellation.close_mailbox();
                    attachment.transfers.cancel();
                    attachment.lifecycle.finish();
                    detach_attachment_bounded(registry_cancellation).await;
                    return;
                }
            },
            event = attachment.events.next(), if !events_closed && !pending_blocks_events => {
                let Some(event) = event else {
                    events_closed = true;
                    if pending.is_none() { return; }
                    continue;
                };
                match event {
                    TerminalEvent::Finalized { snapshot, exit } => {
                        abort_actor_work(&mut pending).await;
                        deferred_screen = None;
                        recovery_needed = false;
                        events_closed = true;
                        let token = attachment
                            .transfers
                            .replace_for_close(&attachment.lifecycle);
                        let final_tab = attachment.tab_id.clone();
                        let final_attachment = attachment.attachment_id.clone();
                        let final_outbound = outbound.clone();
                        let final_services = services.clone();
                        let lifecycle = attachment.lifecycle.clone();
                        let actor_completed = completed_attachments.clone();
                        let completion_id = final_attachment.clone();
                        let mut final_cancellation = cancellation.clone();
                        pending = Some(tokio::spawn(async move {
                            let plan_tab = final_tab.clone();
                            let plan_attachment = final_attachment.clone();
                            let planned = remote_blocking(
                                &final_services,
                                &mut final_cancellation,
                                move || plan_shared_snapshot_for_attachment(
                                    0,
                                    &plan_tab,
                                    Some(&plan_attachment),
                                    snapshot,
                                ),
                            ).await;
                            let result = match planned {
                                Ok(Ok(transfer)) => async {
                                    let trailer = encode_control_event(
                                        final_tab,
                                        final_attachment.clone(),
                                        TerminalEvent::Exited(exit),
                                    ).await?.ok_or(())?;
                                    let trailer = encode_event(trailer).await?;
                                    let (done, completed) = tokio::sync::oneshot::channel();
                                    final_outbound.controls.send(EgressControl::Finalize {
                                        attachment_id: final_attachment.clone(),
                                        done,
                                    }).await.map_err(|_| ())?;
                                    completed.await.unwrap_or(Err(()))?;
                                    let ingress_permit = final_outbound.transfer_slots.clone()
                                        .acquire_owned().await.map_err(|_| ())?;
                                    let (trailer_done, trailer_completed) =
                                        tokio::sync::oneshot::channel();
                                    final_outbound.transfers.send(TaggedTransfer {
                                        attachment_id: Some(final_attachment),
                                        plan: transfer,
                                        token: Some(token),
                                        trailer: Some(trailer),
                                        completion: Some(trailer_done),
                                        ingress_permit: Some(ingress_permit),
                                    }).await.map_err(|_| ())?;
                                    trailer_completed.await.unwrap_or(Err(()))
                                }.await,
                                _ => Err(()),
                            };
                            lifecycle.finish();
                            if result.is_ok() {
                                let _ = actor_completed.try_send(completion_id);
                            }
                            let _ = result;
                            ActorWorkCompletion::Finalized
                        }));
                        pending_blocks_events = true;
                    }
                    TerminalEvent::Snapshot(_) | TerminalEvent::Diff(_) if pending.is_some() => {
                        if deferred_screen.is_none() && !recovery_needed {
                            deferred_screen = Some(event);
                        } else {
                            deferred_screen = None;
                            recovery_needed = true;
                        }
                    }
                    TerminalEvent::Snapshot(snapshot) => {
                        pending = Some(spawn_screen_work(
                            ScreenWork::Snapshot(snapshot),
                            attachment.tab_id.clone(),
                            attachment.attachment_id.clone(),
                            attachment.transfers.current(),
                            outbound.clone(),
                            services.clone(),
                            cancellation.clone(),
                        ));
                        pending_blocks_events = false;
                    }
                    TerminalEvent::Diff(diff) => {
                        if live_revision != Some(diff.base_revision()) {
                            live_revision = None;
                            if enqueue_event(&outbound, recovery_error()).await.is_err() { return; }
                        } else {
                            pending = Some(spawn_screen_work(
                                ScreenWork::Diff(diff),
                                attachment.tab_id.clone(),
                                attachment.attachment_id.clone(),
                                attachment.transfers.current(),
                                outbound.clone(),
                                services.clone(),
                                cancellation.clone(),
                            ));
                            pending_blocks_events = false;
                        }
                    }
                    control => match encode_control_event(
                        attachment.tab_id.clone(),
                        attachment.attachment_id.clone(),
                        control,
                    ).await {
                        Ok(Some(event)) => {
                            if enqueue_event(&outbound, event).await.is_err() { return; }
                        }
                        Ok(None) => {}
                        Err(()) => return,
                    },
                }
            }
            completed = async {
                pending.as_mut().expect("guarded pending actor work").await
            }, if pending.is_some() => {
                pending = None;
                pending_blocks_events = false;
                let Ok(completed) = completed else {
                    recovery_needed = true;
                    continue;
                };
                match completed {
                    ActorWorkCompletion::Screen { revision, result } => {
                        if result.is_ok() {
                            live_revision = Some(revision);
                        } else {
                            recovery_needed = true;
                        }
                    }
                    ActorWorkCompletion::ResumePlanned { emission, token, done } => {
                        match emission {
                            Ok(emission) => {
                                attachment.events.apply_recovery_boundary(emission.boundary);
                                live_revision = Some(emission.revision);
                                deferred_screen = None;
                                recovery_needed = false;
                                let revision = emission.revision;
                                let mut frames = emission.frames;
                                let mut transfers = emission.transfers;
                                let resume_outbound = outbound.clone();
                                let resume_id = attachment.attachment_id.clone();
                                pending = Some(tokio::spawn(async move {
                                    let result = match (frames.pop(), transfers.pop()) {
                                        (Some(frame), Some(transfer)) => match encode_event(frame).await {
                                            Ok(reply) => {
                                                let (sent, completed) = tokio::sync::oneshot::channel();
                                                if resume_outbound.controls.send(EgressControl::Resume {
                                                    attachment_id: resume_id,
                                                    reply,
                                                    done: sent,
                                                }).await.is_err() {
                                                    Err(())
                                                } else if completed.await.unwrap_or(Err(())).is_err() {
                                                    Err(())
                                                } else {
                                                    enqueue_transfer(&resume_outbound, transfer, Some(token)).await
                                                }
                                            }
                                            Err(()) => Err(()),
                                        },
                                        _ => Err(()),
                                    };
                                    let failed = result.is_err();
                                    let _ = done.send(result);
                                    ActorWorkCompletion::ResumeFinished {
                                        revision,
                                        result: if failed { Err(()) } else { Ok(()) },
                                    }
                                }));
                                pending_blocks_events = true;
                            }
                            Err(error) => {
                                let result = enqueue_event(&outbound, error).await;
                                let failed = result.is_err();
                                let _ = done.send(result);
                                if failed { return; }
                            }
                        }
                    }
                    ActorWorkCompletion::ResumeFinished { revision, result } => {
                        if result.is_ok() {
                            live_revision = Some(revision);
                        } else {
                            recovery_needed = true;
                        }
                    }
                    ActorWorkCompletion::DetachFinished => return,
                    ActorWorkCompletion::Finalized => return,
                }
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

enum InboundMessage {
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
}

enum AttachmentCommandOutcome {
    Completed(Result<(), ()>),
    ConnectionCancelled,
    Unavailable,
}

async fn run_attachment_command(
    commands: &tokio::sync::mpsc::Sender<AttachmentCommand>,
    command: AttachmentCommand,
    completed: tokio::sync::oneshot::Receiver<Result<(), ()>>,
    cancellation: &mut tokio::sync::watch::Receiver<bool>,
) -> AttachmentCommandOutcome {
    if *cancellation.borrow() {
        return AttachmentCommandOutcome::ConnectionCancelled;
    }
    let send_deadline = tokio::time::sleep(ATTACHMENT_COMMAND_TIMEOUT);
    tokio::pin!(send_deadline);
    tokio::select! {
        biased;
        changed = cancellation.changed() => {
            let _ = changed;
            return AttachmentCommandOutcome::ConnectionCancelled;
        }
        result = commands.send(command) => {
            if result.is_err() {
                return AttachmentCommandOutcome::Unavailable;
            }
        }
        _ = &mut send_deadline => return AttachmentCommandOutcome::Unavailable,
    }

    let completion_deadline = tokio::time::sleep(ATTACHMENT_COMMAND_TIMEOUT);
    tokio::pin!(completion_deadline);
    tokio::select! {
        biased;
        changed = cancellation.changed() => {
            let _ = changed;
            AttachmentCommandOutcome::ConnectionCancelled
        }
        result = completed => match result {
            Ok(result) => AttachmentCommandOutcome::Completed(result),
            Err(_) => AttachmentCommandOutcome::Unavailable,
        },
        _ = &mut completion_deadline => AttachmentCommandOutcome::Unavailable,
    }
}

fn close_unavailable_attachment(attachment: &ConnectionAttachment) {
    attachment.transfers.begin_finalizing(&attachment.lifecycle);
    attachment.cancellation.close_mailbox();
    let _ = attachment.commands.try_send(AttachmentCommand::Shutdown);
}

async fn socket_reader(
    mut stream: futures_util::stream::SplitStream<WebSocket>,
    inbound: tokio::sync::mpsc::Sender<InboundMessage>,
    cancelled: tokio::sync::watch::Sender<bool>,
) {
    let mut cancellation = cancelled.subscribe();
    loop {
        let message = tokio::select! {
            biased;
            changed = cancellation.changed() => {
                if changed.is_err() || *cancellation.borrow() { return; }
                continue;
            }
            message = stream.next() => message,
        };
        let Some(message) = message else {
            let _ = cancelled.send(true);
            return;
        };
        let inbound_message = match message {
            Ok(Message::Binary(bytes)) if bytes.len() < MAX_MESSAGE_SIZE => {
                InboundMessage::Binary(bytes.to_vec())
            }
            Ok(Message::Ping(bytes)) if bytes.len() < MAX_MESSAGE_SIZE => {
                InboundMessage::Ping(bytes.to_vec())
            }
            Ok(Message::Pong(_)) => InboundMessage::Pong,
            Ok(Message::Close(_)) | Err(_) => {
                let _ = cancelled.send(true);
                return;
            }
            _ => {
                let _ = cancelled.send(true);
                return;
            }
        };
        if inbound.try_send(inbound_message).is_err() {
            let _ = cancelled.send(true);
            return;
        }
    }
}

async fn remote_blocking<T, F>(
    services: &RemoteServices,
    cancellation: &mut tokio::sync::watch::Receiver<bool>,
    operation: F,
) -> Result<T, ()>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if *cancellation.borrow() {
        return Err(());
    }
    let permit = tokio::select! {
        biased;
        changed = cancellation.changed() => {
            let _ = changed;
            return Err(());
        }
        permit = services.blocking_operations.clone().acquire_owned() => {
            permit.map_err(|_| ())?
        }
        _ = tokio::time::sleep(REMOTE_OPERATION_TIMEOUT) => return Err(()),
    };
    let mut task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    });
    tokio::select! {
        biased;
        changed = cancellation.changed() => {
            let _ = changed;
            task.abort();
            Err(())
        }
        result = &mut task => result.map_err(|_| ()),
        _ = tokio::time::sleep(REMOTE_OPERATION_TIMEOUT) => {
            task.abort();
            Err(())
        }
    }
}

async fn run_authenticated_socket(
    socket: WebSocket,
    services: RemoteServices,
    devices: Arc<DeviceStore>,
    device_id: String,
    mut revocations: tokio::sync::watch::Receiver<u64>,
) {
    let mut guard = RequestGuard::new(Instant::now());
    let (socket_sink, socket_stream) = socket.split();
    let (cancelled, mut cancellation) = tokio::sync::watch::channel(false);
    let (inbound, mut inbound_messages) = tokio::sync::mpsc::channel(INBOUND_QUEUE);
    let mut reader = tokio::spawn(socket_reader(socket_stream, inbound, cancelled.clone()));
    let (controls, control_receiver) = tokio::sync::mpsc::channel(EGRESS_CONTROL_QUEUE);
    let (transfers, transfer_receiver) = tokio::sync::mpsc::channel(EGRESS_TRANSFER_QUEUE);
    let transfer_slots = Arc::new(tokio::sync::Semaphore::new(EGRESS_TRANSFER_QUEUE));
    let outbound = EgressHandle {
        controls,
        transfers,
        transfer_slots,
    };
    let mut writer = tokio::spawn(egress_arbiter(
        socket_sink,
        control_receiver,
        transfer_receiver,
        outbound.transfers.clone(),
        cancelled.clone(),
    ));
    let mut attachments = HashMap::<AttachmentId, ConnectionAttachment>::new();
    let registry_events = services.registry.subscribe_changes();
    let mut closed_attachments = ClosedAttachments::default();
    let (attachment_completed, mut completed_attachments) =
        tokio::sync::mpsc::channel(MAX_ATTACHMENTS_PER_CONNECTION);
    let mut reap_tick = tokio::time::interval(ATTACHMENT_REAP_INTERVAL);
    reap_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut registry_event_burst = 0usize;
    'socket: loop {
        reap_finished_attachments(&mut attachments, &mut closed_attachments).await;
        let message = tokio::select! {
            biased;
            changed = revocations.changed() => {
                if changed.is_err() || !devices.is_trusted(&device_id) {
                    if let Ok(event) = response(0, "auth.revoked", &()) {
                        let _ = enqueue_event(&outbound, event).await;
                    }
                    break;
                }
                continue;
            }
            completed = completed_attachments.recv() => {
                if let Some(id) = completed {
                    if let Some(attachment) = attachments.remove(&id) {
                        let tab_id = attachment.tab_id.clone();
                        let lifecycle = attachment.lifecycle.clone();
                        let transfers = attachment.transfers.clone();
                        shutdown_attachment(attachment).await;
                        closed_attachments.insert(id, tab_id, lifecycle, transfers);
                    }
                }
                continue;
            }
            registry_event = registry_events.recv_async(), if registry_event_burst < MAX_REGISTRY_EVENT_BURST => {
                let Some(registry_event) = registry_event else { break; };
                let Ok(events) = registry_event_responses(registry_event, &attachments) else {
                    let _ = cancelled.send(true);
                    let _ = outbound.controls.try_send(EgressControl::Close);
                    break;
                };
                registry_event_burst = registry_event_burst.saturating_add(events.len());
                for event in events {
                    if enqueue_event(&outbound, event).await.is_err() {
                        break 'socket;
                    }
                }
                continue;
            }
            message = inbound_messages.recv() => {
                registry_event_burst = 0;
                message
            },
            changed = cancellation.changed() => {
                if changed.is_err() || *cancellation.borrow() { break; }
                continue;
            }
            _ = reap_tick.tick() => continue,
        };
        let Some(message) = message else {
            break;
        };
        match message {
            InboundMessage::Binary(bytes) => {
                let request = match RemoteRequest::decode(&bytes) {
                    Ok(request) => request,
                    Err(error) => {
                        let Some(request_id) = error.request_id() else {
                            let _ = cancelled.send(true);
                            let _ = outbound.controls.try_send(EgressControl::Close);
                            break;
                        };
                        if guard.admit(request_id, Instant::now()).is_err() {
                            let _ = cancelled.send(true);
                            let _ = outbound.controls.try_send(EgressControl::Close);
                            break;
                        }
                        let (code, message) = if error.code() == "protocol.unknown_request" {
                            (
                                "remote.unsupported",
                                "this operation is not available through the remote gateway",
                            )
                        } else {
                            (error.code(), error.message())
                        };
                        if enqueue_event(
                            &outbound,
                            error_response(request_id, code, message),
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
                    let _ = cancelled.send(true);
                    let _ = outbound.controls.try_send(EgressControl::Close);
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
                let mut owned = closed_attachments.entries.clone();
                owned.extend(attachments.iter().map(|(id, attachment)| {
                    (
                        id.clone(),
                        OwnedAttachment {
                            tab_id: attachment.tab_id.clone(),
                            lifecycle: attachment.lifecycle.clone(),
                            transfers: attachment.transfers.clone(),
                        },
                    )
                }));
                let dispatch_services = services.clone();
                let dispatch_request = request.clone();
                let outcome = match remote_blocking(&services, &mut cancellation, move || {
                    dispatch_services.dispatch(&dispatch_request, &owned)
                })
                .await
                {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        let _ = cancelled.send(true);
                        let _ = outbound.controls.try_send(EgressControl::Close);
                        break;
                    }
                };
                let DispatchOutcome {
                    frames,
                    transfers,
                    transfer_token,
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
                            if enqueue_event(
                                &outbound,
                                error_response(
                                    request_id,
                                    "terminal.attachment_closed",
                                    "the terminal attachment is no longer active",
                                ),
                            )
                            .await
                            .is_err()
                            {
                                break 'socket;
                            }
                            continue;
                        };
                        let (done, completed) = tokio::sync::oneshot::channel();
                        let outcome = run_attachment_command(
                            &attachment.commands,
                            AttachmentCommand::Resume {
                                request_id,
                                tab_id,
                                attachment_id: attachment_id.clone(),
                                requested_revision,
                                done,
                            },
                            completed,
                            &mut cancellation,
                        )
                        .await;
                        match outcome {
                            AttachmentCommandOutcome::Completed(Ok(())) => {}
                            AttachmentCommandOutcome::ConnectionCancelled => break 'socket,
                            AttachmentCommandOutcome::Completed(Err(()))
                            | AttachmentCommandOutcome::Unavailable => {
                                if let Some(attachment) = attachments.get(&attachment_id) {
                                    if attachment.lifecycle.is_active() {
                                        close_unavailable_attachment(attachment);
                                    }
                                }
                                if enqueue_event(
                                    &outbound,
                                    error_response(
                                        request_id,
                                        "terminal.attachment_closed",
                                        "the terminal attachment closed during recovery",
                                    ),
                                )
                                .await
                                .is_err()
                                {
                                    break 'socket;
                                }
                            }
                        }
                    }
                    Some(SequencedAction::Detach {
                        request_id,
                        attachment_id,
                    }) => {
                        let Some(attachment) = attachments.get(&attachment_id) else {
                            if enqueue_event(
                                &outbound,
                                error_response(
                                    request_id,
                                    "terminal.attachment_closed",
                                    "the terminal attachment is no longer active",
                                ),
                            )
                            .await
                            .is_err()
                            {
                                break 'socket;
                            }
                            continue;
                        };
                        let closed_tab = attachment.tab_id.clone();
                        let closed_lifecycle = attachment.lifecycle.clone();
                        let closed_transfers = attachment.transfers.clone();
                        let (done, completed) = tokio::sync::oneshot::channel();
                        let outcome = run_attachment_command(
                            &attachment.commands,
                            AttachmentCommand::Detach { frames, done },
                            completed,
                            &mut cancellation,
                        )
                        .await;
                        match outcome {
                            AttachmentCommandOutcome::Completed(Ok(())) => {
                                if let Some(attachment) = attachments.remove(&attachment_id) {
                                    shutdown_attachment(attachment).await;
                                }
                                closed_attachments.insert(
                                    attachment_id,
                                    closed_tab,
                                    closed_lifecycle,
                                    closed_transfers,
                                );
                            }
                            AttachmentCommandOutcome::ConnectionCancelled => break 'socket,
                            AttachmentCommandOutcome::Completed(Err(()))
                            | AttachmentCommandOutcome::Unavailable => {
                                if let Some(attachment) = attachments.get(&attachment_id) {
                                    if attachment.lifecycle.is_active() {
                                        close_unavailable_attachment(attachment);
                                    }
                                }
                                if enqueue_event(
                                    &outbound,
                                    error_response(
                                        request_id,
                                        "terminal.attachment_closed",
                                        "the terminal attachment closed during detach",
                                    ),
                                )
                                .await
                                .is_err()
                                {
                                    break 'socket;
                                }
                            }
                        }
                    }
                    None => {
                        let transfer_token = transfer_token.or_else(|| {
                            transfers
                                .first()
                                .and_then(TransferPlan::attachment_id)
                                .and_then(|id| {
                                    started
                                        .as_ref()
                                        .filter(|attachment| &attachment.attachment_id == id)
                                        .map(|attachment| attachment.transfers.current())
                                })
                        });
                        if enqueue_outcome(&outbound, frames, transfers, transfer_token)
                            .await
                            .is_err()
                        {
                            break 'socket;
                        }
                    }
                }
                if let Some(started) = started {
                    let id = started.attachment_id.clone();
                    let cancellation = started.cancellation.clone();
                    let transfer_state = started.transfers.clone();
                    let lifecycle = started.lifecycle.clone();
                    let (commands, command_receiver) = tokio::sync::mpsc::channel(8);
                    let task = tokio::spawn(attachment_actor(
                        started,
                        command_receiver,
                        outbound.clone(),
                        services.clone(),
                        cancelled.subscribe(),
                        attachment_completed.clone(),
                    ));
                    attachments.insert(
                        id,
                        ConnectionAttachment {
                            tab_id: tab_id.expect("started attachments name their tab"),
                            cancellation,
                            commands,
                            task,
                            transfers: transfer_state,
                            lifecycle,
                        },
                    );
                }
            }
            InboundMessage::Ping(bytes) => {
                if outbound
                    .controls
                    .send(EgressControl::Message(Message::Pong(bytes.into())))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            InboundMessage::Pong => {}
        }
    }
    let _ = cancelled.send(true);
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
    if tokio::time::timeout(ATTACHMENT_SHUTDOWN_TIMEOUT, &mut reader)
        .await
        .is_err()
    {
        reader.abort();
        let _ = reader.await;
    }
}

async fn shutdown_attachment(mut attachment: ConnectionAttachment) {
    attachment.cancellation.close_mailbox();
    attachment.transfers.cancel();
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

async fn reap_finished_attachments(
    attachments: &mut HashMap<AttachmentId, ConnectionAttachment>,
    closed: &mut ClosedAttachments,
) {
    let finished = attachments
        .iter()
        .filter_map(|(id, attachment)| attachment.task.is_finished().then(|| id.clone()))
        .collect::<Vec<_>>();
    for id in finished {
        if let Some(attachment) = attachments.remove(&id) {
            let tab_id = attachment.tab_id.clone();
            let lifecycle = attachment.lifecycle.clone();
            let transfers = attachment.transfers.clone();
            shutdown_attachment(attachment).await;
            closed.insert(id, tab_id, lifecycle, transfers);
        }
    }
}

#[cfg(test)]
mod request_guard_tests {
    use super::*;
    use crate::terminal::model::{CursorState, ScreenSnapshot, TerminalModes};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Barrier, Condvar};

    fn tagged_test_transfer(
        request_id: u64,
        attachment_id: &AttachmentId,
        token: TransferToken,
    ) -> TaggedTransfer {
        let tab_id = TabId::new();
        let snapshot = ScreenSnapshot::new(
            tab_id.as_str(),
            Revision(1),
            TerminalSize::try_new(1, 1).unwrap(),
            Vec::new(),
            Vec::new(),
            CursorState::new(0, 0, true),
            TerminalModes::new(false, false, false),
        );
        TaggedTransfer {
            attachment_id: Some(attachment_id.clone()),
            plan: plan_snapshot_for_attachment(request_id, &tab_id, Some(attachment_id), snapshot)
                .unwrap(),
            token: Some(token),
            trailer: None,
            completion: None,
            ingress_permit: None,
        }
    }

    #[tokio::test]
    async fn cancelling_any_queue_position_preserves_sibling_transfer_fifo() {
        enum TargetPosition {
            Active,
            Pending,
            Ingress,
        }

        for target_position in [
            TargetPosition::Active,
            TargetPosition::Pending,
            TargetPosition::Ingress,
        ] {
            let target_id = AttachmentId::new();
            let sibling_id = AttachmentId::new();
            let target_token = TransferToken::new();
            target_token.cancel();
            let sibling_token = TransferToken::new();
            let target = tagged_test_transfer(10, &target_id, target_token);
            let sibling_one = tagged_test_transfer(11, &sibling_id, sibling_token.clone());
            let sibling_two = tagged_test_transfer(12, &sibling_id, sibling_token.clone());
            let (mut active, mut pending, ingress_item) = match target_position {
                TargetPosition::Active => (Some(target), Some(sibling_one), sibling_two),
                TargetPosition::Pending => (Some(sibling_one), Some(target), sibling_two),
                TargetPosition::Ingress => (Some(sibling_one), Some(sibling_two), target),
            };
            let (ingress_sender, mut ingress) = tokio::sync::mpsc::channel(1);
            ingress_sender.send(ingress_item).await.unwrap();

            let _ = cancel_attachment_transfers(
                &mut active,
                &mut pending,
                &mut ingress,
                &ingress_sender,
                &target_id,
            );

            let mut order = Vec::new();
            if let Some(transfer) = active {
                order.push(transfer.plan.request_id());
            }
            if let Some(transfer) = pending {
                order.push(transfer.plan.request_id());
            }
            if let Ok(transfer) = ingress.try_recv() {
                order.push(transfer.plan.request_id());
            }
            assert_eq!(order, vec![11, 12]);
        }
    }

    #[test]
    fn a_transfer_authorized_before_finalization_is_not_rebound_to_final_generation() {
        let tab_id = TabId::new();
        let attachment_id = AttachmentId::new();
        let lifecycle = AttachmentLifecycle::new();
        let transfers = AttachmentTransferState::new();
        let attachments = HashMap::from([(
            attachment_id.clone(),
            OwnedAttachment {
                tab_id: tab_id.clone(),
                lifecycle: lifecycle.clone(),
                transfers: transfers.clone(),
            },
        )]);
        let authorization_complete = Arc::new(Barrier::new(2));
        let authorization_complete_for_thread = authorization_complete.clone();
        let admission_allowed = Arc::new(Barrier::new(2));
        let admission_allowed_for_thread = admission_allowed.clone();
        let request_attachment_id = attachment_id.clone();

        let request = std::thread::spawn(move || {
            let request_generation =
                authorize_attachment(&attachments, &tab_id, &request_attachment_id).unwrap();
            authorization_complete_for_thread.wait();
            admission_allowed_for_thread.wait();
            request_generation
        });

        authorization_complete.wait();
        let final_generation = transfers.replace_for_close(&lifecycle);
        admission_allowed.wait();
        let request_generation = request.join().unwrap();

        assert!(
            request_generation.is_cancelled(),
            "a request authorized before finalization must retain the cancelled generation"
        );
        assert!(
            !final_generation.is_cancelled(),
            "the final snapshot must retain its own live generation"
        );

        let stale = tagged_test_transfer(42, &attachment_id, request_generation);
        let (ingress_sender, mut ingress) = tokio::sync::mpsc::channel(1);
        ingress_sender.blocking_send(stale).unwrap();
        let mut active = None;
        let mut pending = None;
        let cancelled = cancel_attachment_transfers(
            &mut active,
            &mut pending,
            &mut ingress,
            &ingress_sender,
            &attachment_id,
        );
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].plan.request_id(), 42);
        assert_eq!(
            cancelled[0].token.as_ref().unwrap().error_code(),
            "terminal.attachment_closed"
        );
        assert!(active.is_none() && pending.is_none() && ingress.try_recv().is_err());
    }

    #[test]
    fn closed_attachment_cache_is_strictly_bounded() {
        let tab_id = TabId::new();
        let mut cache = ClosedAttachments::default();
        let mut ids = Vec::new();
        for _ in 0..=CLOSED_ATTACHMENT_CACHE {
            let id = AttachmentId::new();
            cache.insert(
                id.clone(),
                tab_id.clone(),
                AttachmentLifecycle::new(),
                AttachmentTransferState::new(),
            );
            ids.push(id);
        }

        assert_eq!(cache.entries.len(), CLOSED_ATTACHMENT_CACHE);
        assert!(!cache.entries.contains_key(&ids[0]));
        assert!(matches!(
            authorize_attachment(&cache.entries, &tab_id, ids.last().unwrap()),
            Err("terminal.attachment_closed")
        ));
    }

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn orphaned_blocking_operations_never_exceed_the_global_cap() {
        let services = RemoteServices::new(Arc::new(TabRegistry::default()));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (cancelled, _) = tokio::sync::watch::channel(false);
        let mut tasks = Vec::new();
        for _ in 0..REMOTE_BLOCKING_OPERATIONS * 2 {
            let service = services.clone();
            let current = active.clone();
            let peak = maximum.clone();
            let blocked = gate.clone();
            let mut cancellation = cancelled.subscribe();
            tasks.push(tokio::spawn(async move {
                remote_blocking(&service, &mut cancellation, move || {
                    let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    let (lock, changed) = &*blocked;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = changed.wait(released).unwrap();
                    }
                    current.fetch_sub(1, Ordering::SeqCst);
                })
                .await
            }));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while active.load(Ordering::SeqCst) < REMOTE_BLOCKING_OPERATIONS {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        assert_eq!(maximum.load(Ordering::SeqCst), REMOTE_BLOCKING_OPERATIONS);
        // `send` discards the new value when the receiver count reaches zero
        // at the same instant. `send_replace` makes cancellation sticky for
        // tasks that have been spawned but have not polled their receiver yet.
        cancelled.send_replace(true);
        for task in tasks {
            assert!(task.await.unwrap().is_err());
        }
        let (lock, changed) = &*gate;
        *lock.lock().unwrap() = true;
        changed.notify_all();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while active.load(Ordering::SeqCst) != 0 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
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
