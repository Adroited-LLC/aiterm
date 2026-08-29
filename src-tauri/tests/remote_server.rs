use aiterm_lib::pty::{PtySink, PtySpawnSpec};
use aiterm_lib::remote::auth::DeviceStore;
use aiterm_lib::remote::model::TerminalSize;
use aiterm_lib::remote::server::{
    RemoteGateway, RemoteServices, TlsIdentity, MAX_SCROLLBACK_PAGE_ROWS, MAX_TERMINAL_INPUT_BYTES,
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
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::Message, Connector, MaybeTlsStream, WebSocketStream,
};

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
    revision: u64,
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
        store,
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
            .with_command("sleep 5"),
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
