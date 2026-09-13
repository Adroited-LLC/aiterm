use super::*;
use crate::pty::{PtySink, PtySpawnSpec};
use crate::remote::{
    auth::DeviceStore,
    model::TerminalSize,
    server::{RemoteGateway, RemoteServices, TlsIdentity},
};
use crate::tabs::{PtyBackend, TabLaunch, TabRegistry};
use rand_core::OsRng;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[derive(Default)]
struct TestPty {
    sink: Mutex<Option<Arc<dyn PtySink>>>,
    writes: Mutex<Vec<Vec<u8>>>,
}
impl PtyBackend for TestPty {
    fn spawn(&self, _: PtySpawnSpec, sink: Arc<dyn PtySink>) -> Result<u32, String> {
        *self.sink.lock().unwrap() = Some(sink);
        Ok(1)
    }
    fn write(&self, _: u32, bytes: &[u8]) -> Result<(), String> {
        self.writes.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
    fn resize(&self, _: u32, _: u16, _: u16) -> Result<(), String> {
        Ok(())
    }
    fn kill(&self, _: u32) {}
    fn pty_for_descendant(&self, _: u32) -> Option<u32> {
        None
    }
}
fn test_shared() -> Arc<Shared> {
    Arc::new(Shared {
        view: Mutex::new(ClientView {
            connection: "connected".into(),
            epoch: 1,
            ..Default::default()
        }),
        changed: Notify::new(),
        pairing_cancel: Notify::new(),
        generation: AtomicU64::new(1),
        task: tokio::sync::Mutex::new(None),
        sender: Mutex::new(None),
        store_lock: tokio::sync::Mutex::new(()),
    })
}
async fn drain_until(connection: &mut Connection, ready: impl Fn(&Connection) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready(connection) {
            let bytes = transport::receive(&mut connection.socket).await.unwrap();
            connection.frame(&bytes).await.unwrap();
        }
    })
    .await
    .expect("gateway did not reach the expected state");
}
fn job(
    connection: &Connection,
    action: Action,
) -> (
    Job,
    oneshot::Receiver<Result<Option<Vec<crate::terminal::model::ScreenRow>>, String>>,
) {
    let (reply, result) = oneshot::channel();
    let view = connection.shared.view.lock().unwrap().clone();
    (
        Job {
            generation: connection.generation,
            epoch: connection.epoch,
            target: view.selected_tab.clone(),
            attachment_id: view.attachment_id.clone(),
            action,
            reply,
        },
        result,
    )
}
#[tokio::test]
async fn real_gateway_auth_snapshot_focus_input_scrollback_and_reconnect() {
    let root = std::env::temp_dir().join(format!("aiterm-desktop-client-{}", uuid::Uuid::new_v4()));
    let store = Arc::new(DeviceStore::open(root.join("devices")).unwrap());
    let key = SigningKey::random(&mut OsRng);
    let now = std::time::SystemTime::now();
    let invite = store.begin_enrollment_at(now).unwrap();
    let device = store
        .approve_at(
            invite.secret(),
            "Test desktop",
            key.verifying_key().to_encoded_point(true).as_bytes(),
            now,
        )
        .unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    let tab = registry
        .open_desktop(TabLaunch::new(
            "Remote test",
            "echo test",
            TerminalSize::try_new(40, 8).unwrap(),
        ))
        .unwrap();
    pty.sink.lock().unwrap().as_ref().unwrap().output(
        1,
        b"earlier\r\nline2\r\nline3\r\nline4\r\nline5\r\nline6\r\nline7\r\nline8\r\nready> ",
    );
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store.clone(),
        identity,
        RemoteServices::new(registry.clone()),
    )
    .await
    .unwrap();
    let desktop = Desktop {
        id: "test".into(),
        name: "Test desktop".into(),
        hosts: vec!["127.0.0.1".into()],
        port: gateway.local_addr().port(),
        fingerprint: gateway.spki_fingerprint().into(),
        relay: None,
        device_id: device.id,
        key: key.to_bytes().to_vec(),
    };
    let mut socket = transport::open(&desktop).await.unwrap();
    transport::authenticate(&mut socket, &desktop)
        .await
        .unwrap();
    let shared = test_shared();
    let mut connection = Connection {
        socket,
        next_id: 0,
        pending: HashMap::new(),
        attachment: None,
        assembler: None,
        shared: shared.clone(),
        generation: 1,
        epoch: 1,
        statuses: HashMap::new(),
        status_supported: true,
        agents_loaded: false,
    };
    connection.refresh().await.unwrap();
    drain_until(&mut connection, |c| {
        !c.shared.view.lock().unwrap().tabs.is_empty()
    })
    .await;
    connection.attach(tab.as_str().into(), None).await.unwrap();
    drain_until(&mut connection, |c| {
        c.shared.view.lock().unwrap().screen.is_some()
    })
    .await;
    assert!(!shared.view.lock().unwrap().has_focus);
    let (write, result) = job(&connection, Action::Input("blocked".into()));
    connection.job(write).await.unwrap();
    assert!(result.await.unwrap().is_err());
    assert!(pty.writes.lock().unwrap().is_empty());
    let (focus, _focus_result) = job(&connection, Action::Focus);
    connection.job(focus).await.unwrap();
    drain_until(&mut connection, |c| c.shared.view.lock().unwrap().has_focus).await;
    let (write, result) = job(&connection, Action::Input("hello\r".into()));
    connection.job(write).await.unwrap();
    drain_until(&mut connection, |c| {
        !c.pending.values().any(|p| p.kind == "terminal.input")
    })
    .await;
    assert!(result.await.unwrap().is_ok());
    assert_eq!(
        pty.writes.lock().unwrap().as_slice(),
        &[b"hello\r".to_vec()]
    );
    let revision = shared
        .view
        .lock()
        .unwrap()
        .screen
        .as_ref()
        .unwrap()
        .revision();
    pty.sink
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .output(1, b"new remote output");
    drain_until(&mut connection, |c| {
        c.shared
            .view
            .lock()
            .unwrap()
            .screen
            .as_ref()
            .is_some_and(|s| s.revision() != revision)
    })
    .await;
    let (page, result) = job(&connection, Action::Scrollback(0));
    connection.job(page).await.unwrap();
    drain_until(&mut connection, |c| {
        !c.pending.values().any(|p| p.kind == "terminal.scrollback")
    })
    .await;
    assert!(result.await.unwrap().is_ok());
    assert!(!shared.view.lock().unwrap().scrollback.is_empty());
    let (resize, result) = job(&connection, Action::Resize(50, 10));
    connection.job(resize).await.unwrap();
    drain_until(&mut connection, |c| {
        c.shared
            .view
            .lock()
            .unwrap()
            .screen
            .as_ref()
            .is_some_and(|s| s.cols() == 50 && s.rows() == 10)
    })
    .await;
    drain_until(&mut connection, |c| {
        !c.pending.values().any(|p| p.kind == "terminal.resize")
    })
    .await;
    assert!(result.await.unwrap().is_ok());
    let (mut stale, result) = job(&connection, Action::Input("never".into()));
    stale.epoch = 0;
    connection.job(stale).await.unwrap();
    assert!(result.await.unwrap().is_err());
    let (mut stale, result) = job(&connection, Action::Input("never".into()));
    stale.generation = 0;
    connection.job(stale).await.unwrap();
    assert!(result.await.unwrap().is_err());
    let (mut stale, result) = job(&connection, Action::Input("never".into()));
    stale.target = Some("other tab".into());
    connection.job(stale).await.unwrap();
    assert!(result.await.unwrap().is_err());
    assert_eq!(pty.writes.lock().unwrap().len(), 1);
    // Giving control back reattaches read-only, without ending the terminal.
    let old_attachment = connection.attachment.clone();
    let (history, history_result) = job(&connection, Action::Scrollback(0));
    connection.job(history).await.unwrap();
    let (queued, queued_result) = job(&connection, Action::Input("stale after handoff".into()));
    connection.attach(tab.as_str().into(), None).await.unwrap();
    drain_until(&mut connection, |c| {
        c.attachment.is_some() && c.attachment != old_attachment
    })
    .await;
    assert!(history_result.await.unwrap().is_err());
    assert!(!connection
        .pending
        .values()
        .any(|p| p.kind == "terminal.scrollback"));
    assert!(!shared.view.lock().unwrap().has_focus);
    assert!(registry.get(&tab).unwrap().input_owner().is_none());
    let (focus, _result) = job(&connection, Action::Focus);
    connection.job(focus).await.unwrap();
    drain_until(&mut connection, |c| c.shared.view.lock().unwrap().has_focus).await;
    connection.job(queued).await.unwrap();
    assert!(queued_result.await.unwrap().is_err());
    assert_eq!(pty.writes.lock().unwrap().len(), 1);
    connection.socket.close(None).await.unwrap();
    drop(connection);
    let mut socket = transport::open(&desktop).await.unwrap();
    transport::authenticate(&mut socket, &desktop)
        .await
        .unwrap();
    shared.publish(1, |v| {
        v.screen = None;
        v.has_focus = false;
    });
    let mut connection = Connection {
        socket,
        next_id: 0,
        pending: HashMap::new(),
        attachment: None,
        assembler: None,
        shared,
        generation: 1,
        epoch: 2,
        statuses: HashMap::new(),
        status_supported: true,
        agents_loaded: false,
    };
    connection.attach(tab.as_str().into(), None).await.unwrap();
    drain_until(&mut connection, |c| {
        c.shared.view.lock().unwrap().screen.is_some()
    })
    .await;
    assert!(
        !connection.shared.view.lock().unwrap().has_focus,
        "reconnect must not reclaim input"
    );
    let attachment = connection.attachment.as_ref().unwrap().1.clone();
    let (typed, result) = job(
        &connection,
        Action::Type("first typed input".into(), 45, 12, attachment),
    );
    connection.job(typed).await.unwrap();
    drain_until(&mut connection, |c| {
        !c.pending.values().any(|p| p.kind == "terminal.input")
    })
    .await;
    assert!(result.await.unwrap().is_ok());
    drain_until(&mut connection, |c| c.shared.view.lock().unwrap().has_focus).await;
    assert_eq!(
        pty.writes.lock().unwrap().last().unwrap(),
        b"first typed input"
    );
    let (stale, result) = job(
        &connection,
        Action::Type("wrong attachment".into(), 45, 12, "old".into()),
    );
    connection.job(stale).await.unwrap();
    assert!(result.await.unwrap().is_err());
    connection.socket.close(None).await.ok();
    drop(connection);
    registry.close(&tab).ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn pairing_requires_host_approval_then_uses_the_saved_key() {
    let root = std::env::temp_dir().join(format!("aiterm-client-pair-{}", uuid::Uuid::new_v4()));
    let store = Arc::new(DeviceStore::open(root.join("devices")).unwrap());
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store.clone(),
        identity,
        RemoteServices::new(Arc::new(TabRegistry::default())),
    )
    .await
    .unwrap();
    let enrollment = store
        .begin_enrollment_at(std::time::SystemTime::now())
        .unwrap();
    let uri = crate::remote::pairing_payload(
        &["127.0.0.1".parse().unwrap()],
        gateway.local_addr().port(),
        gateway.spki_fingerprint(),
        enrollment.secret(),
        "Other desktop",
    );
    let invite = PairingUri::parse(&uri).unwrap();
    let mut desktop = Desktop::from_invite(&invite).unwrap();
    let pair = enroll(&mut desktop, &invite, "Client desktop");
    let approve = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let pending = store.list_pending_pairings();
                if let Some(pending) = pending.first() {
                    assert_eq!(pending.name, "Client desktop");
                    assert!(store.list_devices().is_empty());
                    store
                        .approve_pairing_at(&pending.id, std::time::SystemTime::now())
                        .unwrap();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    };
    let (paired, _) = tokio::join!(pair, approve);
    paired.unwrap();
    assert!(!desktop.device_id.is_empty());
    let mut socket = transport::open(&desktop).await.unwrap();
    transport::authenticate(&mut socket, &desktop)
        .await
        .unwrap();
    socket.close(None).await.ok();
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}

