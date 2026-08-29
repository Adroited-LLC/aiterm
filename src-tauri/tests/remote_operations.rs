use aiterm_lib::agents::{AgentChoice, Caps, Detection, ModelOption};
use aiterm_lib::launch::{LaunchPlan, LaunchRequest};
use aiterm_lib::pty::{PtySink, PtySpawnSpec};
use aiterm_lib::remote::auth::DeviceStore;
use aiterm_lib::remote::model::TerminalSize;
use aiterm_lib::remote::server::{RemoteGateway, RemoteServices, TlsIdentity};
use aiterm_lib::services::agents::{AgentOperations, AgentService, AgentServiceError};
use aiterm_lib::services::sessions::{SessionRoots, SessionService};
use aiterm_lib::tabs::{PtyBackend, TabLaunch, TabRegistry, TabState};
use futures_util::{SinkExt, StreamExt};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use rand_core::OsRng;
use rustls::pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::Message, Connector, MaybeTlsStream, WebSocketStream,
};

type TestSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Deserialize)]
struct Challenge {
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
struct RequestEnvelope<'a> {
    version: u16,
    request_id: u64,
    kind: &'a str,
    payload: &'a [u8],
}

#[derive(Deserialize, Serialize)]
struct ResponseEnvelope {
    request_id: u64,
    kind: String,
    payload: Vec<u8>,
}

#[derive(Serialize)]
struct ResponseWire<'a> {
    version: u16,
    request_id: u64,
    kind: &'a str,
    payload: &'a [u8],
}

#[derive(Serialize)]
struct SessionListWire<'a> {
    sessions: &'a [aiterm_lib::sessions::Session],
}

#[derive(Deserialize)]
struct SessionListReply {
    sessions: Vec<SessionReply>,
}

#[derive(Deserialize, Debug, PartialEq, Eq)]
struct SessionReply {
    id: String,
    agent: String,
    title: String,
    project_path: String,
    group_path: String,
    branch: Option<String>,
    forked: bool,
    background: bool,
    fork_parent: Option<String>,
    last_active: u64,
}

#[derive(Deserialize)]
struct ErrorReply {
    code: String,
}

#[derive(Deserialize)]
struct SessionPreviewReply {
    messages: Vec<PreviewReply>,
}

#[derive(Deserialize)]
struct PreviewReply {
    role: String,
    text: String,
}

#[derive(Deserialize)]
struct SessionForkReply {
    session_id: String,
}

#[derive(Serialize)]
struct SessionOpenRequest<'a> {
    session_id: &'a str,
    size: TerminalSize,
}

#[derive(Deserialize)]
struct SessionOpenReply {
    tab_id: String,
    selected_existing: bool,
}

#[derive(Serialize)]
struct SessionCloseRequest<'a> {
    session_id: &'a str,
    tab_id: Option<&'a aiterm_lib::tabs::TabId>,
}

#[derive(Default)]
struct TestPty {
    next_id: AtomicU32,
    sinks: Mutex<std::collections::HashMap<u32, Arc<dyn PtySink>>>,
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
        self.sinks.lock().unwrap().remove(&id);
    }

    fn pty_for_descendant(&self, _pid: u32) -> Option<u32> {
        None
    }
}

impl TestPty {
    fn exit(&self, id: u32) {
        let sink = self.sinks.lock().unwrap().get(&id).cloned().unwrap();
        sink.exited(id, Some(0), None);
    }
}

#[derive(Serialize)]
struct SessionIdPayload<'a> {
    session_id: &'a str,
}

#[derive(Serialize)]
struct SessionIdWithUnknownField<'a> {
    session_id: &'a str,
    recursive: bool,
}

#[derive(Serialize)]
struct AgentActionWithCommand<'a> {
    action: &'a str,
    agent_id: &'a str,
    command: &'a str,
    cwd: &'a str,
    title: &'a str,
    size: TerminalSize,
}

#[derive(Serialize)]
struct AgentStartRequest<'a> {
    action: &'a str,
    agent_id: &'a str,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    cwd: &'a str,
    title: &'a str,
    size: TerminalSize,
}

