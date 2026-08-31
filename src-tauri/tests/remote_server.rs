use aiterm_lib::pty::{PtySink, PtySpawnSpec};
use aiterm_lib::remote::auth::DeviceStore;
use aiterm_lib::remote::model::TerminalSize;
use aiterm_lib::remote::server::{
    RemoteGateway, RemoteServices, TlsIdentity, MAX_SCROLLBACK_PAGE_ROWS, MAX_TERMINAL_INPUT_BYTES,
};
use aiterm_lib::remote::uploads::{
    MAX_SUBMISSION_BYTES, MAX_UPLOADS_PER_SUBMISSION, MAX_UPLOAD_CHUNK_BYTES,
};
use aiterm_lib::services::agents::AgentService;
use aiterm_lib::services::sessions::{SessionRoots, SessionService};
use aiterm_lib::tabs::{
    AttachmentId, AttachmentKind, PtyBackend, TabLaunch, TabRegistry, TabRegistryEvent, TabUpdate,
};
use futures_util::{SinkExt, StreamExt};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use rand_core::OsRng;
use rustls::pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::Message, Connector, MaybeTlsStream, WebSocketStream,
};
use x509_parser::extensions::GeneralName;

type TestSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Default)]
struct TestPty {
    next_id: AtomicU32,
    sinks: Mutex<HashMap<u32, Arc<dyn PtySink>>>,
    blocking_kill: AtomicU32,
    kill_entered: AtomicBool,
    kill_state: Mutex<bool>,
    kill_changed: Condvar,
}

impl TestPty {
    fn emit(&self, id: u32, bytes: &[u8]) {
        self.sinks.lock().unwrap()[&id].output(id, bytes);
    }

    fn last_id(&self) -> u32 {
        self.next_id.load(Ordering::SeqCst)
    }

    fn block_kill(&self, id: u32) {
        self.blocking_kill.store(id, Ordering::SeqCst);
    }

    fn release_kill(&self) {
        *self.kill_state.lock().unwrap() = true;
        self.kill_changed.notify_all();
    }

    fn exit(&self, id: u32, code: Option<u32>, signal: Option<&str>) {
        self.sinks.lock().unwrap()[&id].exited(id, code, signal);
    }
}

impl PtyBackend for TestPty {
    fn spawn(&self, _spec: PtySpawnSpec, sink: Arc<dyn PtySink>) -> Result<u32, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.sinks.lock().unwrap().insert(id, sink);
        Ok(id)
    }

    fn write(&self, _id: u32, _bytes: &[u8]) -> Result<(), String> {
        Ok(())
    }

    fn resize(&self, _id: u32, _cols: u16, _rows: u16) -> Result<(), String> {
        Ok(())
    }

    fn kill(&self, id: u32) {
        if self.blocking_kill.load(Ordering::SeqCst) == id {
            self.kill_entered.store(true, Ordering::SeqCst);
            let mut released = self.kill_state.lock().unwrap();
            while !*released {
                released = self.kill_changed.wait(released).unwrap();
            }
        }
        self.sinks.lock().unwrap().remove(&id);
    }

    fn pty_for_descendant(&self, _pid: u32) -> Option<u32> {
        None
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    kind: String,
    nonce: Vec<u8>,
}

#[derive(Serialize)]
struct Proof<'a> {
    kind: &'static str,
    device_id: &'a str,
    signature_der: &'a [u8],
}

#[derive(Deserialize)]
struct AuthReply {
    kind: String,
}

#[derive(Serialize)]
struct PairRequest<'a> {
    kind: &'static str,
    enrollment_secret: &'a [u8],
    device_name: &'a str,
    public_key: &'a [u8],
}

#[derive(Deserialize)]
struct PairPending {
    kind: String,
    request_id: String,
}

#[derive(Deserialize)]
struct PairApproved {
    kind: String,
    device_id: String,
}

#[derive(Serialize)]
struct RequestEnvelope<'a> {
    version: u16,
    request_id: u64,
    kind: &'a str,
    payload: &'a [u8],
}

#[derive(Deserialize)]
struct ResponseEnvelope {
    version: u16,
    request_id: u64,
    kind: String,
    payload: Vec<u8>,
}

#[derive(Deserialize)]
struct TabListReply {
    tabs: Vec<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct RemoteRosterTab {
    id: String,
    title: String,
    size: TerminalSize,
    focus: String,
}

#[derive(Deserialize)]
struct StateSnapshotReply {
    transfer_id: String,
    revision: u64,
    index: u32,
    total: u32,
    tabs: Vec<RemoteRosterTab>,
}

#[derive(Deserialize)]
struct TabChangedReply {
    revision: u64,
    change: String,
    tab_id: String,
    tab: Option<RemoteRosterTab>,
    requested: Option<bool>,
}

#[derive(Deserialize)]
struct ErrorReply {
    code: String,
}

#[derive(Serialize)]
struct TabRequest<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
}

#[derive(Serialize)]
struct TabRequestWithUnknownField<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
    unexpected: bool,
}

#[derive(Deserialize)]
struct AttachedReply {
    tab_id: String,
    attachment_id: String,
    has_focus: bool,
    title: String,
}

#[derive(Deserialize)]
struct SnapshotChunkReply {
    transfer_id: String,
    tab_id: String,
    attachment_id: Option<String>,
    kind: String,
    index: u32,
    total: u32,
}

#[derive(Deserialize)]
struct ExitReply {
    attachment_id: String,
}

#[derive(Serialize)]
struct AttachmentRequest<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
    attachment_id: &'a str,
}

#[derive(Serialize)]
struct ResumeRequest<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
    attachment_id: &'a str,
    revision: u64,
}

#[derive(Serialize)]
struct InputRequest<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
    attachment_id: &'a str,
    data: Vec<u8>,
}

#[derive(Serialize)]
struct UploadBeginRequest<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
    attachment_id: &'a str,
    submission_id: &'a str,
    submission_count: u8,
    submission_bytes: u64,
    length: u64,
    media_type: &'a str,
    #[serde(with = "serde_bytes")]
    sha256: &'a [u8],
}

#[derive(Serialize)]
struct UploadBeginRequestWithUnknownField<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
    attachment_id: &'a str,
    submission_id: &'a str,
    submission_count: u8,
    submission_bytes: u64,
    length: u64,
    media_type: &'a str,
    #[serde(with = "serde_bytes")]
    sha256: &'a [u8],
    unexpected: bool,
}

#[derive(Serialize)]
struct UploadChunkRequest<'a> {
    upload_id: &'a str,
    index: u32,
    #[serde(with = "serde_bytes")]
    data: &'a [u8],
}

#[derive(Serialize)]
struct UploadChunkRequestWithUnknownField<'a> {
    upload_id: &'a str,
    index: u32,
    #[serde(with = "serde_bytes")]
    data: &'a [u8],
    unexpected: bool,
}

#[derive(Serialize)]
struct UploadIdRequest<'a> {
    upload_id: &'a str,
}

#[derive(Serialize)]
struct UploadIdRequestWithUnknownField<'a> {
    upload_id: &'a str,
    unexpected: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadBeginReply {
    upload_id: String,
    next_chunk: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadFinishReply {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SuccessReply {
    ok: bool,
}

#[derive(Serialize)]
struct ScrollbackRequest<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
    attachment_id: &'a str,
    offset: usize,
    count: usize,
}

#[derive(Serialize)]
struct SizedAttachmentRequest<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
    attachment_id: &'a str,
    size: TerminalSize,
}

#[derive(Deserialize)]
struct FocusChangedReply {
    attachment_id: String,
    focus: String,
}

#[derive(Deserialize)]
struct TitleChangedReply {
    title: String,
}

#[derive(Serialize)]
struct OpenRequest {
    kind: &'static str,
    project_path: Option<String>,
    title: Option<String>,
    size: TerminalSize,
}

#[derive(Deserialize)]
struct TabOpenedReply {
    tab_id: String,
}

#[derive(Deserialize)]
struct ResumeReply {
    tab_id: String,
    attachment_id: String,
    requested_revision: u64,
    current_revision: u64,
    recovery_required: bool,
    title: String,
    focus: String,
    size: TerminalSize,
}

fn private_test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("aiterm-gateway-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> T {
    ciborium::from_reader(bytes).unwrap()
}

fn test_jpeg() -> Vec<u8> {
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
    use std::io::Cursor;

    let image = ImageBuffer::from_pixel(16, 12, Rgb([19_u8, 71_u8, 113_u8]));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut bytes, ImageFormat::Jpeg)
        .unwrap();
    bytes.into_inner()
}

fn upload_begin<'a>(
    tab_id: &'a aiterm_lib::tabs::TabId,
    attachment_id: &'a str,
    submission_id: &'a str,
    bytes: &'a [u8],
    digest: &'a [u8],
) -> UploadBeginRequest<'a> {
    UploadBeginRequest {
        tab_id,
        attachment_id,
        submission_id,
        submission_count: 1,
        submission_bytes: bytes.len() as u64,
        length: bytes.len() as u64,
        media_type: "image/jpeg",
        sha256: digest,
    }
}

fn tls_client(cert: &[u8], versions: &[&'static rustls::SupportedProtocolVersion]) -> Connector {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(cert.to_vec()))
        .expect("gateway certificate should be a usable trust anchor");
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(versions)
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Connector::Rustls(Arc::new(config))
}

async fn connect(gateway: &aiterm_lib::remote::server::GatewayHandle) -> TestSocket {
    let url = format!("wss://127.0.0.1:{}/v1/ws", gateway.local_addr().port());
    connect_async_tls_with_config(
        url,
        None,
        true,
        Some(tls_client(
            gateway.certificate_der(),
            &[&rustls::version::TLS13],
        )),
    )
    .await
    .expect("pinned TLS websocket should connect")
    .0
}

async fn challenge(socket: &mut TestSocket) -> Challenge {
    let message = socket
        .next()
        .await
        .expect("server should send an authentication challenge")
        .expect("challenge frame should be readable");
    let Message::Binary(bytes) = message else {
        panic!("challenge should be binary CBOR");
    };
    decode(&bytes)
}

async fn authenticate(
    socket: &mut TestSocket,
    key: &SigningKey,
    device_id: &str,
) -> ResponseEnvelope {
    let challenge = challenge(socket).await;
    let signature: Signature = key.sign(&challenge.nonce);
    socket
        .send(Message::Binary(
            encode(&Proof {
                kind: "auth.proof",
                device_id,
                signature_der: signature.to_der().as_bytes(),
            })
            .into(),
        ))
        .await
        .unwrap();
    let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
        panic!("authentication reply should be binary CBOR");
    };
    let reply: AuthReply = decode(&bytes);
    assert_eq!(reply.kind, "auth.ok");
    let state = tokio::time::timeout(Duration::from_secs(1), response(socket))
        .await
        .expect("authenticated connection did not publish a state snapshot");
    assert_eq!(state.request_id, 0);
    assert_eq!(state.kind, "state.snapshot");
    state
}