struct ResumeAgent;
impl crate::services::agents::AgentOperations for ResumeAgent {
    fn detect(&self) -> Vec<crate::agents::Detection> {
        vec![]
    }
    fn caps(&self) -> HashMap<String, crate::agents::Caps> {
        HashMap::new()
    }
    fn list(&self) -> Vec<crate::agents::AgentChoice> {
        vec![]
    }
    fn resolve(
        &self,
        request: crate::launch::LaunchRequest,
    ) -> Result<crate::launch::LaunchPlan, crate::services::agents::AgentServiceError> {
        let crate::launch::LaunchRequest::Resume { session_id } = request else {
            panic!("only resume expected");
        };
        Ok(crate::launch::LaunchPlan {
            command: "fixture-resume".into(),
            env_provider: None,
            env_model: None,
            session_id: Some(session_id),
            agent_id: "claude".into(),
            caps: crate::agents::Caps::default(),
        })
    }
}
#[tokio::test]
async fn manager_previews_resumes_existing_history_and_keeps_request_errors_connected() {
    let root = std::env::temp_dir().join(format!("aiterm-desktop-client-{}", uuid::Uuid::new_v4()));
    let store = Arc::new(DeviceStore::open(root.join("devices")).unwrap());
    let key = SigningKey::random(&mut OsRng);
    let now = std::time::SystemTime::now();
    let invite = store.begin_enrollment_at(now).unwrap();
    let device = store
        .approve_at(
            invite.secret(),
            "Test desktop",
            key.verifying_key().to_encoded_point(true).as_bytes(),
            now,
        )
        .unwrap();
    let pty = Arc::new(TestPty::default());
    let registry = Arc::new(TabRegistry::with_backend(pty.clone()));
    use crate::services::sessions::{SessionRoots, SessionService};
    let session_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    let sessions = root.join("sessions");
    std::fs::create_dir_all(sessions.join("project")).unwrap();
    std::fs::write(sessions.join("project").join(format!("{session_id}.jsonl")), format!("{}\n", json!({"type":"user","uuid":"first","parentUuid":null,"sessionId":session_id,"cwd":"/fixture/project","message":{"role":"user","content":"Saved conversation"}}))).unwrap();
    let service = SessionService::from_roots(SessionRoots::new(
        sessions,
        root.join("trash"),
        root.join("tasks"),
        root.join("jobs"),
        root.join("forks.json"),
    ));
    let identity =
        TlsIdentity::load_or_create(root.join("tls"), &[IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap();
    let gateway = RemoteGateway::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        store.clone(),
        identity,
        RemoteServices::with_application_services(
            registry.clone(),
            service.clone(),
            crate::services::agents::AgentService::from_operations(Arc::new(ResumeAgent)),
        ),
    )
    .await
    .unwrap();
    let desktop = Desktop {
        id: "test".into(),
        name: "Test desktop".into(),
        hosts: vec!["127.0.0.1".into()],
        port: gateway.local_addr().port(),
        fingerprint: gateway.spki_fingerprint().into(),
        relay: None,
        device_id: device.id,
        key: key.to_bytes().to_vec(),
    };
    let mut socket = transport::open(&desktop).await.unwrap();
    transport::authenticate(&mut socket, &desktop)
        .await
        .unwrap();
    let shared = test_shared();
    let mut connection = Connection {
        socket,
        next_id: 0,
        pending: HashMap::new(),
        attachment: None,
        assembler: None,
        shared: shared.clone(),
        generation: 1,
        epoch: 1,
        statuses: HashMap::new(),
        status_supported: true,
        agents_loaded: false,
    };

    connection.refresh().await.unwrap();
    drain_until(&mut connection, |c| {
        !c.shared.view.lock().unwrap().sessions.is_empty()
    })
    .await;
    let (preview, result) = job(
        &connection,
        Action::Session("preview".into(), session_id.into(), None, None),
    );
    connection.job(preview).await.unwrap();
    drain_until(&mut connection, |c| {
        !c.shared.view.lock().unwrap().preview_loading
    })
    .await;
    assert!(result.await.unwrap().is_ok());
    assert!(
        registry.list().is_empty(),
        "preview must not start an agent"
    );
    assert_eq!(
        shared.view.lock().unwrap().preview_session.as_deref(),
        Some(session_id)
    );
    // The rich conversation reader uses the application catalog; inject its
    // bounded reply so the fixture never reads or writes real user history.
    connection.next_id += 1;
    let id = connection.next_id;
    connection.pending.insert(
        id,
        Pending {
            kind: "session.conversation".into(),
            session_id: Some(session_id.into()),
            started: Instant::now(),
            job: None,
        },
    );
    connection
        .frame(
            &transport::encode(&Frame {
                version: 1,
                request_id: id,
                kind: "session.conversation".into(),
                payload: transport::encode(
                    &json!({"messages":[{"role":"user","text":"Saved conversation"}]}),
                )
                .unwrap(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        shared.view.lock().unwrap().preview_messages[0]["text"],
        "Saved conversation"
    );
    for _ in 0..2 {
        let (open, result) = job(
            &connection,
            Action::Session("open".into(), session_id.into(), None, None),
        );
        connection.job(open).await.unwrap();
        drain_until(&mut connection, |c| {
            !c.pending
                .values()
                .any(|p| p.kind == "session.open" || p.kind == "terminal.attach")
        })
        .await;
        assert!(result.await.unwrap().is_ok());
        drain_until(&mut connection, |c| {
            c.shared.view.lock().unwrap().screen.is_some()
        })
        .await;
        assert_eq!(
            registry.list().len(),
            1,
            "reopen must reuse the existing tab"
        );
        assert!(shared.view.lock().unwrap().preview_session.is_none());
        assert!(
            !shared.view.lock().unwrap().has_focus,
            "reading does not take control"
        );
    }
    // A missing session is an action error, not a transport failure.
    let (bad, result) = job(
        &connection,
        Action::Session("open".into(), "missing-session".into(), None, None),
    );
    connection.job(bad).await.unwrap();
    drain_until(&mut connection, |c| {
        !c.pending.values().any(|p| p.kind == "session.open")
    })
    .await;
    assert!(result.await.unwrap().is_err());
    assert_eq!(shared.view.lock().unwrap().connection, "connected");
    let (delete, result) = job(
        &connection,
        Action::Session("delete".into(), session_id.into(), None, None),
    );
    connection.job(delete).await.unwrap();
    assert!(
        result.await.unwrap().is_err(),
        "live sessions cannot be deleted"
    );
    assert!(service.find(session_id).is_ok());
    connection.socket.close(None).await.ok();
    drop(connection);
    gateway.stop().await.unwrap();
    std::fs::remove_dir_all(root).ok();
}