#[derive(Deserialize)]
struct AgentListReply {
    agents: Vec<AgentChoiceReply>,
    caps: std::collections::HashMap<String, ciborium::value::Value>,
}

#[derive(Deserialize)]
struct AgentChoiceReply {
    id: String,
    display_name: String,
}

#[derive(Deserialize)]
struct AgentStartedReply {
    tab_id: String,
    session_id: Option<String>,
}

struct FixtureAgents;

impl AgentOperations for FixtureAgents {
    fn detect(&self) -> Vec<Detection> {
        Vec::new()
    }

    fn caps(&self) -> std::collections::HashMap<String, Caps> {
        std::collections::HashMap::from([("fixture".into(), Caps::default())])
    }

    fn list(&self) -> Vec<AgentChoice> {
        vec![AgentChoice {
            id: "fixture".into(),
            display_name: "Fixture Agent".into(),
            models: vec![ModelOption {
                id: "fixture-model".into(),
                display_name: "Fixture Model".into(),
                efforts: vec!["medium".into()],
                default_effort: Some("medium".into()),
            }],
            mints_session_id: true,
        }]
    }

    fn resolve(&self, request: LaunchRequest) -> Result<LaunchPlan, AgentServiceError> {
        match request {
            LaunchRequest::Agent { agent_id, .. } if agent_id == "fixture" => Ok(LaunchPlan {
                command: "fixture-agent --safe".into(),
                env_provider: None,
                env_model: None,
                session_id: Some("55555555-5555-4555-8555-555555555555".into()),
                agent_id,
                caps: Caps::default(),
            }),
            LaunchRequest::Resume { session_id } => Ok(LaunchPlan {
                command: format!("fixture-agent --resume {session_id}"),
                env_provider: None,
                env_model: None,
                session_id: Some(session_id),
                agent_id: "fixture".into(),
                caps: Caps::default(),
            }),
            _ => panic!("fixture received an unexpected launch request"),
        }
    }
}

struct Fixture {
    root: PathBuf,
    sessions: PathBuf,
    service: SessionService,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "aiterm-remote-operations-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        let sessions = root.join("sessions");
        std::fs::create_dir_all(sessions.join("project")).unwrap();
        let service = SessionService::from_roots(SessionRoots::new(
            sessions.clone(),
            root.join("trash"),
            root.join("tasks"),
            root.join("jobs"),
            root.join("forks.json"),
        ));
        Self {
            root,
            sessions,
            service,
        }
    }

    fn write_session(&self, id: &str, title: &str, cwd: &str) {
        let transcript = self.sessions.join("project").join(format!("{id}.jsonl"));
        std::fs::write(
            transcript,
            format!(
                "{{\"type\":\"user\",\"uuid\":\"first\",\"parentUuid\":null,\"sessionId\":\"{id}\",\"cwd\":\"{cwd}\",\"message\":{{\"role\":\"user\",\"content\":\"hello\"}}}}\n{{\"type\":\"custom-title\",\"customTitle\":\"{title}\",\"sessionId\":\"{id}\"}}\n"
            ),
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> T {
    ciborium::from_reader(bytes).unwrap()
}

fn paired_store(root: &Path) -> (Arc<DeviceStore>, SigningKey, String) {
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

fn tls_client(cert: &[u8]) -> Connector {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(cert.to_vec())).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Connector::Rustls(Arc::new(config))
}

async fn connect(gateway: &aiterm_lib::remote::server::GatewayHandle) -> TestSocket {
    let url = format!("wss://127.0.0.1:{}/v1/ws", gateway.local_addr().port());
    connect_async_tls_with_config(url, None, true, Some(tls_client(gateway.certificate_der())))
        .await
        .unwrap()
        .0
}

async fn receive(socket: &mut TestSocket) -> ResponseEnvelope {
    let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
        panic!("expected binary response");
    };
    decode(&bytes)
}

async fn receive_correlated(socket: &mut TestSocket, request_id: u64) -> ResponseEnvelope {
    loop {
        let envelope = receive(socket).await;
        if envelope.request_id == request_id {
            return envelope;
        }
    }
}

async fn authenticate(socket: &mut TestSocket, key: &SigningKey, device_id: &str) {
    let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
        panic!("expected authentication challenge");
    };
    let challenge: Challenge = decode(&bytes);
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
        panic!("expected authentication reply");
    };
    let auth: AuthReply = decode(&bytes);
    assert_eq!(auth.kind, "auth.ok");
    assert_eq!(receive(socket).await.kind, "state.snapshot");
}

