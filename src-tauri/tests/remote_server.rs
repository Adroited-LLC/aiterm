use aiterm_lib::remote::auth::DeviceStore;
use aiterm_lib::remote::model::TerminalSize;
use aiterm_lib::remote::server::{RemoteGateway, RemoteServices, TlsIdentity};
use aiterm_lib::tabs::{TabLaunch, TabRegistry};
use futures_util::{SinkExt, StreamExt};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use rand_core::OsRng;
use rustls::pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::Message, Connector, MaybeTlsStream, WebSocketStream,
};

type TestSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

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
struct ErrorReply {
    code: String,
}

#[derive(Serialize)]
struct TabRequest<'a> {
    tab_id: &'a aiterm_lib::tabs::TabId,
}

#[derive(Deserialize)]
struct AttachedReply {
    tab_id: String,
    attachment_id: String,
    has_focus: bool,
}

#[derive(Deserialize)]
struct SnapshotChunkReply {
    tab_id: String,
    attachment_id: Option<String>,
    kind: String,
    index: u32,
    total: u32,
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

async fn authenticate(socket: &mut TestSocket, key: &SigningKey, device_id: &str) {
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
async fn session_requests_receive_structured_unsupported_responses() {
    let root = private_test_dir("unsupported-dispatch");
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

    socket.send(request(73, "session.list", b"")).await.unwrap();
    let reply = response(&mut socket).await;

    assert_eq!(reply.request_id, 73);
    assert_eq!(reply.kind, "error");
    let payload: ErrorReply = decode(&reply.payload);
    assert_eq!(payload.code, "protocol.unsupported_request");
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