#[tokio::test]
async fn authentication_proof_rejects_a_trailing_cbor_item() {
    let root = private_test_dir("auth-trailing-cbor");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    let challenge = challenge(&mut socket).await;
    let signature: Signature = key.sign(&challenge.nonce);
    let mut proof = encode(&Proof {
        kind: "auth.proof",
        device_id: &device_id,
        signature_der: signature.to_der().as_bytes(),
    });
    proof.push(0xf6);
    socket.send(Message::Binary(proof.into())).await.unwrap();
    assert_closed(&mut socket).await;

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn request_payload_rejects_trailing_cbor_with_request_id_preserved() {
    let root = private_test_dir("payload-trailing-cbor");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    let mut payload = encode(&TabRequest {
        tab_id: &aiterm_lib::tabs::TabId::new(),
    });
    payload.extend_from_slice(b"garbage");
    socket
        .send(request(41, "tab.close", &payload))
        .await
        .unwrap();
    let error = response(&mut socket).await;
    assert_eq!(error.request_id, 41);
    assert_eq!(
        decode::<ErrorReply>(&error.payload).code,
        "protocol.invalid_payload"
    );

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

fn valid_request(request_id: u64) -> Message {
    Message::Binary(
        encode(&RequestEnvelope {
            version: 1,
            request_id,
            kind: "session.list",
            payload: b"",
        })
        .into(),
    )
}

fn request(request_id: u64, kind: &str, payload: &[u8]) -> Message {
    Message::Binary(
        encode(&RequestEnvelope {
            version: 1,
            request_id,
            kind,
            payload,
        })
        .into(),
    )
}

async fn response(socket: &mut TestSocket) -> ResponseEnvelope {
    let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
        panic!("response should be a binary CBOR frame");
    };
    decode(&bytes)
}

async fn response_kind(socket: &mut TestSocket, kind: &str) -> ResponseEnvelope {
    loop {
        let event = response(socket).await;
        if event.kind == kind {
            return event;
        }
    }
}

async fn response_request(socket: &mut TestSocket, request_id: u64) -> ResponseEnvelope {
    loop {
        let event = response(socket).await;
        if event.request_id == request_id {
            return event;
        }
    }
}

async fn assert_closed(socket: &mut TestSocket) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match tokio::time::timeout_at(deadline, socket.next()).await {
            Ok(Some(Ok(Message::Binary(_)))) => continue,
            Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) => return,
            other => panic!("server left an unauthorized connection open: {other:?}"),
        }
    }
}

fn paired_store(root: &PathBuf) -> (Arc<DeviceStore>, SigningKey, String) {
    let store = Arc::new(DeviceStore::open(root.join("devices")).unwrap());
    let now = UNIX_EPOCH + Duration::from_secs(10_000);
    let enrollment = store.begin_enrollment_at(now).unwrap();
    let key = SigningKey::random(&mut OsRng);
    let public_key = key.verifying_key().to_encoded_point(true);
    let device = store
        .approve_at(enrollment.secret(), "phone", public_key.as_bytes(), now)
        .unwrap();
    (store, key, device.id)
}

fn services() -> RemoteServices {
    RemoteServices::new(Arc::new(TabRegistry::default()))
}