fn request<T: Serialize>(request_id: u64, kind: &str, payload: &T) -> Message {
    Message::Binary(
        encode(&RequestEnvelope {
            version: 1,
            request_id,
            kind,
            payload: &encode(payload),
        })
        .into(),
    )
}

fn empty_request(request_id: u64, kind: &str) -> Message {
    Message::Binary(
        encode(&RequestEnvelope {
            version: 1,
            request_id,
            kind,
            payload: &[],
        })
        .into(),
    )
}

async fn start(fixture: &Fixture) -> (aiterm_lib::remote::server::GatewayHandle, TestSocket) {
    start_with_registry(fixture, Arc::new(TabRegistry::default())).await
}

async fn start_with_registry(
    fixture: &Fixture,
    registry: Arc<TabRegistry>,
) -> (aiterm_lib::remote::server::GatewayHandle, TestSocket) {
    start_with_services(fixture, registry, AgentService::empty()).await
}

async fn start_with_services(
    fixture: &Fixture,
    registry: Arc<TabRegistry>,
    agents: AgentService,
) -> (aiterm_lib::remote::server::GatewayHandle, TestSocket) {
    let (store, key, device_id) = paired_store(&fixture.root);
    let identity =
        TlsIdentity::load_or_create(fixture.root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)])
            .unwrap();
    let services =
        RemoteServices::with_application_services(registry, fixture.service.clone(), agents);
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
    (gateway, socket)
}

#[tokio::test]
async fn remote_agent_list_matches_the_shared_agent_service() {
    let fixture = Fixture::new("agent-list");
    let agents = AgentService::from_operations(Arc::new(FixtureAgents));
    let (gateway, mut socket) =
        start_with_services(&fixture, Arc::new(TabRegistry::default()), agents).await;

    socket.send(empty_request(15, "agent.list")).await.unwrap();
    let envelope = receive(&mut socket).await;
    let reply: AgentListReply = decode(&envelope.payload);

    assert_eq!(envelope.kind, "agent.list");
    assert_eq!(reply.agents.len(), 1);
    assert_eq!(reply.agents[0].id, "fixture");
    assert_eq!(reply.agents[0].display_name, "Fixture Agent");
    assert!(reply.caps.contains_key("fixture"));
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_agent_start_uses_the_shared_resolver_instead_of_a_client_command() {
    let fixture = Fixture::new("agent-start");
    fixture.write_session(
        "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
        "Project anchor",
        "/fixture/project",
    );
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let agents = AgentService::from_operations(Arc::new(FixtureAgents));
    let (gateway, mut socket) = start_with_services(&fixture, registry.clone(), agents).await;

    socket
        .send(request(
            16,
            "agent.action",
            &AgentStartRequest {
                action: "start",
                agent_id: "fixture",
                model: Some("fixture-model"),
                effort: Some("medium"),
                cwd: "/fixture/project",
                title: "New fixture session",
                size: TerminalSize::try_new(72, 20).unwrap(),
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let reply: AgentStartedReply = decode(&envelope.payload);

    assert_eq!(envelope.kind, "agent.action");
    assert_eq!(
        reply.session_id.as_deref(),
        Some("55555555-5555-4555-8555-555555555555")
    );
    assert_eq!(registry.list()[0].id().as_str(), reply.tab_id);
    assert_eq!(registry.list()[0].command(), Some("fixture-agent --safe"));
    assert_eq!(pty.next_id.load(Ordering::SeqCst), 1);
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_agent_start_rejects_a_cwd_not_exposed_by_the_session_service() {
    let fixture = Fixture::new("agent-cwd");
    fixture.write_session(
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
        "Allowed project",
        "/fixture/project",
    );
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let agents = AgentService::from_operations(Arc::new(FixtureAgents));
    let (gateway, mut socket) = start_with_services(&fixture, registry, agents).await;

    socket
        .send(request(
            24,
            "agent.action",
            &AgentStartRequest {
                action: "start",
                agent_id: "fixture",
                model: None,
                effort: None,
                cwd: "/etc",
                title: "Not allowed",
                size: TerminalSize::try_new(80, 24).unwrap(),
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let error: ErrorReply = decode(&envelope.payload);

    assert_eq!(envelope.kind, "error");
    assert_eq!(error.code, "remote.path_not_allowed");
    assert_eq!(pty.next_id.load(Ordering::SeqCst), 0);
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_open_selects_a_live_tab_before_its_transcript_exists() {
    let fixture = Fixture::new("open-live-tab");
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let session_id = "33333333-3333-4333-8333-333333333333";
    let tab_id = registry
        .open(
            TabLaunch::new(
                "Starting session",
                session_id,
                TerminalSize::try_new(80, 24).unwrap(),
            )
            .with_session_id(session_id)
            .with_command("fixture-agent"),
        )
        .unwrap();
    let (gateway, mut socket) = start_with_registry(&fixture, registry.clone()).await;

    socket
        .send(request(
            12,
            "session.open",
            &SessionOpenRequest {
                session_id,
                size: TerminalSize::try_new(40, 12).unwrap(),
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let opened: SessionOpenReply = decode(&envelope.payload);

    assert_eq!(envelope.kind, "session.open");
    assert_eq!(opened.tab_id, tab_id.as_str());
    assert!(opened.selected_existing);
    assert_eq!(pty.next_id.load(Ordering::SeqCst), 1);
    assert_eq!(registry.list().len(), 1);
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_open_resolves_and_spawns_one_resumed_tab_when_none_is_live() {
    let fixture = Fixture::new("open-resume");
    let session_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    fixture.write_session(session_id, "Resume me", "/fixture/project");
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let agents = AgentService::from_operations(Arc::new(FixtureAgents));
    let (gateway, mut socket) = start_with_services(&fixture, registry.clone(), agents).await;

    socket
        .send(request(
            22,
            "session.open",
            &SessionOpenRequest {
                session_id,
                size: TerminalSize::try_new(90, 30).unwrap(),
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let opened: SessionOpenReply = decode(&envelope.payload);
    let tabs = registry.list();

    assert_eq!(envelope.kind, "session.open");
    assert!(!opened.selected_existing);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].id().as_str(), opened.tab_id);
    assert_eq!(tabs[0].session_id(), Some(session_id));
    assert_eq!(tabs[0].resumed_id(), Some(session_id));
    assert_eq!(tabs[0].cwd(), Some("/fixture/project"));
    assert_eq!(
        tabs[0].command(),
        Some(format!("fixture-agent --resume {session_id}").as_str())
    );
    assert_eq!(pty.next_id.load(Ordering::SeqCst), 1);
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_open_preserves_an_exited_slot_owner_and_spawns_one_running_tab() {
    let fixture = Fixture::new("open-exited-slot");
    let session_id = "abababab-abab-4bab-8bab-abababababab";
    fixture.write_session(session_id, "Resume after exit", "/fixture/project");
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let exited_id = registry
        .open(
            TabLaunch::new(
                "Exited session",
                session_id,
                TerminalSize::try_new(80, 24).unwrap(),
            )
            .with_session_id(session_id)
            .with_command("fixture-agent"),
        )
        .unwrap();
    pty.exit(1);
    assert_eq!(registry.get(&exited_id).unwrap().state(), &TabState::Exited);
    let agents = AgentService::from_operations(Arc::new(FixtureAgents));
    let (gateway, mut socket) = start_with_services(&fixture, registry.clone(), agents).await;

    socket
        .send(request(
            25,
            "session.open",
            &SessionOpenRequest {
                session_id,
                size: TerminalSize::try_new(90, 30).unwrap(),
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let opened: SessionOpenReply = decode(&envelope.payload);
    let tabs = registry.list();

    assert_eq!(envelope.kind, "session.open");
    assert!(!opened.selected_existing);
    assert_eq!(tabs.len(), 2);
    assert_eq!(registry.get(&exited_id).unwrap().state(), &TabState::Exited);
    assert_eq!(
        tabs.iter()
            .find(|tab| tab.id().as_str() == opened.tab_id)
            .unwrap()
            .state(),
        &TabState::Running
    );
    assert_eq!(pty.next_id.load(Ordering::SeqCst), 2);
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_open_validates_an_id_before_matching_a_running_tab() {
    let fixture = Fixture::new("open-invalid-id");
    let invalid_id = "..";
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    registry
        .open(
            TabLaunch::new(
                "Invalid slot",
                invalid_id,
                TerminalSize::try_new(80, 24).unwrap(),
            )
            .with_session_id(invalid_id)
            .with_command("fixture-agent"),
        )
        .unwrap();
    let (gateway, mut socket) = start_with_registry(&fixture, registry).await;

    socket
        .send(request(
            26,
            "session.open",
            &SessionOpenRequest {
                session_id: invalid_id,
                size: TerminalSize::try_new(80, 24).unwrap(),
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let error: ErrorReply = decode(&envelope.payload);

    assert_eq!(envelope.kind, "error");
    assert_eq!(error.code, "session.invalid_id");
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_close_requires_a_tab_id_when_matches_are_ambiguous() {
    let fixture = Fixture::new("close-ambiguous");
    let session_id = "44444444-4444-4444-8444-444444444444";
    fixture.write_session(session_id, "Two tabs", "/fixture/project");
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let size = TerminalSize::try_new(80, 24).unwrap();
    let first = registry
        .open(
            TabLaunch::new("First", session_id, size)
                .with_session_id(session_id)
                .with_command("fixture-agent"),
        )
        .unwrap();
    registry
        .open(
            TabLaunch::new("Second", "other-slot", size)
                .with_resumed_id(session_id)
                .with_command("fixture-agent"),
        )
        .unwrap();
    let (gateway, mut socket) = start_with_registry(&fixture, registry.clone()).await;

    socket
        .send(request(
            13,
            "session.close",
            &SessionCloseRequest {
                session_id,
                tab_id: None,
            },
        ))
        .await
        .unwrap();
    let error: ErrorReply = decode(&receive(&mut socket).await.payload);
    assert_eq!(error.code, "session.tab_ambiguous");
    assert_eq!(registry.list().len(), 2);

    socket
        .send(request(
            14,
            "session.close",
            &SessionCloseRequest {
                session_id,
                tab_id: Some(&first),
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    assert_eq!(envelope.kind, "session.close");
    assert_eq!(registry.list().len(), 1);
    assert!(fixture
        .sessions
        .join("project")
        .join(format!("{session_id}.jsonl"))
        .exists());
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_close_can_close_a_live_tab_before_its_transcript_exists() {
    let fixture = Fixture::new("close-live-tab");
    let session_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let registry = Arc::new(TabRegistry::with_backend(Arc::new(TestPty::default())));
    let tab_id = registry
        .open(
            TabLaunch::new(
                "Starting session",
                session_id,
                TerminalSize::try_new(80, 24).unwrap(),
            )
            .with_session_id(session_id)
            .with_command("fixture-agent"),
        )
        .unwrap();
    let (gateway, mut socket) = start_with_registry(&fixture, registry.clone()).await;

    socket
        .send(request(
            21,
            "session.close",
            &SessionCloseRequest {
                session_id,
                tab_id: Some(&tab_id),
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;

    assert_eq!(envelope.kind, "session.close");
    assert!(registry.list().is_empty());
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_close_ignores_exited_matches_and_closes_only_the_running_tab() {
    let fixture = Fixture::new("close-running-only");
    let session_id = "cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd";
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let size = TerminalSize::try_new(80, 24).unwrap();
    let exited_id = registry
        .open(
            TabLaunch::new("Exited", session_id, size)
                .with_session_id(session_id)
                .with_command("fixture-agent"),
        )
        .unwrap();
    pty.exit(1);
    let (gateway, mut socket) = start_with_registry(&fixture, registry.clone()).await;

    socket
        .send(request(
            27,
            "session.close",
            &SessionCloseRequest {
                session_id,
                tab_id: None,
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let error: ErrorReply = decode(&envelope.payload);
    assert_eq!(envelope.kind, "error");
    assert_eq!(error.code, "session.tab_not_found");

    let running_id = registry
        .open(
            TabLaunch::new("Running", "running-slot", size)
                .with_resumed_id(session_id)
                .with_command("fixture-agent"),
        )
        .unwrap();
    socket
        .send(request(
            28,
            "session.close",
            &SessionCloseRequest {
                session_id,
                tab_id: None,
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_correlated(&mut socket, 28).await.kind,
        "session.close"
    );
    assert!(registry.get(&running_id).is_err());
    assert_eq!(registry.get(&exited_id).unwrap().state(), &TabState::Exited);
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_list_matches_desktop_service_result() {
    let fixture = Fixture::new("list-parity");
    fixture.write_session(
        "11111111-1111-4111-8111-111111111111",
        "Fixture session",
        "/fixture/project",
    );
    let desktop = fixture.service.list().unwrap();
    let (gateway, mut socket) = start(&fixture).await;

    socket.send(empty_request(7, "session.list")).await.unwrap();
    let envelope = receive(&mut socket).await;
    let remote: SessionListReply = decode(&envelope.payload);

    assert_eq!(envelope.request_id, 7);
    assert_eq!(envelope.kind, "session.list");
    assert_eq!(remote.sessions.len(), desktop.len());
    assert_eq!(remote.sessions[0].id, desktop[0].id);
    assert_eq!(remote.sessions[0].agent, desktop[0].agent);
    assert_eq!(remote.sessions[0].title, desktop[0].title);
    assert_eq!(remote.sessions[0].project_path, desktop[0].project_path);
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn oversized_session_lists_return_a_bounded_correlated_error() {
    let fixture = Fixture::new("list-bound");
    fixture.write_session(
        "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        &"x".repeat(1024 * 1024),
        "/fixture/project",
    );
    let (gateway, mut socket) = start(&fixture).await;

    socket
        .send(empty_request(23, "session.list"))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let error: ErrorReply = decode(&envelope.payload);

    assert_eq!(envelope.request_id, 23);
    assert_eq!(envelope.kind, "error");
    assert_eq!(error.code, "protocol.response_too_large");
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn session_list_reserves_space_for_the_complete_response_envelope() {
    let fixture = Fixture::new("list-envelope-bound");
    let session_id = "dededede-dede-4ede-8ede-dededededede";
    fixture.write_session(session_id, "", "/fixture/project");
    let mut session = fixture.service.find(session_id).unwrap();
    let limit = aiterm_lib::terminal::MAX_SCREEN_FRAME_BYTES;
    let mut low = 0usize;
    let mut high = limit;
    let mut boundary_title = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        session.title = "x".repeat(middle);
        let nested = encode(&SessionListWire {
            sessions: std::slice::from_ref(&session),
        });
        let outer = encode(&ResponseWire {
            version: 1,
            request_id: 29,
            kind: "session.list",
            payload: &nested,
        });
        if nested.len() < limit {
            if outer.len() >= limit {
                boundary_title = Some(session.title.clone());
            }
            low = middle + 1;
        } else {
            high = middle.saturating_sub(1);
        }
    }
    let boundary_title = boundary_title.expect("CBOR envelope has measurable overhead");
    fixture.write_session(session_id, &boundary_title, "/fixture/project");
    let (gateway, mut socket) = start(&fixture).await;

    socket
        .send(empty_request(29, "session.list"))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let error: ErrorReply = decode(&envelope.payload);

    assert_eq!(envelope.request_id, 29);
    assert_eq!(envelope.kind, "error");
    assert_eq!(error.code, "protocol.response_too_large");
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_preview_uses_the_same_bounded_service_shape() {
    let fixture = Fixture::new("preview");
    let session_id = "66666666-6666-4666-8666-666666666666";
    fixture.write_session(session_id, "Preview me", "/fixture/project");
    let (gateway, mut socket) = start(&fixture).await;

    socket
        .send(request(
            17,
            "session.preview",
            &SessionIdPayload { session_id },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let preview: SessionPreviewReply = decode(&envelope.payload);

    assert_eq!(envelope.kind, "session.preview");
    assert_eq!(preview.messages.len(), 1);
    assert_eq!(preview.messages[0].role, "user");
    assert_eq!(preview.messages[0].text, "hello");
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_fork_creates_a_service_visible_branch_under_the_fixture_root() {
    let fixture = Fixture::new("fork");
    let session_id = "77777777-7777-4777-8777-777777777777";
    fixture.write_session(session_id, "Fork me", "/fixture/project");
    let (gateway, mut socket) = start(&fixture).await;

    socket
        .send(request(
            18,
            "session.fork",
            &SessionIdPayload { session_id },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let forked: SessionForkReply = decode(&envelope.payload);

    assert_eq!(envelope.kind, "session.fork");
    let branch = fixture.service.find(&forked.session_id).unwrap();
    assert_eq!(branch.fork_parent.as_deref(), Some(session_id));
    assert!(fixture
        .sessions
        .join("project")
        .join(format!("{}.jsonl", forked.session_id))
        .exists());
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_delete_moves_only_the_requested_fixture_session() {
    let fixture = Fixture::new("delete-known");
    let deleted_id = "88888888-8888-4888-8888-888888888888";
    let kept_id = "99999999-9999-4999-8999-999999999998";
    fixture.write_session(deleted_id, "Delete me", "/fixture/project");
    fixture.write_session(kept_id, "Keep me", "/fixture/project");
    let (gateway, mut socket) = start(&fixture).await;

    socket
        .send(request(
            19,
            "session.delete",
            &SessionIdPayload {
                session_id: deleted_id,
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;

    assert_eq!(envelope.kind, "session.delete");
    assert!(fixture
        .root
        .join("trash")
        .join(format!("{deleted_id}.jsonl"))
        .exists());
    assert!(fixture.service.find(deleted_id).is_err());
    assert_eq!(fixture.service.find(kept_id).unwrap().title, "Keep me");
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_session_stop_is_idempotent_for_an_already_stopped_fixture_session() {
    let fixture = Fixture::new("stop");
    let session_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    fixture.write_session(session_id, "Stopped", "/fixture/project");
    let before = snapshot_tree(&fixture.root);
    let (gateway, mut socket) = start(&fixture).await;

    socket
        .send(request(
            20,
            "session.stop",
            &SessionIdPayload { session_id },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;

    assert_eq!(envelope.kind, "session.stop");
    assert_eq!(snapshot_tree(&fixture.root), before);
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn remote_delete_rejects_an_unknown_session_without_touching_disk() {
    let fixture = Fixture::new("unknown-delete");
    fixture.write_session(
        "22222222-2222-4222-8222-222222222222",
        "Keep me",
        "/fixture/project",
    );
    let before = snapshot_tree(&fixture.root);
    let (gateway, mut socket) = start(&fixture).await;

    socket
        .send(request(
            8,
            "session.delete",
            &SessionIdPayload {
                session_id: "99999999-9999-4999-8999-999999999999",
            },
        ))
        .await
        .unwrap();
    let envelope = receive(&mut socket).await;
    let error: ErrorReply = decode(&envelope.payload);

    assert_eq!(envelope.request_id, 8);
    assert_eq!(envelope.kind, "error");
    assert_eq!(error.code, "session.not_found");
    assert_eq!(snapshot_tree(&fixture.root), before);
    gateway.stop().await.unwrap();
}

#[cfg(unix)]
#[test]
fn rooted_session_service_rejects_file_and_directory_symlink_escapes() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("symlink-containment");
    let outside = fixture.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let file_id = "efefefef-efef-4fef-8fef-efefefefefef";
    let directory_id = "fefefefe-fefe-4efe-8efe-fefefefefefe";
    let external_file = outside.join(format!("{file_id}.jsonl"));
    std::fs::write(
        &external_file,
        format!("{{\"sessionId\":\"{file_id}\",\"cwd\":\"/outside\"}}\n"),
    )
    .unwrap();
    let external_directory = outside.join("directory");
    std::fs::create_dir_all(&external_directory).unwrap();
    let external_nested = external_directory.join(format!("{directory_id}.jsonl"));
    std::fs::write(
        &external_nested,
        format!("{{\"sessionId\":\"{directory_id}\",\"cwd\":\"/outside\"}}\n"),
    )
    .unwrap();
    symlink(
        &external_file,
        fixture
            .sessions
            .join("project")
            .join(format!("{file_id}.jsonl")),
    )
    .unwrap();
    symlink(
        &external_directory,
        fixture.sessions.join("linked-directory"),
    )
    .unwrap();
    let before_file = std::fs::read(&external_file).unwrap();
    let before_nested = std::fs::read(&external_nested).unwrap();

    let listed = fixture.service.list().unwrap();
    assert!(listed.iter().all(|session| session.id != file_id));
    assert!(listed.iter().all(|session| session.id != directory_id));
    assert_eq!(
        fixture.service.preview(file_id).err().unwrap().code(),
        "session.not_found"
    );
    assert_eq!(
        fixture.service.delete(file_id).unwrap_err().code(),
        "session.not_found"
    );
    assert_eq!(
        fixture.service.delete(directory_id).unwrap_err().code(),
        "session.not_found"
    );
    assert_eq!(std::fs::read(&external_file).unwrap(), before_file);
    assert_eq!(std::fs::read(&external_nested).unwrap(), before_nested);
}

#[tokio::test]
async fn operation_payloads_reject_unknown_fields_and_arbitrary_commands() {
    let fixture = Fixture::new("strict-payloads");
    let (gateway, mut socket) = start(&fixture).await;

    socket
        .send(request(
            9,
            "session.delete",
            &SessionIdWithUnknownField {
                session_id: "11111111-1111-4111-8111-111111111111",
                recursive: true,
            },
        ))
        .await
        .unwrap();
    let error: ErrorReply = decode(&receive(&mut socket).await.payload);
    assert_eq!(error.code, "protocol.invalid_payload");

    socket
        .send(request(
            10,
            "agent.action",
            &AgentActionWithCommand {
                action: "start",
                agent_id: "claude",
                command: "rm -rf /",
                cwd: "/fixture/project",
                title: "unsafe",
                size: TerminalSize::try_new(80, 24).unwrap(),
            },
        ))
        .await
        .unwrap();
    let error: ErrorReply = decode(&receive(&mut socket).await.payload);
    assert_eq!(error.code, "protocol.invalid_payload");
    gateway.stop().await.unwrap();
}

#[tokio::test]
async fn desktop_only_and_unknown_operations_return_remote_unsupported() {
    let fixture = Fixture::new("unsupported");
    let (gateway, mut socket) = start(&fixture).await;

    for (request_id, kind) in [
        (11, "settings.write"),
        (12, "filesystem.write_anywhere"),
        (13, "font.install"),
        (14, "diagnostics.toggle"),
        (15, "unknown.command"),
    ] {
        socket.send(empty_request(request_id, kind)).await.unwrap();
        let envelope = receive(&mut socket).await;
        let error: ErrorReply = decode(&envelope.payload);
        assert_eq!(envelope.request_id, request_id);
        assert_eq!(envelope.kind, "error");
        assert_eq!(error.code, "remote.unsupported");
    }
    gateway.stop().await.unwrap();
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(root: &Path, path: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                walk(root, &entry, out);
            } else if !entry.components().any(|part| part.as_os_str() == "devices")
                && !entry.components().any(|part| part.as_os_str() == "tls")
            {
                out.push((
                    entry.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&entry).unwrap(),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}