#[tokio::test]
async fn authenticated_device_completes_a_real_tls_websocket_handshake() {
    let root = private_test_dir("proof");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store.clone(),
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    let challenge = challenge(&mut socket).await;
    assert_eq!(challenge.kind, "auth.challenge");
    assert_eq!(challenge.nonce.len(), 32);
    let signature: Signature = key.sign(&challenge.nonce);

    socket
        .send(Message::Binary(
            encode(&Proof {
                kind: "auth.proof",
                device_id: &device_id,
                signature_der: signature.to_der().as_bytes(),
            })
            .into(),
        ))
        .await
        .unwrap();
    let reply = socket.next().await.unwrap().unwrap();
    let Message::Binary(bytes) = reply else {
        panic!("authentication reply should be binary CBOR");
    };
    let reply: AuthReply = decode(&bytes);
    assert_eq!(reply.kind, "auth.ok");
    assert_eq!(
        store.list_devices()[0].last_ip.as_deref(),
        Some("127.0.0.1"),
        "the gateway must pass the socket peer IP to successful authentication",
    );

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn revocation_notifies_and_closes_a_live_authenticated_connection() {
    let root = private_test_dir("live-revocation");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store.clone(),
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    assert!(store.revoke(&device_id).unwrap());
    let revoked = tokio::time::timeout(Duration::from_secs(1), response(&mut socket))
        .await
        .expect("live revocation should be delivered before close");
    assert_eq!(revoked.request_id, 0);
    assert_eq!(revoked.kind, "auth.revoked");
    assert_closed(&mut socket).await;

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn authenticated_connection_starts_with_a_recoverable_tab_state_snapshot() {
    let root = private_test_dir("initial-tab-state");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open_desktop(TabLaunch::new(
            "Before auth",
            "before-auth",
            TerminalSize::try_new(34, 7).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;

    let state = authenticate(&mut socket, &key, &device_id).await;
    let state: StateSnapshotReply = decode(&state.payload);
    assert_eq!(state.revision, 1);
    assert_eq!(state.index, 0);
    assert_eq!(state.total, 1);
    assert!(!state.transfer_id.is_empty());
    assert_eq!(state.tabs.len(), 1);
    assert_eq!(state.tabs[0].id, tab.as_str());
    assert_eq!(state.tabs[0].title, "Before auth");
    assert_eq!(state.tabs[0].size, TerminalSize::try_new(34, 7).unwrap());
    assert_eq!(state.tabs[0].focus, "unowned");

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn authenticated_roster_snapshot_is_a_bounded_complete_ordered_transfer() {
    let root = private_test_dir("chunked-tab-state");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let title = "x".repeat(16 * 1024);
    let mut expected = Vec::new();
    for index in 0..80 {
        expected.push(
            registry
                .open_desktop(TabLaunch::new(
                    format!("{index}:{title}"),
                    format!("roster-{index}"),
                    TerminalSize::try_new(80, 24).unwrap(),
                ))
                .unwrap()
                .to_string(),
        );
    }
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;

    let first = authenticate(&mut socket, &key, &device_id).await;
    let mut chunks = vec![decode::<StateSnapshotReply>(&first.payload)];
    let total = chunks[0].total;
    while chunks.len() < total as usize {
        let frame = response(&mut socket).await;
        assert_eq!(frame.kind, "state.snapshot");
        chunks.push(decode(&frame.payload));
    }

    assert!(total > 1, "the oversized roster must exercise chunking");
    assert!(chunks
        .iter()
        .all(|chunk| chunk.transfer_id == chunks[0].transfer_id));
    assert_eq!(
        chunks.iter().map(|chunk| chunk.index).collect::<Vec<_>>(),
        (0..total).collect::<Vec<_>>(),
    );
    let actual = chunks
        .into_iter()
        .flat_map(|chunk| chunk.tabs.into_iter().map(|tab| tab.id))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);

    for tab in registry.list() {
        registry.close(tab.id()).ok();
    }
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn sustained_registry_events_cannot_starve_a_correlated_inbound_request() {
    let root = private_test_dir("fair-registry-events");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open_desktop(TabLaunch::new(
            "flood",
            "flood",
            TerminalSize::try_new(80, 24).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    let keep_flooding = Arc::new(AtomicBool::new(true));
    let flood_flag = keep_flooding.clone();
    let flood_registry = registry.clone();
    let flood_tab = tab.clone();
    let flood = tokio::spawn(async move {
        let mut sequence = 0u64;
        while flood_flag.load(Ordering::SeqCst) {
            flood_registry
                .update(
                    &flood_tab,
                    TabUpdate::new().title(format!("title-{sequence}")),
                )
                .unwrap();
            sequence += 1;
            tokio::task::yield_now().await;
        }
    });
    socket.send(request(991, "tab.list", b"")).await.unwrap();

    let mut registry_events = 0usize;
    let correlated = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = response(&mut socket).await;
            if event.request_id == 991 {
                break event;
            }
            if event.kind == "tab.changed" || event.kind == "state.snapshot" {
                registry_events += 1;
            }
        }
    })
    .await
    .expect("a sustained title stream must not starve inbound work");
    keep_flooding.store(false, Ordering::SeqCst);
    flood.await.unwrap();

    assert_eq!(correlated.kind, "tab.list");
    assert!(
        registry_events <= 8,
        "inbound work was delayed behind {registry_events} registry events"
    );

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn idle_authenticated_connection_continues_past_each_registry_fairness_budget() {
    let root = private_test_dir("idle-registry-events");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open_desktop(TabLaunch::new(
            "idle-flood",
            "idle-flood",
            TerminalSize::try_new(80, 24).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    for sequence in 0..12 {
        registry
            .update(
                &tab,
                TabUpdate::new().title(format!("idle-title-{sequence}")),
            )
            .unwrap();
    }
    let received = tokio::time::timeout(Duration::from_secs(2), async {
        let mut titles = 0usize;
        while titles < 12 {
            let event = response(&mut socket).await;
            if event.kind == "tab.changed" {
                titles += 1;
            }
        }
        titles
    })
    .await;

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
    assert_eq!(
        received.expect("idle clients must receive registry events after the fairness turn"),
        12
    );
}

#[tokio::test]
async fn desktop_open_and_update_emit_authenticated_remote_tab_changes() {
    let root = private_test_dir("desktop-tab-changes");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    let initial: StateSnapshotReply =
        decode(&authenticate(&mut socket, &key, &device_id).await.payload);
    assert!(initial.tabs.is_empty());

    let tab = registry
        .open_desktop(TabLaunch::new(
            "Desktop",
            "desktop-change",
            TerminalSize::try_new(40, 8).unwrap(),
        ))
        .unwrap();
    let opened = response_kind(&mut socket, "tab.changed").await;
    let opened: TabChangedReply = decode(&opened.payload);
    assert_eq!(opened.change, "opened");
    assert_eq!(opened.tab_id, tab.as_str());
    assert_eq!(opened.tab.as_ref().unwrap().title, "Desktop");

    registry
        .update(&tab, TabUpdate::new().title("Updated on desktop"))
        .unwrap();
    let changed_event = response_kind(&mut socket, "tab.changed").await;
    assert!(!changed_event
        .payload
        .windows("attachmentId".len())
        .any(|window| window == b"attachmentId"));
    assert!(!changed_event
        .payload
        .windows("inputOwner".len())
        .any(|window| window == b"inputOwner"));
    let changed: TabChangedReply = decode(&changed_event.payload);
    assert!(changed.revision > opened.revision);
    assert_eq!(changed.change, "changed");
    assert_eq!(changed.tab_id, tab.as_str());
    assert_eq!(changed.tab.unwrap().title, "Updated on desktop");
    assert!(changed.requested.is_none());

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn remote_open_and_close_drive_the_desktop_registry_projection() {
    let root = private_test_dir("remote-desktop-projection");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let desktop = registry.subscribe_changes();
    let _initial = desktop.recv_timeout(Duration::from_secs(1)).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    let _state = authenticate(&mut socket, &key, &device_id).await;

    socket
        .send(request(
            1,
            "tab.open",
            &encode(&OpenRequest {
                kind: "shell",
                project_path: None,
                title: Some("Phone tab".to_string()),
                size: TerminalSize::try_new(30, 6).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let opened_reply = response_kind(&mut socket, "tab.open").await;
    let opened: TabOpenedReply = decode(&opened_reply.payload);
    let desktop_open = desktop.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        desktop_open,
        TabRegistryEvent::Opened { tab, .. } if tab.id().as_str() == opened.tab_id
    ));

    let tab = registry.list()[0].id().clone();
    socket
        .send(request(
            2,
            "tab.close",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    assert_eq!(response_kind(&mut socket, "tab.close").await.request_id, 2);
    assert!(matches!(
        desktop.recv_timeout(Duration::from_secs(1)).unwrap(),
        TabRegistryEvent::Removed {
            requested: true,
            ..
        }
    ));

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn authenticated_tab_list_is_dispatched_with_a_typed_correlated_response() {
    let root = private_test_dir("tab-list-dispatch");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let services = RemoteServices::new(Arc::new(TabRegistry::default()));
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services,
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    socket.send(request(44, "tab.list", b"")).await.unwrap();
    let reply = response(&mut socket).await;

    assert_eq!(reply.version, 1);
    assert_eq!(reply.request_id, 44);
    assert_eq!(reply.kind, "tab.list");
    let payload: TabListReply = decode(&reply.payload);
    assert!(payload.tabs.is_empty());
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn session_list_is_available_after_authentication() {
    let root = private_test_dir("session-list-dispatch");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let services = RemoteServices::with_application_services(
        Arc::new(TabRegistry::default()),
        SessionService::from_roots(SessionRoots::new(
            root.join("sessions"),
            root.join("trash"),
            root.join("tasks"),
            root.join("jobs"),
            root.join("forks.json"),
        )),
        AgentService::empty(),
    );
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services,
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    socket.send(request(73, "session.list", b"")).await.unwrap();
    let reply = response(&mut socket).await;

    assert_eq!(reply.request_id, 73);
    assert_eq!(reply.kind, "session.list");
    let payload: serde::de::IgnoredAny = decode(&reply.payload);
    let _ = payload;
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn terminal_attach_returns_an_opaque_attachment_and_typed_snapshot_chunks() {
    let root = private_test_dir("terminal-attach-dispatch");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::default());
    let tab = registry
        .open(
            TabLaunch::new(
                "Remote shell",
                "remote-server-test",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            .with_command("sleep 5"),
        )
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    socket
        .send(request(
            91,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached = response(&mut socket).await;
    assert_eq!(attached.request_id, 91);
    assert_eq!(attached.kind, "terminal.attach");
    let attached_payload: AttachedReply = decode(&attached.payload);
    assert_eq!(attached_payload.tab_id, tab.as_str());
    assert!(!attached_payload.attachment_id.is_empty());
    assert!(!attached_payload.has_focus);
    assert_eq!(attached_payload.title, "Remote shell");

    let snapshot = response(&mut socket).await;
    assert_eq!(snapshot.request_id, 91);
    assert_eq!(snapshot.kind, "terminal.snapshot");
    let snapshot_payload: SnapshotChunkReply = decode(&snapshot.payload);
    assert_eq!(snapshot_payload.tab_id, tab.as_str());
    assert_eq!(
        snapshot_payload.attachment_id.as_deref(),
        Some(attached_payload.attachment_id.as_str())
    );
    assert_eq!(snapshot_payload.kind, "snapshot");
    assert_eq!(snapshot_payload.index, 0);
    assert!(snapshot_payload.total >= 1);

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn terminal_resume_is_authorized_and_returns_correlated_snapshot_recovery() {
    let root = private_test_dir("terminal-resume");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::default());
    let tab = registry
        .open(
            TabLaunch::new(
                "Resume",
                "remote-resume-test",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            .with_command("sleep 5"),
        )
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached = response(&mut socket).await;
    let attached: AttachedReply = decode(&attached.payload);
    let _initial_snapshot = response(&mut socket).await;
    let remote_attachment: AttachmentId = decode(&encode(&attached.attachment_id));
    let recovery_size = TerminalSize::try_new(31, 4).unwrap();
    registry
        .update(&tab, TabUpdate::new().title("authoritative recovery title"))
        .unwrap();
    registry
        .take_focus(&tab, &remote_attachment, recovery_size)
        .unwrap();

    socket
        .send(request(
            2,
            "terminal.resume",
            &encode(&ResumeRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                revision: u64::MAX,
            }),
        ))
        .await
        .unwrap();
    let resumed = loop {
        let event = response(&mut socket).await;
        if event.request_id == 2 {
            break event;
        }
        assert_eq!(event.request_id, 0);
        assert!(matches!(
            event.kind.as_str(),
            "tab.changed" | "terminal.snapshot" | "terminal.title" | "terminal.focus_changed"
        ));
    };
    assert_eq!(resumed.request_id, 2);
    assert_eq!(resumed.kind, "terminal.resume");
    let resumed: ResumeReply = decode(&resumed.payload);
    assert_eq!(resumed.tab_id, tab.as_str());
    assert_eq!(resumed.attachment_id, attached.attachment_id);
    assert_eq!(resumed.requested_revision, u64::MAX);
    assert!(resumed.recovery_required);
    assert_ne!(resumed.current_revision, u64::MAX);
    assert_eq!(resumed.title, "authoritative recovery title");
    assert_eq!(resumed.focus, "self");
    assert_eq!(resumed.size, recovery_size);
    let recovery = response(&mut socket).await;
    assert_eq!(recovery.request_id, 2);
    assert_eq!(recovery.kind, "terminal.snapshot");
    let recovery_chunk: SnapshotChunkReply = decode(&recovery.payload);
    for expected in recovery_chunk.index + 1..recovery_chunk.total {
        let next = response(&mut socket).await;
        assert_eq!(next.request_id, 2);
        assert_eq!(next.kind, "terminal.snapshot");
        assert_eq!(decode::<SnapshotChunkReply>(&next.payload).index, expected);
    }

    registry
        .update(&tab, TabUpdate::new().title("post recovery title"))
        .unwrap();
    let desktop = registry.attach(&tab, AttachmentKind::Desktop).unwrap();
    registry
        .take_focus(&tab, &desktop.id, TerminalSize::try_new(42, 5).unwrap())
        .unwrap();
    let mut saw_title = false;
    let mut saw_focus = false;
    while !saw_title || !saw_focus {
        let event = tokio::time::timeout(Duration::from_secs(1), response(&mut socket))
            .await
            .expect("post-boundary controls must follow recovery");
        match event.kind.as_str() {
            "terminal.title" => {
                assert_eq!(
                    decode::<TitleChangedReply>(&event.payload).title,
                    "post recovery title"
                );
                saw_title = true;
            }
            "terminal.focus_changed" => {
                assert_eq!(decode::<FocusChangedReply>(&event.payload).focus, "other");
                saw_focus = true;
            }
            "terminal.snapshot" => {
                assert_eq!(event.request_id, 0);
            }
            "tab.changed" => {
                assert_eq!(event.request_id, 0);
            }
            other => panic!(
                "unexpected event after recovery: {other}, request_id={}",
                event.request_id
            ),
        }
    }

    let mut other = connect(&gateway).await;
    authenticate(&mut other, &key, &device_id).await;
    other
        .send(request(
            1,
            "terminal.resume",
            &encode(&ResumeRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                revision: resumed.current_revision,
            }),
        ))
        .await
        .unwrap();
    let unauthorized = response(&mut other).await;
    assert_eq!(unauthorized.request_id, 1);
    assert_eq!(
        decode::<ErrorReply>(&unauthorized.payload).code,
        "terminal.attachment_not_found"
    );

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn tab_list_and_focus_projection_never_serialize_internal_attachment_ids() {
    let root = private_test_dir("private-attachment-ids");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::default());
    let tab = registry
        .open(
            TabLaunch::new(
                "Private",
                "private-ids-test",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            // Keep the fixture alive through TLS setup even on a loaded test
            // host. The registry closes it explicitly at teardown.
            .with_command("sleep 300"),
        )
        .unwrap();
    let desktop = registry.attach(&tab, AttachmentKind::Desktop).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached = response(&mut socket).await;
    let attached: AttachedReply = decode(&attached.payload);
    let _snapshot = response(&mut socket).await;

    socket.send(request(2, "tab.list", b"")).await.unwrap();
    let listed = response(&mut socket).await;

    assert!(!listed
        .payload
        .windows(desktop.id.as_str().len())
        .any(|window| window == desktop.id.as_str().as_bytes()));
    assert!(!listed
        .payload
        .windows(attached.attachment_id.len())
        .any(|window| window == attached.attachment_id.as_bytes()));
    assert!(!listed
        .payload
        .windows("inputOwner".len())
        .any(|window| window == b"inputOwner"));

    let mut other = connect(&gateway).await;
    authenticate(&mut other, &key, &device_id).await;
    other
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let other_attached = response(&mut other).await;
    let other_attached: AttachedReply = decode(&other_attached.payload);
    let _other_snapshot = response(&mut other).await;
    other
        .send(request(
            2,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &other_attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        response_kind(&mut other, "terminal.focus").await.kind,
        "terminal.focus"
    );
    let first_focus_event = response_kind(&mut socket, "terminal.focus_changed").await;
    let first_focus: FocusChangedReply = decode(&first_focus_event.payload);
    assert_eq!(first_focus.attachment_id, attached.attachment_id);
    assert_eq!(first_focus.focus, "other");
    assert!(!first_focus_event
        .payload
        .windows(other_attached.attachment_id.len())
        .any(|window| window == other_attached.attachment_id.as_bytes()));
    let other_focus_event = response_kind(&mut other, "terminal.focus_changed").await;
    let other_focus: FocusChangedReply = decode(&other_focus_event.payload);
    assert_eq!(other_focus.attachment_id, other_attached.attachment_id);
    assert_eq!(other_focus.focus, "self");

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn idle_detach_and_connection_teardown_remove_registry_attachments() {
    let root = private_test_dir("idle-detach");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::default());
    let tab = registry
        .open(
            TabLaunch::new(
                "Idle",
                "remote-idle-test",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            .with_command("sleep 5"),
        )
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    for request_id in 1..=24 {
        socket
            .send(request(
                request_id * 2 - 1,
                "terminal.attach",
                &encode(&TabRequest { tab_id: &tab }),
            ))
            .await
            .unwrap();
        let attached = response(&mut socket).await;
        let attached: AttachedReply = decode(&attached.payload);
        let _snapshot = response(&mut socket).await;
        socket
            .send(request(
                request_id * 2,
                "terminal.detach",
                &encode(&AttachmentRequest {
                    tab_id: &tab,
                    attachment_id: &attached.attachment_id,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response(&mut socket).await.kind, "terminal.detach");
        assert_eq!(registry.attachment_count(&tab).unwrap(), 0);
    }

    socket
        .send(request(
            49,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let _attached = response(&mut socket).await;
    let _snapshot = response(&mut socket).await;
    assert_eq!(registry.attachment_count(&tab).unwrap(), 1);
    socket.close(None).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while registry.attachment_count(&tab).unwrap() != 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn resume_is_a_barrier_against_pre_snapshot_live_events() {
    let root = private_test_dir("resume-barrier");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Barrier",
            "remote-resume-barrier",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply = decode(&response(&mut socket).await.payload);
    let _initial = response(&mut socket).await;

    pty.emit(pty.last_id(), b"before");
    socket
        .send(request(
            2,
            "terminal.resume",
            &encode(&ResumeRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                revision: u64::MAX,
            }),
        ))
        .await
        .unwrap();
    loop {
        let event = response(&mut socket).await;
        if event.request_id == 2 && event.kind == "terminal.snapshot" {
            break;
        }
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(40), socket.next())
            .await
            .is_err()
    );

    pty.emit(pty.last_id(), b"after");
    let later = tokio::time::timeout(
        Duration::from_secs(1),
        response_kind(&mut socket, "terminal.diff"),
    )
    .await
    .expect("post-recovery damage should remain live");
    assert_eq!(later.request_id, 0);

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn detach_ack_is_the_last_event_for_an_attachment_with_pending_damage() {
    let root = private_test_dir("detach-barrier");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Barrier",
            "remote-detach-barrier",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply = decode(&response(&mut socket).await.payload);
    let _initial = response(&mut socket).await;

    pty.emit(pty.last_id(), b"pending");
    socket
        .send(request(
            2,
            "terminal.detach",
            &encode(&AttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
            }),
        ))
        .await
        .unwrap();
    loop {
        let event = response(&mut socket).await;
        if event.request_id == 2 && event.kind == "terminal.detach" {
            break;
        }
    }
    let detached_deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while registry.attachment_count(&tab).unwrap() != 0 {
        assert!(tokio::time::Instant::now() < detached_deadline);
        tokio::task::yield_now().await;
    }
    pty.emit(pty.last_id(), b"later");
    assert!(
        tokio::time::timeout(Duration::from_millis(40), socket.next())
            .await
            .is_err()
    );

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn well_formed_envelope_errors_preserve_request_id_and_tab_list_must_be_empty() {
    let root = private_test_dir("correlated-errors");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    socket.send(request(10, "tab.list", &[0xa0])).await.unwrap();
    let invalid_payload = response(&mut socket).await;
    assert_eq!(invalid_payload.request_id, 10);
    assert_eq!(invalid_payload.kind, "error");
    assert_eq!(
        decode::<ErrorReply>(&invalid_payload.payload).code,
        "protocol.invalid_payload"
    );

    socket
        .send(Message::Binary(
            cbor_request_with_version(11, 99, "tab.list", b"").into(),
        ))
        .await
        .unwrap();
    let version = response(&mut socket).await;
    assert_eq!(version.request_id, 11);
    assert_eq!(
        decode::<ErrorReply>(&version.payload).code,
        "protocol.unsupported_version"
    );

    socket
        .send(request(
            12,
            "terminal.attach",
            &encode(&TabRequestWithUnknownField {
                tab_id: &aiterm_lib::tabs::TabId::new(),
                unexpected: true,
            }),
        ))
        .await
        .unwrap();
    let unknown_field = response(&mut socket).await;
    assert_eq!(unknown_field.request_id, 12);
    assert_eq!(
        decode::<ErrorReply>(&unknown_field.payload).code,
        "protocol.invalid_payload"
    );

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn authenticated_socket_rejects_a_frame_exactly_one_mebibyte_before_decode() {
    let root = private_test_dir("exact-frame");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    socket
        .send(Message::Binary(vec![0; 1024 * 1024].into()))
        .await
        .unwrap();
    assert_closed(&mut socket).await;

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn a_slow_close_does_not_stall_an_unrelated_authenticated_connection() {
    let root = private_test_dir("slow-close");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let slow = registry
        .open(TabLaunch::new(
            "Slow",
            "slow-close",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let _other = registry
        .open(TabLaunch::new(
            "Other",
            "other-tab",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    pty.block_kill(1);
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry),
    )
    .await
    .unwrap();
    let mut closing = connect(&gateway).await;
    let mut unrelated = connect(&gateway).await;
    authenticate(&mut closing, &key, &device_id).await;
    authenticate(&mut unrelated, &key, &device_id).await;

    closing
        .send(request(
            1,
            "tab.close",
            &encode(&TabRequest { tab_id: &slow }),
        ))
        .await
        .unwrap();
    let entered_deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while !pty.kill_entered.load(Ordering::SeqCst) {
        assert!(tokio::time::Instant::now() < entered_deadline);
        tokio::task::yield_now().await;
    }
    unrelated.send(request(1, "tab.list", b"")).await.unwrap();
    let unrelated_reply = tokio::time::timeout(
        Duration::from_millis(250),
        response_kind(&mut unrelated, "tab.list"),
    )
    .await;
    pty.release_kill();
    let closing_reply = response_kind(&mut closing, "tab.close").await;

    assert_eq!(unrelated_reply.unwrap().kind, "tab.list");
    assert_eq!(closing_reply.kind, "tab.close");
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn dense_snapshot_streams_multiple_sub_mebibyte_socket_frames_and_slow_reader_is_bounded() {
    let root = private_test_dir("multi-chunk");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Dense",
            "dense-snapshot",
            TerminalSize::try_new(512, 48).unwrap(),
        ))
        .unwrap();
    let cell = format!("x{}", "\u{301}".repeat(32));
    let row = cell.repeat(512);
    let output = (0..48)
        .map(|_| row.as_str())
        .collect::<Vec<_>>()
        .join("\r\n");
    pty.emit(pty.last_id(), output.as_bytes());
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached = response(&mut socket).await;
    assert_eq!(attached.kind, "terminal.attach");

    let mut total = None;
    let mut seen = 0u32;
    loop {
        let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
            panic!("snapshot chunks are binary CBOR");
        };
        assert!(bytes.len() < 1024 * 1024);
        let envelope: ResponseEnvelope = decode(&bytes);
        assert_eq!(envelope.kind, "terminal.snapshot");
        let chunk: SnapshotChunkReply = decode(&envelope.payload);
        total.get_or_insert(chunk.total);
        assert_eq!(chunk.index, seen);
        seen += 1;
        if seen == chunk.total {
            break;
        }
    }
    assert!(total.unwrap() > 1);

    // Stop reading while enough live damage is produced to fill the bounded
    // outbound lane. Registry output must still complete via snapshot recovery
    // rather than creating an unbounded forwarding queue.
    let emitting = pty.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            for _ in 0..200 {
                emitting.emit(emitting.last_id(), b"\rbounded");
            }
        }),
    )
    .await
    .expect("slow reader must not block canonical PTY ingestion")
    .unwrap();

    socket.close(None).await.ok();
    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn connection_egress_keeps_competing_transfers_contiguous_and_controls_prompt() {
    let root = private_test_dir("egress-arbiter");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Arbiter",
            "egress-arbiter",
            TerminalSize::try_new(512, 48).unwrap(),
        ))
        .unwrap();
    let cell = format!("x{}", "\u{301}".repeat(32));
    let row = cell.repeat(512);
    let output = (0..48)
        .map(|_| row.as_str())
        .collect::<Vec<_>>()
        .join("\r\n");
    pty.emit(pty.last_id(), output.as_bytes());
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    socket
        .send(request(
            2,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();

    let mut attached = 0;
    let mut current_transfer = None::<String>;
    let mut transfer_order = Vec::<String>::new();
    let mut completed = HashMap::<String, u32>::new();
    let mut title_requested = false;
    let mut title_prompt = false;
    let stop_controls = Arc::new(AtomicBool::new(false));
    let mut control_producer = None;
    while attached < 2 || completed.len() < 2 {
        let event = tokio::time::timeout(Duration::from_secs(5), response(&mut socket))
            .await
            .expect("egress must continue making progress");
        match event.kind.as_str() {
            "terminal.attach" => attached += 1,
            "terminal.snapshot" => {
                let chunk: SnapshotChunkReply = decode(&event.payload);
                if current_transfer.as_ref() != Some(&chunk.transfer_id) {
                    assert!(
                        !transfer_order.contains(&chunk.transfer_id),
                        "transfer chunks interleaved"
                    );
                    transfer_order.push(chunk.transfer_id.clone());
                    current_transfer = Some(chunk.transfer_id.clone());
                }
                assert_eq!(
                    completed.get(&chunk.transfer_id).copied().unwrap_or(0),
                    chunk.index
                );
                completed.insert(chunk.transfer_id.clone(), chunk.index + 1);
                if !title_requested && chunk.index == 0 && chunk.total > 1 {
                    let updating_registry = registry.clone();
                    let updating_tab = tab.clone();
                    let stop = stop_controls.clone();
                    control_producer = Some(tokio::task::spawn_blocking(move || {
                        let mut generation = 0u64;
                        while !stop.load(Ordering::Acquire) {
                            updating_registry
                                .update(
                                    &updating_tab,
                                    TabUpdate::new().title(format!("control-{generation}")),
                                )
                                .unwrap();
                            generation = generation.saturating_add(1);
                        }
                    }));
                    title_requested = true;
                }
                if chunk.index + 1 == chunk.total {
                    current_transfer = None;
                }
            }
            "terminal.title" => {
                if title_requested && current_transfer.is_some() {
                    title_prompt = true;
                }
            }
            other => panic!("unexpected egress event {other}"),
        }
    }
    stop_controls.store(true, Ordering::Release);
    if let Some(producer) = control_producer {
        producer.await.unwrap();
    }
    assert!(title_requested && title_prompt);
    assert_eq!(transfer_order.len(), 2);

    socket.close(None).await.ok();
    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn multi_chunk_final_snapshot_completes_before_exactly_one_exit_trailer() {
    let root = private_test_dir("final-snapshot-trailer");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Final",
            "final-snapshot-trailer",
            TerminalSize::try_new(512, 48).unwrap(),
        ))
        .unwrap();
    let cell = format!("x{}", "\u{301}".repeat(32));
    let row = cell.repeat(512);
    pty.emit(
        pty.last_id(),
        (0..48)
            .map(|_| row.as_str())
            .collect::<Vec<_>>()
            .join("\r\n")
            .as_bytes(),
    );
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    socket
        .send(request(
            2,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let mut attachment_ids = Vec::new();
    let mut initial_completed = HashMap::<String, u32>::new();
    while attachment_ids.len() < 2 || initial_completed.len() < 2 {
        let event = response(&mut socket).await;
        match event.kind.as_str() {
            "terminal.attach" => {
                let attached: AttachedReply = decode(&event.payload);
                attachment_ids.push(attached.attachment_id);
            }
            "terminal.snapshot" => {
                let chunk: SnapshotChunkReply = decode(&event.payload);
                if chunk.index + 1 == chunk.total {
                    initial_completed.insert(chunk.transfer_id, chunk.total);
                }
            }
            other => panic!("unexpected initial event {other}"),
        }
    }

    pty.exit(pty.last_id(), Some(0), None);
    let mut current_transfer = None::<String>;
    let mut transfer_attachment = HashMap::<String, String>::new();
    let mut next_index = HashMap::<String, u32>::new();
    let mut totals = HashMap::<String, u32>::new();
    let mut exited = Vec::<String>::new();
    let mut roster_changes = 0;
    while exited.len() < 2 {
        let event = tokio::time::timeout(Duration::from_secs(5), response(&mut socket))
            .await
            .expect("final transfer must make progress");
        match event.kind.as_str() {
            "terminal.snapshot" => {
                let chunk: SnapshotChunkReply = decode(&event.payload);
                let attachment_id = chunk.attachment_id.expect("final snapshots are attached");
                assert!(attachment_ids.contains(&attachment_id));
                if let Some(id) = &current_transfer {
                    assert_eq!(id, &chunk.transfer_id, "final transfers interleaved");
                } else {
                    current_transfer = Some(chunk.transfer_id.clone());
                    transfer_attachment.insert(chunk.transfer_id.clone(), attachment_id.clone());
                    totals.insert(attachment_id.clone(), chunk.total);
                    assert!(chunk.total > 1);
                }
                let expected = next_index.entry(attachment_id).or_default();
                assert_eq!(chunk.index, *expected);
                *expected += 1;
                if chunk.index + 1 == chunk.total {
                    current_transfer = None;
                }
            }
            "terminal.exited" => {
                let exit: ExitReply = decode(&event.payload);
                assert!(attachment_ids.contains(&exit.attachment_id));
                assert!(!exited.contains(&exit.attachment_id), "duplicate exit");
                assert_eq!(
                    next_index.get(&exit.attachment_id),
                    totals.get(&exit.attachment_id),
                    "exit preceded its final snapshot"
                );
                exited.push(exit.attachment_id);
            }
            "tab.changed" => {
                assert_eq!(event.request_id, 0);
                let change: TabChangedReply = decode(&event.payload);
                assert_eq!(change.change, "changed");
                assert_eq!(change.tab_id, tab.as_str());
                roster_changes += 1;
            }
            other => panic!("unexpected finalization event {other}"),
        }
    }
    assert_eq!(transfer_attachment.len(), 2);
    assert_eq!(roster_changes, 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), socket.next())
            .await
            .is_err()
    );

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn every_post_exit_attachment_operation_gets_one_correlated_closed_error() {
    let root = private_test_dir("post-exit-errors");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Exited",
            "post-exit-errors",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply = decode(&response(&mut socket).await.payload);
    while response(&mut socket).await.kind != "terminal.snapshot" {}

    pty.exit(pty.last_id(), Some(0), None);
    loop {
        if response(&mut socket).await.kind == "terminal.exited" {
            break;
        }
    }

    let requests = [
        request(
            2,
            "terminal.input",
            &encode(&InputRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                data: b"x".to_vec(),
            }),
        ),
        request(
            3,
            "terminal.resize",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ),
        request(
            4,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ),
        request(
            5,
            "terminal.scrollback",
            &encode(&ScrollbackRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                offset: 0,
                count: 1,
            }),
        ),
        request(
            6,
            "terminal.resume",
            &encode(&ResumeRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                revision: 0,
            }),
        ),
        request(
            7,
            "terminal.detach",
            &encode(&AttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
            }),
        ),
    ];
    for (request_id, operation) in (2u64..=7).zip(requests) {
        socket.send(operation).await.unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(2), response(&mut socket))
            .await
            .expect("post-exit operation must receive a correlated response");
        assert_eq!(reply.request_id, request_id);
        assert_eq!(reply.kind, "error");
        assert_eq!(
            decode::<ErrorReply>(&reply.payload).code,
            "terminal.attachment_closed"
        );
    }

    socket.send(request(8, "tab.list", &[])).await.unwrap();
    assert_eq!(response(&mut socket).await.kind, "tab.list");
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn resume_cancelling_a_scrollback_transfer_returns_a_correlated_outcome() {
    let root = private_test_dir("cancelled-scrollback-outcome");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Scrollback",
            "cancelled-scrollback-outcome",
            TerminalSize::try_new(512, 48).unwrap(),
        ))
        .unwrap();
    let cell = format!("x{}", "\u{301}".repeat(32));
    let row = cell.repeat(512);
    pty.emit(
        pty.last_id(),
        (0..300)
            .map(|_| row.as_str())
            .collect::<Vec<_>>()
            .join("\r\n")
            .as_bytes(),
    );
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply = decode(&response(&mut socket).await.payload);
    loop {
        let event = response(&mut socket).await;
        if event.kind == "terminal.snapshot" {
            let chunk: SnapshotChunkReply = decode(&event.payload);
            if chunk.index + 1 == chunk.total {
                break;
            }
        }
    }

    socket
        .send(request(
            2,
            "terminal.scrollback",
            &encode(&ScrollbackRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                offset: 0,
                count: 256,
            }),
        ))
        .await
        .unwrap();
    let first = response_kind(&mut socket, "terminal.scrollback").await;
    let first: SnapshotChunkReply = decode(&first.payload);
    assert!(first.total > 1);
    socket
        .send(request(
            3,
            "terminal.resume",
            &encode(&ResumeRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                revision: 0,
            }),
        ))
        .await
        .unwrap();

    let mut scrollback_error = None;
    let mut scrollback_chunks = 1;
    let mut resume_reply = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline
        && (!resume_reply || (scrollback_error.is_none() && scrollback_chunks < first.total))
    {
        let event = tokio::time::timeout(Duration::from_millis(500), response(&mut socket)).await;
        let Ok(event) = event else { continue };
        match (event.request_id, event.kind.as_str()) {
            (2, "terminal.scrollback") => scrollback_chunks += 1,
            (2, "error") => scrollback_error = Some(decode::<ErrorReply>(&event.payload).code),
            (3, "terminal.resume") => resume_reply = true,
            _ => {}
        }
    }
    assert!(resume_reply);
    assert!(
        scrollback_chunks == first.total
            || scrollback_error.as_deref() == Some("terminal.transfer_cancelled"),
        "a correlated transfer must complete or report cancellation"
    );

    socket.close(None).await.ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn finalization_preempts_backpressured_live_transfer_admission() {
    let root = private_test_dir("finalize-preempts-admission");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Preempt",
            "finalize-preempts-admission",
            TerminalSize::try_new(512, 48).unwrap(),
        ))
        .unwrap();
    let cell = format!("x{}", "\u{301}".repeat(32));
    let row = cell.repeat(512);
    pty.emit(
        pty.last_id(),
        (0..48)
            .map(|_| row.as_str())
            .collect::<Vec<_>>()
            .join("\r\n")
            .as_bytes(),
    );
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    for request_id in 1..=3 {
        socket
            .send(request(
                request_id,
                "terminal.attach",
                &encode(&TabRequest { tab_id: &tab }),
            ))
            .await
            .unwrap();
    }
    let mut attachment_ids = Vec::new();
    while attachment_ids.len() < 3 {
        let event = response(&mut socket).await;
        if event.kind == "terminal.attach" {
            attachment_ids.push(decode::<AttachedReply>(&event.payload).attachment_id);
        }
    }

    pty.emit(pty.last_id(), format!("\r\n{row}").as_bytes());
    tokio::time::sleep(Duration::from_millis(100)).await;
    socket
        .send(request(
            4,
            "terminal.resume",
            &encode(&ResumeRequest {
                tab_id: &tab,
                attachment_id: &attachment_ids[0],
                revision: 0,
            }),
        ))
        .await
        .unwrap();
    let resume_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(tokio::time::Instant::now() < resume_deadline);
        let event = response(&mut socket).await;
        if event.request_id == 4 && event.kind == "terminal.resume" {
            break;
        }
    }
    socket
        .send(request(
            5,
            "terminal.detach",
            &encode(&AttachmentRequest {
                tab_id: &tab,
                attachment_id: &attachment_ids[1],
            }),
        ))
        .await
        .unwrap();
    let detach_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(tokio::time::Instant::now() < detach_deadline);
        let event = response(&mut socket).await;
        if event.request_id == 5 && event.kind == "terminal.detach" {
            break;
        }
    }
    pty.exit(pty.last_id(), Some(0), None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_final_snapshot = false;
    while tokio::time::Instant::now() < deadline {
        let Ok(event) =
            tokio::time::timeout(Duration::from_millis(500), response(&mut socket)).await
        else {
            continue;
        };
        if event.request_id == 0 && event.kind == "terminal.snapshot" {
            saw_final_snapshot = true;
            break;
        }
    }
    assert!(
        saw_final_snapshot,
        "Finalized must cancel blocked live admission instead of waiting behind it"
    );

    socket.close(None).await.ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn operations_during_final_transfer_are_correlated_and_a_sibling_stays_usable() {
    let root = private_test_dir("during-final-operations");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let exiting_tab = registry
        .open(TabLaunch::new(
            "Exiting",
            "during-final-operations",
            TerminalSize::try_new(512, 16).unwrap(),
        ))
        .unwrap();
    let exiting_pty = pty.last_id();
    let cell = format!("x{}", "\u{301}".repeat(32));
    let row = cell.repeat(512);
    pty.emit(
        exiting_pty,
        (0..16)
            .map(|_| row.as_str())
            .collect::<Vec<_>>()
            .join("\r\n")
            .as_bytes(),
    );
    let sibling_tab = registry
        .open(TabLaunch::new(
            "Sibling",
            "during-final-sibling",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest {
                tab_id: &exiting_tab,
            }),
        ))
        .await
        .unwrap();
    let exiting: AttachedReply = decode(&response(&mut socket).await.payload);
    loop {
        let event = response(&mut socket).await;
        if event.kind == "terminal.snapshot" {
            let chunk: SnapshotChunkReply = decode(&event.payload);
            if chunk.index + 1 == chunk.total {
                break;
            }
        }
    }
    socket
        .send(request(
            2,
            "terminal.attach",
            &encode(&TabRequest {
                tab_id: &sibling_tab,
            }),
        ))
        .await
        .unwrap();
    let sibling: AttachedReply = decode(&response(&mut socket).await.payload);
    let _sibling_snapshot = response_kind(&mut socket, "terminal.snapshot").await;

    pty.exit(exiting_pty, Some(0), None);
    loop {
        let event = response(&mut socket).await;
        if event.request_id == 0 && event.kind == "terminal.snapshot" {
            let chunk: SnapshotChunkReply = decode(&event.payload);
            if chunk.attachment_id.as_deref() == Some(exiting.attachment_id.as_str()) {
                assert!(chunk.total > 1);
                break;
            }
        }
    }

    let operations = [
        request(
            3,
            "terminal.input",
            &encode(&InputRequest {
                tab_id: &exiting_tab,
                attachment_id: &exiting.attachment_id,
                data: b"x".to_vec(),
            }),
        ),
        request(
            4,
            "terminal.resize",
            &encode(&SizedAttachmentRequest {
                tab_id: &exiting_tab,
                attachment_id: &exiting.attachment_id,
                size: TerminalSize::try_new(512, 16).unwrap(),
            }),
        ),
        request(
            5,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &exiting_tab,
                attachment_id: &exiting.attachment_id,
                size: TerminalSize::try_new(512, 16).unwrap(),
            }),
        ),
        request(
            6,
            "terminal.scrollback",
            &encode(&ScrollbackRequest {
                tab_id: &exiting_tab,
                attachment_id: &exiting.attachment_id,
                offset: 0,
                count: 1,
            }),
        ),
        request(
            7,
            "terminal.resume",
            &encode(&ResumeRequest {
                tab_id: &exiting_tab,
                attachment_id: &exiting.attachment_id,
                revision: 0,
            }),
        ),
        request(
            8,
            "terminal.detach",
            &encode(&AttachmentRequest {
                tab_id: &exiting_tab,
                attachment_id: &exiting.attachment_id,
            }),
        ),
    ];
    let mut saw_exit = false;
    for (request_id, operation) in (3u64..=8).zip(operations) {
        socket.send(operation).await.unwrap();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), response(&mut socket))
                .await
                .expect("finalizing requests must remain responsive");
            if event.kind == "terminal.exited" {
                saw_exit = true;
            }
            if event.request_id == request_id {
                assert_eq!(event.kind, "error");
                assert_eq!(
                    decode::<ErrorReply>(&event.payload).code,
                    "terminal.attachment_closed"
                );
                break;
            }
        }
    }

    socket
        .send(request(
            9,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &sibling_tab,
                attachment_id: &sibling.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    loop {
        let event = response(&mut socket).await;
        if event.kind == "terminal.exited" {
            saw_exit = true;
        }
        if event.request_id == 9 {
            if event.kind == "error" {
                let error: ErrorReply = decode(&event.payload);
                panic!("sibling focus failed: {}", error.code);
            }
            assert_eq!(event.kind, "terminal.focus");
            break;
        }
    }
    socket
        .send(request(
            10,
            "terminal.resize",
            &encode(&SizedAttachmentRequest {
                tab_id: &sibling_tab,
                attachment_id: &sibling.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        response_kind(&mut socket, "terminal.resize")
            .await
            .request_id,
        10
    );
    while !saw_exit {
        if response(&mut socket).await.kind == "terminal.exited" {
            saw_exit = true;
        }
    }

    socket.close(None).await.ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn peer_close_cancels_a_blocked_dispatch_and_detaches_other_attachments() {
    let root = private_test_dir("cancel-blocked-dispatch");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let attached_tab = registry
        .open(TabLaunch::new(
            "Attached",
            "cancel-attached",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let blocked_tab = registry
        .open(TabLaunch::new(
            "Blocked",
            "cancel-blocked",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    pty.block_kill(2);
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest {
                tab_id: &attached_tab,
            }),
        ))
        .await
        .unwrap();
    let _attached = response(&mut socket).await;
    let _snapshot = response(&mut socket).await;
    assert_eq!(registry.attachment_count(&attached_tab).unwrap(), 1);
    socket
        .send(request(
            2,
            "tab.close",
            &encode(&TabRequest {
                tab_id: &blocked_tab,
            }),
        ))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while !pty.kill_entered.load(Ordering::SeqCst) {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    drop(socket);
    let detached = tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            if registry.attachment_count(&attached_tab).unwrap() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    pty.release_kill();
    assert!(
        detached.is_ok(),
        "peer close did not cancel blocked dispatch"
    );

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn revocation_after_a_blocked_dispatch_delivers_auth_revoked_before_close() {
    let root = private_test_dir("revoke-blocked-dispatch");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let attached_tab = registry
        .open(TabLaunch::new(
            "Attached",
            "revoke-attached",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let blocked_tab = registry
        .open(TabLaunch::new(
            "Blocked",
            "revoke-blocked",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    pty.block_kill(2);
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store.clone(),
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest {
                tab_id: &attached_tab,
            }),
        ))
        .await
        .unwrap();
    let _attached = response(&mut socket).await;
    let _snapshot = response(&mut socket).await;
    socket
        .send(request(
            2,
            "tab.close",
            &encode(&TabRequest {
                tab_id: &blocked_tab,
            }),
        ))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while !pty.kill_entered.load(Ordering::SeqCst) {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }

    assert!(store.revoke(&device_id).unwrap());
    pty.release_kill();
    let revoked = tokio::time::timeout(Duration::from_secs(1), response(&mut socket))
        .await
        .expect("revocation must be delivered after the blocked dispatch releases");
    assert_eq!(revoked.request_id, 0);
    assert_eq!(revoked.kind, "auth.revoked");
    assert_closed(&mut socket).await;

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn peer_close_cancels_dense_recovery_planning_and_detaches_promptly() {
    let root = private_test_dir("cancel-recovery-planning");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open(TabLaunch::new(
            "Recovery",
            "cancel-recovery-planning",
            TerminalSize::try_new(512, 48).unwrap(),
        ))
        .unwrap();
    let cell = format!("x{}", "\u{301}".repeat(32));
    let row = cell.repeat(512);
    pty.emit(
        pty.last_id(),
        (0..48)
            .map(|_| row.as_str())
            .collect::<Vec<_>>()
            .join("\r\n")
            .as_bytes(),
    );
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached = response(&mut socket).await;
    let attached: AttachedReply = decode(&attached.payload);
    loop {
        let event = response(&mut socket).await;
        let chunk: SnapshotChunkReply = decode(&event.payload);
        if chunk.index + 1 == chunk.total {
            break;
        }
    }
    socket
        .send(request(
            2,
            "terminal.resume",
            &encode(&ResumeRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                revision: u64::MAX,
            }),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    drop(socket);
    tokio::time::timeout(Duration::from_millis(300), async {
        while registry.attachment_count(&tab).unwrap() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("peer close must cancel recovery planning");

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn attachment_input_scrollback_command_and_path_limits_are_enforced() {
    let root = private_test_dir("request-limits");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty));
    let tab = registry
        .open(TabLaunch::new(
            "Limits",
            "limits",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached = response(&mut socket).await;
    let attached: AttachedReply = decode(&attached.payload);
    let _snapshot = response(&mut socket).await;

    socket
        .send(request(
            2,
            "terminal.input",
            &encode(&InputRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                data: vec![0; MAX_TERMINAL_INPUT_BYTES + 1],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response(&mut socket).await.payload).code,
        "terminal.input_too_large"
    );

    socket
        .send(request(
            3,
            "terminal.scrollback",
            &encode(&ScrollbackRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                offset: 0,
                count: MAX_SCROLLBACK_PAGE_ROWS + 1,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response(&mut socket).await.payload).code,
        "terminal.invalid_scrollback_page"
    );

    for (request_id, project_path, title) in [
        (4, Some("x".repeat(4 * 1024 + 1)), None),
        (5, None, Some("x".repeat(4 * 1024 + 1))),
    ] {
        socket
            .send(request(
                request_id,
                "tab.open",
                &encode(&OpenRequest {
                    kind: "shell",
                    project_path,
                    title,
                    size: TerminalSize::try_new(20, 2).unwrap(),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(
            decode::<ErrorReply>(&response(&mut socket).await.payload).code,
            "protocol.value_too_large"
        );
    }

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn terminal_upload_round_trip_survives_focus_loss_and_is_connection_scoped() {
    let root = private_test_dir("upload-round-trip");
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open(
            TabLaunch::new(
                "Uploads",
                "upload-round-trip",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            .with_cwd(project.to_string_lossy())
            .with_command("sleep 300"),
        )
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply =
        decode(&response_kind(&mut socket, "terminal.attach").await.payload);
    let _snapshot = response_kind(&mut socket, "terminal.snapshot").await;
    socket
        .send(request(
            2,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let _focused = response_kind(&mut socket, "terminal.focus").await;

    let jpeg = test_jpeg();
    let digest = Sha256::digest(&jpeg);
    socket
        .send(request(
            3,
            "terminal.upload.begin",
            &encode(&upload_begin(
                &tab,
                &attached.attachment_id,
                "round-trip",
                &jpeg,
                digest.as_slice(),
            )),
        ))
        .await
        .unwrap();
    let began = response_kind(&mut socket, "terminal.upload.begin").await;
    let began: UploadBeginReply = decode(&began.payload);
    assert_eq!(began.next_chunk, 0);

    let mut other = connect(&gateway).await;
    authenticate(&mut other, &key, &device_id).await;
    other
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let other_attached: AttachedReply =
        decode(&response_kind(&mut other, "terminal.attach").await.payload);
    let _other_snapshot = response_kind(&mut other, "terminal.snapshot").await;

    other
        .send(request(
            2,
            "terminal.upload.chunk",
            &encode(&UploadChunkRequest {
                upload_id: &began.upload_id,
                index: 0,
                data: &jpeg,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response_kind(&mut other, "error").await.payload).code,
        "terminal.upload_not_found"
    );

    other
        .send(request(
            3,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &other_attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let _other_focused = response_kind(&mut other, "terminal.focus").await;

    socket
        .send(request(
            4,
            "terminal.upload.chunk",
            &encode(&UploadChunkRequest {
                upload_id: &began.upload_id,
                index: 0,
                data: &jpeg,
            }),
        ))
        .await
        .unwrap();
    let chunk: SuccessReply = decode(
        &response_kind(&mut socket, "terminal.upload.chunk")
            .await
            .payload,
    );
    assert!(
        chunk.ok,
        "focus loss must not interrupt an authorized upload"
    );

    socket
        .send(request(
            5,
            "terminal.upload.finish",
            &encode(&UploadIdRequest {
                upload_id: &began.upload_id,
            }),
        ))
        .await
        .unwrap();
    let finished: UploadFinishReply = decode(
        &response_kind(&mut socket, "terminal.upload.finish")
            .await
            .payload,
    );
    let published = PathBuf::from(&finished.path);
    assert!(published.starts_with(project.join(".aiterm/attachments")));
    assert_eq!(std::fs::read(&published).unwrap(), jpeg);

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn terminal_upload_cancel_remains_authorized_after_focus_loss() {
    let root = private_test_dir("upload-cancel-focus-loss");
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open(
            TabLaunch::new(
                "Uploads",
                "upload-cancel-focus-loss",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            .with_cwd(project.to_string_lossy())
            .with_command("sleep 300"),
        )
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply =
        decode(&response_kind(&mut socket, "terminal.attach").await.payload);
    let _ = response_kind(&mut socket, "terminal.snapshot").await;
    socket
        .send(request(
            2,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let _ = response_kind(&mut socket, "terminal.focus").await;
    let jpeg = test_jpeg();
    let digest = Sha256::digest(&jpeg);
    socket
        .send(request(
            3,
            "terminal.upload.begin",
            &encode(&UploadBeginRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                submission_id: "cancel-focus-loss",
                submission_count: 2,
                submission_bytes: (jpeg.len() * 2) as u64,
                length: jpeg.len() as u64,
                media_type: "image/jpeg",
                sha256: digest.as_slice(),
            }),
        ))
        .await
        .unwrap();
    let began: UploadBeginReply = decode(
        &response_kind(&mut socket, "terminal.upload.begin")
            .await
            .payload,
    );

    socket
        .send(request(
            4,
            "terminal.upload.chunk",
            &encode(&UploadChunkRequest {
                upload_id: &began.upload_id,
                index: 0,
                data: &jpeg,
            }),
        ))
        .await
        .unwrap();
    let chunked: SuccessReply = decode(
        &response_kind(&mut socket, "terminal.upload.chunk")
            .await
            .payload,
    );
    assert!(chunked.ok);
    socket
        .send(request(
            5,
            "terminal.upload.finish",
            &encode(&UploadIdRequest {
                upload_id: &began.upload_id,
            }),
        ))
        .await
        .unwrap();
    let _: UploadFinishReply = decode(
        &response_kind(&mut socket, "terminal.upload.finish")
            .await
            .payload,
    );

    let desktop = registry.attach(&tab, AttachmentKind::Desktop).unwrap();
    registry
        .take_focus(&tab, &desktop.id, TerminalSize::try_new(20, 2).unwrap())
        .unwrap();
    socket
        .send(request(
            6,
            "terminal.upload.cancel",
            &encode(&UploadIdRequest {
                upload_id: &began.upload_id,
            }),
        ))
        .await
        .unwrap();
    let cancelled: SuccessReply = decode(&response_request(&mut socket, 6).await.payload);
    assert!(cancelled.ok);
    socket
        .send(request(
            7,
            "terminal.upload.cancel",
            &encode(&UploadIdRequest {
                upload_id: &began.upload_id,
            }),
        ))
        .await
        .unwrap();
    let cancelled_again: SuccessReply = decode(&response_request(&mut socket, 7).await.payload);
    assert!(cancelled_again.ok);
    assert!(std::fs::read_dir(project.join(".aiterm/attachments"))
        .unwrap()
        .all(|entry| entry
            .unwrap()
            .path()
            .extension()
            .and_then(|value| value.to_str())
            != Some("part")));

    socket
        .send(request(
            8,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let _ = response_kind(&mut socket, "terminal.focus").await;
    socket
        .send(request(
            9,
            "terminal.upload.begin",
            &encode(&upload_begin(
                &tab,
                &attached.attachment_id,
                "retry-after-cancel",
                &jpeg,
                digest.as_slice(),
            )),
        ))
        .await
        .unwrap();
    let retried: UploadBeginReply = decode(
        &response_kind(&mut socket, "terminal.upload.begin")
            .await
            .payload,
    );
    assert_eq!(retried.next_chunk, 0);

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn terminal_upload_detach_releases_incomplete_submission_before_late_cancel() {
    let root = private_test_dir("upload-detach-cancel-race");
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open(
            TabLaunch::new(
                "Uploads",
                "upload-detach-cancel-race",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            .with_cwd(project.to_string_lossy())
            .with_command("sleep 300"),
        )
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply =
        decode(&response_kind(&mut socket, "terminal.attach").await.payload);
    let _ = response_kind(&mut socket, "terminal.snapshot").await;
    socket
        .send(request(
            2,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let _ = response_kind(&mut socket, "terminal.focus").await;

    let jpeg = test_jpeg();
    let digest = Sha256::digest(&jpeg);
    socket
        .send(request(
            3,
            "terminal.upload.begin",
            &encode(&UploadBeginRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                submission_id: "detach-interrupted",
                submission_count: 2,
                submission_bytes: (jpeg.len() * 2) as u64,
                length: jpeg.len() as u64,
                media_type: "image/jpeg",
                sha256: digest.as_slice(),
            }),
        ))
        .await
        .unwrap();
    let began: UploadBeginReply = decode(
        &response_kind(&mut socket, "terminal.upload.begin")
            .await
            .payload,
    );
    socket
        .send(request(
            4,
            "terminal.upload.chunk",
            &encode(&UploadChunkRequest {
                upload_id: &began.upload_id,
                index: 0,
                data: &jpeg,
            }),
        ))
        .await
        .unwrap();
    let _: SuccessReply = decode(
        &response_kind(&mut socket, "terminal.upload.chunk")
            .await
            .payload,
    );
    socket
        .send(request(
            5,
            "terminal.upload.finish",
            &encode(&UploadIdRequest {
                upload_id: &began.upload_id,
            }),
        ))
        .await
        .unwrap();
    let finished: UploadFinishReply = decode(
        &response_kind(&mut socket, "terminal.upload.finish")
            .await
            .payload,
    );
    let published = PathBuf::from(finished.path);

    socket
        .send(request(
            6,
            "terminal.detach",
            &encode(&AttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
            }),
        ))
        .await
        .unwrap();
    let detached = response_request(&mut socket, 6).await;
    assert_eq!(detached.kind, "terminal.detach");
    socket
        .send(request(
            7,
            "terminal.upload.cancel",
            &encode(&UploadIdRequest {
                upload_id: &began.upload_id,
            }),
        ))
        .await
        .unwrap();
    let late_cancel = response_request(&mut socket, 7).await;
    assert_eq!(late_cancel.kind, "error");
    assert!(matches!(
        decode::<ErrorReply>(&late_cancel.payload).code.as_str(),
        "terminal.attachment_not_found" | "terminal.attachment_closed"
    ));
    assert_eq!(std::fs::read(&published).unwrap(), jpeg);

    socket
        .send(request(
            8,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let reattached: AttachedReply =
        decode(&response_kind(&mut socket, "terminal.attach").await.payload);
    let _ = response_kind(&mut socket, "terminal.snapshot").await;
    socket
        .send(request(
            9,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &reattached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let _ = response_kind(&mut socket, "terminal.focus").await;
    socket
        .send(request(
            10,
            "terminal.upload.begin",
            &encode(&upload_begin(
                &tab,
                &reattached.attachment_id,
                "retry-after-detach",
                &jpeg,
                digest.as_slice(),
            )),
        ))
        .await
        .unwrap();
    let retried = response_request(&mut socket, 10).await;
    assert_eq!(retried.kind, "terminal.upload.begin");
    assert_eq!(decode::<UploadBeginReply>(&retried.payload).next_chunk, 0);
    assert_eq!(std::fs::read(&published).unwrap(), jpeg);

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn terminal_upload_begin_requires_focus_and_payloads_are_strict_and_bounded() {
    let root = private_test_dir("upload-validation");
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open(
            TabLaunch::new(
                "Uploads",
                "upload-validation",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            .with_cwd(project.to_string_lossy())
            .with_command("sleep 300"),
        )
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply =
        decode(&response_kind(&mut socket, "terminal.attach").await.payload);
    let _ = response_kind(&mut socket, "terminal.snapshot").await;
    let jpeg = test_jpeg();
    let digest = Sha256::digest(&jpeg);

    socket
        .send(request(
            2,
            "terminal.upload.begin",
            &encode(&upload_begin(
                &tab,
                &attached.attachment_id,
                "no-focus",
                &jpeg,
                digest.as_slice(),
            )),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response_kind(&mut socket, "error").await.payload).code,
        "terminal.input_not_owned"
    );

    socket
        .send(request(
            3,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let _ = response_kind(&mut socket, "terminal.focus").await;

    let invalid_requests = [
        (
            4,
            encode(&UploadBeginRequestWithUnknownField {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                submission_id: "unknown-field",
                submission_count: 1,
                submission_bytes: jpeg.len() as u64,
                length: jpeg.len() as u64,
                media_type: "image/jpeg",
                sha256: digest.as_slice(),
                unexpected: true,
            }),
            "protocol.invalid_payload",
        ),
        (
            5,
            encode(&UploadBeginRequest {
                media_type: "image/png",
                ..upload_begin(
                    &tab,
                    &attached.attachment_id,
                    "bad-media",
                    &jpeg,
                    digest.as_slice(),
                )
            }),
            "terminal.upload_invalid_image",
        ),
        (
            6,
            encode(&UploadBeginRequest {
                sha256: &[0; 31],
                ..upload_begin(
                    &tab,
                    &attached.attachment_id,
                    "bad-digest",
                    &jpeg,
                    digest.as_slice(),
                )
            }),
            "terminal.upload_invalid_image",
        ),
        (
            7,
            encode(&UploadBeginRequest {
                submission_count: (MAX_UPLOADS_PER_SUBMISSION + 1) as u8,
                ..upload_begin(
                    &tab,
                    &attached.attachment_id,
                    "fifth-image",
                    &jpeg,
                    digest.as_slice(),
                )
            }),
            "terminal.upload_too_large",
        ),
        (
            8,
            encode(&UploadBeginRequest {
                submission_bytes: MAX_SUBMISSION_BYTES + 1,
                ..upload_begin(
                    &tab,
                    &attached.attachment_id,
                    "huge-submission",
                    &jpeg,
                    digest.as_slice(),
                )
            }),
            "terminal.upload_too_large",
        ),
    ];
    for (request_id, payload, expected) in invalid_requests {
        socket
            .send(request(request_id, "terminal.upload.begin", &payload))
            .await
            .unwrap();
        assert_eq!(
            decode::<ErrorReply>(&response_kind(&mut socket, "error").await.payload).code,
            expected
        );
    }

    socket
        .send(request(
            9,
            "terminal.upload.begin",
            &encode(&upload_begin(
                &tab,
                &attached.attachment_id,
                "chunk-limits",
                &jpeg,
                digest.as_slice(),
            )),
        ))
        .await
        .unwrap();
    let began: UploadBeginReply = decode(
        &response_kind(&mut socket, "terminal.upload.begin")
            .await
            .payload,
    );
    socket
        .send(request(
            10,
            "terminal.upload.chunk",
            &encode(&UploadChunkRequest {
                upload_id: &began.upload_id,
                index: 1,
                data: &jpeg,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response_kind(&mut socket, "error").await.payload).code,
        "terminal.upload_out_of_order"
    );

    socket
        .send(request(
            11,
            "terminal.upload.begin",
            &encode(&UploadBeginRequest {
                submission_id: "oversize-chunk",
                submission_bytes: (MAX_UPLOAD_CHUNK_BYTES + 1) as u64,
                length: (MAX_UPLOAD_CHUNK_BYTES + 1) as u64,
                sha256: &[0; 32],
                ..upload_begin(
                    &tab,
                    &attached.attachment_id,
                    "oversize-chunk",
                    &jpeg,
                    digest.as_slice(),
                )
            }),
        ))
        .await
        .unwrap();
    let began: UploadBeginReply = decode(
        &response_kind(&mut socket, "terminal.upload.begin")
            .await
            .payload,
    );
    socket
        .send(request(
            12,
            "terminal.upload.chunk",
            &encode(&UploadChunkRequest {
                upload_id: &began.upload_id,
                index: 0,
                data: &vec![0; MAX_UPLOAD_CHUNK_BYTES + 1],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response_kind(&mut socket, "error").await.payload).code,
        "terminal.upload_too_large"
    );

    socket
        .send(request(
            13,
            "terminal.upload.begin",
            &encode(&upload_begin(
                &tab,
                &attached.attachment_id,
                "completed-submission",
                &jpeg,
                digest.as_slice(),
            )),
        ))
        .await
        .unwrap();
    let completed: UploadBeginReply = decode(
        &response_kind(&mut socket, "terminal.upload.begin")
            .await
            .payload,
    );
    socket
        .send(request(
            14,
            "terminal.upload.chunk",
            &encode(&UploadChunkRequest {
                upload_id: &completed.upload_id,
                index: 0,
                data: &jpeg,
            }),
        ))
        .await
        .unwrap();
    let _: SuccessReply = decode(
        &response_kind(&mut socket, "terminal.upload.chunk")
            .await
            .payload,
    );
    socket
        .send(request(
            15,
            "terminal.upload.finish",
            &encode(&UploadIdRequest {
                upload_id: &completed.upload_id,
            }),
        ))
        .await
        .unwrap();
    let _: UploadFinishReply = decode(
        &response_kind(&mut socket, "terminal.upload.finish")
            .await
            .payload,
    );
    socket
        .send(request(
            16,
            "terminal.upload.begin",
            &encode(&upload_begin(
                &tab,
                &attached.attachment_id,
                "completed-submission",
                &jpeg,
                digest.as_slice(),
            )),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response_kind(&mut socket, "error").await.payload).code,
        "terminal.upload_invalid_submission"
    );

    socket
        .send(request(
            17,
            "terminal.upload.begin",
            &encode(&UploadBeginRequest {
                submission_id: "inconsistent-submission",
                submission_count: 2,
                submission_bytes: (jpeg.len() * 2) as u64,
                ..upload_begin(
                    &tab,
                    &attached.attachment_id,
                    "inconsistent-submission",
                    &jpeg,
                    digest.as_slice(),
                )
            }),
        ))
        .await
        .unwrap();
    let _: UploadBeginReply = decode(
        &response_kind(&mut socket, "terminal.upload.begin")
            .await
            .payload,
    );
    socket
        .send(request(
            18,
            "terminal.upload.begin",
            &encode(&UploadBeginRequest {
                submission_id: "inconsistent-submission",
                submission_count: 1,
                submission_bytes: jpeg.len() as u64,
                ..upload_begin(
                    &tab,
                    &attached.attachment_id,
                    "inconsistent-submission",
                    &jpeg,
                    digest.as_slice(),
                )
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response_kind(&mut socket, "error").await.payload).code,
        "terminal.upload_invalid_submission"
    );

    for (request_id, kind, payload) in [
        (
            19,
            "terminal.upload.chunk",
            encode(&UploadChunkRequestWithUnknownField {
                upload_id: "unknown",
                index: 0,
                data: &[],
                unexpected: true,
            }),
        ),
        (
            20,
            "terminal.upload.finish",
            encode(&UploadIdRequestWithUnknownField {
                upload_id: "unknown",
                unexpected: true,
            }),
        ),
        (
            21,
            "terminal.upload.cancel",
            encode(&UploadIdRequestWithUnknownField {
                upload_id: "unknown",
                unexpected: true,
            }),
        ),
    ] {
        socket
            .send(request(request_id, kind, &payload))
            .await
            .unwrap();
        assert_eq!(
            decode::<ErrorReply>(&response_kind(&mut socket, "error").await.payload).code,
            "protocol.invalid_payload"
        );
    }

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn terminal_upload_disconnect_removes_unfinished_staging_files() {
    let root = private_test_dir("upload-disconnect");
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open(
            TabLaunch::new(
                "Uploads",
                "upload-disconnect",
                TerminalSize::try_new(20, 2).unwrap(),
            )
            .with_cwd(project.to_string_lossy())
            .with_command("sleep 300"),
        )
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    socket
        .send(request(
            1,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    let attached: AttachedReply =
        decode(&response_kind(&mut socket, "terminal.attach").await.payload);
    let _ = response_kind(&mut socket, "terminal.snapshot").await;
    socket
        .send(request(
            2,
            "terminal.focus",
            &encode(&SizedAttachmentRequest {
                tab_id: &tab,
                attachment_id: &attached.attachment_id,
                size: TerminalSize::try_new(20, 2).unwrap(),
            }),
        ))
        .await
        .unwrap();
    let _ = response_kind(&mut socket, "terminal.focus").await;
    let jpeg = test_jpeg();
    let digest = Sha256::digest(&jpeg);
    socket
        .send(request(
            3,
            "terminal.upload.begin",
            &encode(&upload_begin(
                &tab,
                &attached.attachment_id,
                "disconnect",
                &jpeg,
                digest.as_slice(),
            )),
        ))
        .await
        .unwrap();
    let _ = response_kind(&mut socket, "terminal.upload.begin").await;
    let directory = project.join(".aiterm/attachments");
    assert!(std::fs::read_dir(&directory).unwrap().any(|entry| entry
        .unwrap()
        .path()
        .extension()
        .and_then(|value| value.to_str())
        == Some("part")));

    drop(socket);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_dir(&directory).unwrap().all(|entry| {
                entry
                    .unwrap()
                    .path()
                    .extension()
                    .and_then(|value| value.to_str())
                    != Some("part")
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disconnect must cancel every connection-local upload");

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn ninth_attachment_is_rejected_and_teardown_releases_all_eight() {
    let root = private_test_dir("attachment-cap");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab = registry
        .open(TabLaunch::new(
            "Cap",
            "attachment-cap",
            TerminalSize::try_new(20, 2).unwrap(),
        ))
        .unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;
    for request_id in 1..=8 {
        socket
            .send(request(
                request_id,
                "terminal.attach",
                &encode(&TabRequest { tab_id: &tab }),
            ))
            .await
            .unwrap();
        assert_eq!(response(&mut socket).await.kind, "terminal.attach");
        assert_eq!(response(&mut socket).await.kind, "terminal.snapshot");
    }
    assert_eq!(registry.attachment_count(&tab).unwrap(), 8);
    socket
        .send(request(
            9,
            "terminal.attach",
            &encode(&TabRequest { tab_id: &tab }),
        ))
        .await
        .unwrap();
    assert_eq!(
        decode::<ErrorReply>(&response(&mut socket).await.payload).code,
        "terminal.too_many_attachments"
    );
    socket.close(None).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while registry.attachment_count(&tab).unwrap() != 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }

    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

fn cbor_request_with_version(request_id: u64, version: u16, kind: &str, payload: &[u8]) -> Vec<u8> {
    encode(&RequestEnvelope {
        version,
        request_id,
        kind,
        payload,
    })
}

#[tokio::test]
async fn qr_pairing_waits_for_explicit_desktop_approval_before_issuing_device_identity() {
    let root = private_test_dir("pairing");
    let store = Arc::new(DeviceStore::open(root.join("devices")).unwrap());
    let enrollment = store
        .begin_enrollment_at(std::time::SystemTime::now())
        .unwrap();
    let key = SigningKey::random(&mut OsRng);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store.clone(),
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    let _ = challenge(&mut socket).await;

    socket
        .send(Message::Binary(
            encode(&PairRequest {
                kind: "pair.request",
                enrollment_secret: enrollment.secret(),
                device_name: "Matt's phone",
                public_key: key.verifying_key().to_encoded_point(true).as_bytes(),
            })
            .into(),
        ))
        .await
        .unwrap();
    let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
        panic!("pending pairing reply should be binary CBOR");
    };
    let pending: PairPending = decode(&bytes);
    assert_eq!(pending.kind, "pair.pending");
    assert!(store.list_devices().is_empty());

    let device = store
        .approve_pairing_at(&pending.request_id, std::time::SystemTime::now())
        .unwrap();
    assert_eq!(
        device.last_ip.as_deref(),
        Some("127.0.0.1"),
        "desktop approval must retain the pairing socket's peer IP",
    );
    let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
        panic!("approved pairing reply should be binary CBOR");
    };
    let approved: PairApproved = decode(&bytes);
    assert_eq!(approved.kind, "pair.approved");
    assert_eq!(approved.device_id, device.id);

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn websocket_rejects_messages_before_device_nonce_proof() {
    let root = private_test_dir("missing-proof");
    let (store, _, _) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    let _ = challenge(&mut socket).await;

    socket
        .send(Message::Binary(
            b"not an authentication proof".to_vec().into(),
        ))
        .await
        .unwrap();
    assert_closed(&mut socket).await;

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn websocket_rejects_a_frame_larger_than_one_mebibyte() {
    let root = private_test_dir("large-frame");
    let (store, _, _) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    let _ = challenge(&mut socket).await;

    socket
        .send(Message::Binary(vec![0; 1024 * 1024 + 1].into()))
        .await
        .unwrap();
    assert_closed(&mut socket).await;

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn authenticated_connection_rejects_a_replayed_request_id() {
    let root = private_test_dir("replay");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    socket.send(valid_request(7)).await.unwrap();
    socket.send(valid_request(7)).await.unwrap();
    assert_closed(&mut socket).await;

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn authenticated_connection_closes_on_more_than_120_requests_per_second() {
    let root = private_test_dir("rate-limit");
    let (store, key, device_id) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let mut socket = connect(&gateway).await;
    authenticate(&mut socket, &key, &device_id).await;

    let mut send_was_rejected = false;
    for request_id in 1..=121 {
        if socket.send(valid_request(request_id)).await.is_err() {
            send_was_rejected = true;
            break;
        }
    }
    if !send_was_rejected {
        assert_closed(&mut socket).await;
    }

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn tls_twelve_client_cannot_connect() {
    let root = private_test_dir("tls12");
    let (store, _, _) = paired_store(&root);
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store,
        identity,
        services(),
    )
    .await
    .unwrap();
    let url = format!("wss://127.0.0.1:{}/v1/ws", gateway.local_addr().port());

    let result = connect_async_tls_with_config(
        url,
        None,
        true,
        Some(tls_client(
            gateway.certificate_der(),
            &[&rustls::version::TLS12],
        )),
    )
    .await;
    assert!(result.is_err(), "a TLS 1.2-only peer must be rejected");

    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tls_identity_and_pin_survive_a_desktop_restart() {
    let root = private_test_dir("identity");
    let tls_root = root.join("tls");
    let first = TlsIdentity::load_or_create(&tls_root, &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let first_certificate = first.certificate_der().to_vec();
    let first_fingerprint = first.spki_fingerprint().to_string();
    drop(first);

    let second =
        TlsIdentity::load_or_create(&tls_root, &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    assert_eq!(second.certificate_der(), first_certificate);
    assert_eq!(second.spki_fingerprint(), first_fingerprint);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tls_identity_refreshes_certificate_sans_without_rotating_the_spki_pin() {
    let root = private_test_dir("identity-address-refresh");
    let tls_root = root.join("tls");
    let original_address = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99));
    let rebound_address = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 151));

    let first = TlsIdentity::load_or_create(&tls_root, &[original_address]).unwrap();
    let first_fingerprint = first.spki_fingerprint().to_string();
    let first_private_key = std::fs::read(tls_root.join("gateway-key.der")).unwrap();
    drop(first);

    let refreshed =
        TlsIdentity::load_or_create(&tls_root, &[original_address, rebound_address]).unwrap();
    let (_, certificate) = x509_parser::parse_x509_certificate(refreshed.certificate_der())
        .expect("the persisted gateway certificate must remain valid DER");
    let subject_alt_names = certificate
        .subject_alternative_name()
        .expect("the certificate must have one valid SAN extension")
        .expect("the certificate must have a SAN extension");

    assert!(
        subject_alt_names
            .value
            .general_names
            .contains(&GeneralName::IPAddress([10, 0, 0, 151].as_ref())),
        "reissuing for a rebound listener must add the phone-reachable IP SAN"
    );
    assert_eq!(
        refreshed.spki_fingerprint(),
        first_fingerprint,
        "reissuing the certificate must retain the remembered phone's SPKI pin"
    );
    assert_eq!(
        std::fs::read(tls_root.join("gateway-key.der")).unwrap(),
        first_private_key,
        "certificate refresh must not rewrite or rotate the persisted private key"
    );
    assert_eq!(
        std::fs::read(tls_root.join("gateway-cert.der")).unwrap(),
        refreshed.certificate_der(),
        "the validated refreshed certificate must be the durable identity on disk"
    );
    assert_eq!(
        std::fs::read_dir(&tls_root)
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        2,
        "a successful refresh must clean its same-directory temporary file"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(tls_root.join("gateway-cert.der"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "the atomically replaced certificate must remain private"
        );
    }

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tls_identity_rejects_more_than_sixteen_unique_advertised_hosts() {
    let root = private_test_dir("identity-host-bound");
    let hosts = (1..=17)
        .map(|last| IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)))
        .collect::<Vec<_>>();

    let error = TlsIdentity::load_or_create(root.join("tls"), &hosts)
        .err()
        .expect("an unbounded certificate identity must be rejected");
    assert_eq!(error.code(), "gateway.too_many_advertised_hosts");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn mismatched_existing_certificate_and_key_fail_closed_before_san_refresh() {
    let root = private_test_dir("identity-mismatch-refresh");
    let first_root = root.join("first");
    let second_root = root.join("second");
    TlsIdentity::load_or_create(&first_root, &[IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99))])
        .unwrap();
    TlsIdentity::load_or_create(&second_root, &[IpAddr::V4(Ipv4Addr::new(10, 0, 0, 151))]).unwrap();
    let original_certificate = std::fs::read(first_root.join("gateway-cert.der")).unwrap();
    let mismatched_private_key = std::fs::read(second_root.join("gateway-key.der")).unwrap();
    std::fs::write(first_root.join("gateway-key.der"), &mismatched_private_key).unwrap();

    let error = TlsIdentity::load_or_create(
        &first_root,
        &[
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 151)),
        ],
    )
    .err()
    .expect("a mismatched persisted identity must not be repaired by rotating its key");

    assert_eq!(error.code(), "gateway.tls_failed");
    assert_eq!(
        std::fs::read(first_root.join("gateway-cert.der")).unwrap(),
        original_certificate,
        "failed identity validation must not replace the last certificate"
    );
    assert_eq!(
        std::fs::read(first_root.join("gateway-key.der")).unwrap(),
        mismatched_private_key,
        "failed identity validation must never replace the persisted private key"
    );

    std::fs::remove_dir_all(root).ok();
}
