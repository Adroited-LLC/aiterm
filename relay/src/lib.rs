use aiterm_relay_protocol::Frame;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinSet;
use tokio::time::{timeout, Duration};

const CLIENT_HELLO_LIMIT: usize = 64 * 1024;
const CLIENT_HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECTOR_QUEUE: usize = 256;
const STREAM_QUEUE: usize = 32;
const MAX_STREAMS_PER_CONNECTOR: usize = 128;

#[derive(Clone, Debug, Deserialize)]
pub struct RelayConfig {
    pub connector_listen: SocketAddr,
    pub ingress_listen: SocketAddr,
    pub public_domain: String,
    pub routes: Vec<RouteConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RouteConfig {
    pub id: String,
    pub token_sha256: String,
}

#[derive(Clone)]
struct RouteAuth {
    token_sha256: [u8; 32],
}

#[derive(Clone)]
struct Connector {
    generation: u64,
    outgoing: mpsc::Sender<Frame>,
    streams: Arc<Mutex<HashMap<u64, mpsc::Sender<Vec<u8>>>>>,
}

#[derive(Clone)]
struct RelayState {
    public_domain: Arc<str>,
    routes: Arc<HashMap<String, RouteAuth>>,
    connectors: Arc<Mutex<HashMap<String, Connector>>>,
    next_generation: Arc<AtomicU64>,
    next_stream: Arc<AtomicU64>,
}

impl RelayState {
    fn from_config(config: &RelayConfig) -> Result<Self, String> {
        let public_domain = normalize_dns_name(&config.public_domain)
            .ok_or_else(|| "public_domain is not a valid DNS name".to_string())?;
        let mut routes = HashMap::new();
        for route in &config.routes {
            if !valid_route_id(&route.id) || routes.contains_key(&route.id) {
                return Err(format!("invalid or duplicate route id {}", route.id));
            }
            let token_sha256 = decode_hex_32(&route.token_sha256)
                .ok_or_else(|| format!("route {} has an invalid token hash", route.id))?;
            routes.insert(route.id.clone(), RouteAuth { token_sha256 });
        }
        Ok(Self {
            public_domain: public_domain.into(),
            routes: Arc::new(routes),
            connectors: Arc::new(Mutex::new(HashMap::new())),
            next_generation: Arc::new(AtomicU64::new(1)),
            next_stream: Arc::new(AtomicU64::new(1)),
        })
    }
}

pub async fn run(config: RelayConfig) -> Result<(), String> {
    let state = RelayState::from_config(&config)?;
    let connector_listener = TcpListener::bind(config.connector_listen)
        .await
        .map_err(|error| format!("connector listener: {error}"))?;
    let ingress_listener = TcpListener::bind(config.ingress_listen)
        .await
        .map_err(|error| format!("phone ingress listener: {error}"))?;
    run_with_listeners(state, connector_listener, ingress_listener).await
}

async fn run_with_listeners(
    state: RelayState,
    connector_listener: TcpListener,
    ingress_listener: TcpListener,
) -> Result<(), String> {
    let app = Router::new()
        .route("/v1/connect/{route}", get(connector_upgrade))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state.clone());
    let connector_server = axum::serve(connector_listener, app);
    let ingress_server = serve_ingress(ingress_listener, state);
    tokio::select! {
        result = connector_server => result.map_err(|error| format!("connector server: {error}")),
        result = ingress_server => result,
    }
}

async fn connector_upgrade(
    State(state): State<RelayState>,
    Path(route): Path<String>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let Some(expected) = state.routes.get(&route) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(token) = bearer_token(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let actual: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    if !bool::from(actual.ct_eq(&expected.token_sha256)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    upgrade
        .max_message_size(aiterm_relay_protocol::MAX_FRAME_BYTES)
        .on_upgrade(move |socket| serve_connector(socket, route, state))
        .into_response()
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| token.len() >= 32 && token.len() <= 256)
}

async fn serve_connector(socket: WebSocket, route: String, state: RelayState) {
    let generation = state.next_generation.fetch_add(1, Ordering::Relaxed);
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel(CONNECTOR_QUEUE);
    let streams = Arc::new(Mutex::new(HashMap::new()));
    let connector = Connector {
        generation,
        outgoing: outgoing_tx,
        streams: streams.clone(),
    };
    if let Some(old) = state
        .connectors
        .lock()
        .await
        .insert(route.clone(), connector)
    {
        close_all_streams(&old.streams).await;
    }

    let (mut sink, mut source) = socket.split();
    loop {
        tokio::select! {
            outbound = outgoing_rx.recv() => {
                let Some(frame) = outbound else { break };
                let Ok(encoded) = frame.encode() else { break };
                if sink.send(Message::Binary(encoded.into())).await.is_err() { break; }
            }
            inbound = source.next() => {
                let Some(Ok(message)) = inbound else { break };
                let Message::Binary(bytes) = message else {
                    if matches!(message, Message::Close(_)) { break; }
                    continue;
                };
                let Ok(frame) = Frame::decode(&bytes) else { break };
                match frame {
                    Frame::Data { stream_id, bytes } => {
                        let target = streams.lock().await.get(&stream_id).cloned();
                        let Some(target) = target else { continue };
                        if target.try_send(bytes).is_err() {
                            streams.lock().await.remove(&stream_id);
                            let encoded = Frame::Close {
                                stream_id,
                                reason: b"phone too slow".to_vec(),
                            }.encode().expect("fixed relay close is valid");
                            if sink.send(Message::Binary(encoded.into())).await.is_err() { break; }
                        }
                    }
                    Frame::Close { stream_id, .. } => {
                        streams.lock().await.remove(&stream_id);
                    }
                    Frame::Ping => {
                        if outgoing_rx.is_closed() { break; }
                        let encoded = Frame::Pong.encode().expect("fixed relay pong is valid");
                        if sink.send(Message::Binary(encoded.into())).await.is_err() { break; }
                    }
                    Frame::Pong => {}
                    Frame::Open { .. } => break,
                }
            }
        }
    }

    close_all_streams(&streams).await;
    let mut connectors = state.connectors.lock().await;
    if connectors
        .get(&route)
        .is_some_and(|current| current.generation == generation)
    {
        connectors.remove(&route);
    }
}

async fn close_all_streams(streams: &Mutex<HashMap<u64, mpsc::Sender<Vec<u8>>>>) {
    streams.lock().await.clear();
}

async fn serve_ingress(listener: TcpListener, state: RelayState) -> Result<(), String> {
    let mut tasks = JoinSet::new();
    loop {
        let (socket, _) = listener.accept().await.map_err(|error| error.to_string())?;
        let state = state.clone();
        tasks.spawn(async move { serve_phone(socket, state).await });
        while tasks.len() > 1024 {
            let _ = tasks.join_next().await;
        }
    }
}

async fn serve_phone(mut phone: TcpStream, state: RelayState) {
    let sni = match timeout(CLIENT_HELLO_TIMEOUT, peek_sni(&phone)).await {
        Ok(Ok(sni)) => sni,
        _ => return,
    };
    let Some(route) = route_from_sni(&sni, &state.public_domain) else {
        return;
    };
    let connector = state.connectors.lock().await.get(route).cloned();
    let Some(connector) = connector else { return };
    let stream_id = next_nonzero(&state.next_stream);
    let (toward_phone, mut from_connector) = mpsc::channel::<Vec<u8>>(STREAM_QUEUE);
    {
        let mut streams = connector.streams.lock().await;
        if streams.len() >= MAX_STREAMS_PER_CONNECTOR {
            return;
        }
        streams.insert(stream_id, toward_phone);
    }
    if connector
        .outgoing
        .send(Frame::Open { stream_id })
        .await
        .is_err()
    {
        connector.streams.lock().await.remove(&stream_id);
        return;
    }

    let (mut phone_read, mut phone_write) = phone.split();
    let mut buffer = vec![0u8; aiterm_relay_protocol::MAX_DATA_BYTES];
    loop {
        tokio::select! {
            read = phone_read.read(&mut buffer) => match read {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let frame = Frame::Data { stream_id, bytes: buffer[..count].to_vec() };
                    if connector.outgoing.send(frame).await.is_err() { break; }
                }
            },
            inbound = from_connector.recv() => match inbound {
                Some(bytes) if phone_write.write_all(&bytes).await.is_ok() => {}
                _ => break,
            }
        }
    }
    connector.streams.lock().await.remove(&stream_id);
    let _ = connector
        .outgoing
        .send(Frame::Close {
            stream_id,
            reason: Vec::new(),
        })
        .await;
}

fn next_nonzero(counter: &AtomicU64) -> u64 {
    loop {
        let id = counter.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

fn route_from_sni<'a>(sni: &'a str, public_domain: &str) -> Option<&'a str> {
    let suffix = format!(".{public_domain}");
    let route = sni.strip_suffix(&suffix)?;
    valid_route_id(route).then_some(route)
}

fn valid_route_id(value: &str) -> bool {
    (8..=63).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn normalize_dns_name(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 253
        || value.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        None
    } else {
        Some(value)
    }
}

async fn peek_sni(socket: &TcpStream) -> Result<String, &'static str> {
    let mut bytes = vec![0u8; CLIENT_HELLO_LIMIT];
    let mut previous_count = 0;
    loop {
        socket.readable().await.map_err(|_| "socket unreadable")?;
        let count = socket
            .peek(&mut bytes)
            .await
            .map_err(|_| "client hello unreadable")?;
        if count == 0 {
            return Err("client closed before hello");
        }
        match parse_client_hello_sni(&bytes[..count]) {
            Ok(name) => return Ok(name),
            Err(ParseHelloError::Incomplete) if count < CLIENT_HELLO_LIMIT => {
                // Peeking leaves the existing prefix readable, so readiness
                // alone cannot tell us that another fragment arrived.
                if count == previous_count {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                previous_count = count;
                continue;
            }
            Err(ParseHelloError::Incomplete) => return Err("client hello is too large"),
            Err(ParseHelloError::Invalid) => return Err("invalid client hello"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ParseHelloError {
    Incomplete,
    Invalid,
}

fn parse_client_hello_sni(bytes: &[u8]) -> Result<String, ParseHelloError> {
    // A legal ClientHello may span several TLS handshake records. Reassemble
    // only the bounded handshake prefix required for SNI; socket bytes remain
    // untouched because the caller uses peek(2).
    let mut record_at = 0;
    let mut hello = Vec::new();
    loop {
        if bytes.len() < record_at + 5 {
            return Err(ParseHelloError::Incomplete);
        }
        if bytes[record_at] != 22 {
            return Err(ParseHelloError::Invalid);
        }
        let record_len = u16::from_be_bytes([bytes[record_at + 3], bytes[record_at + 4]]) as usize;
        if record_len == 0 || record_len > CLIENT_HELLO_LIMIT - 5 {
            return Err(ParseHelloError::Invalid);
        }
        let payload_at = record_at + 5;
        let record_end = payload_at
            .checked_add(record_len)
            .ok_or(ParseHelloError::Invalid)?;
        let payload = bytes
            .get(payload_at..record_end)
            .ok_or(ParseHelloError::Incomplete)?;
        if hello.len().saturating_add(payload.len()) > CLIENT_HELLO_LIMIT {
            return Err(ParseHelloError::Invalid);
        }
        hello.extend_from_slice(payload);
        if hello.len() >= 4 {
            let hello_len =
                ((hello[1] as usize) << 16) | ((hello[2] as usize) << 8) | hello[3] as usize;
            if hello_len > CLIENT_HELLO_LIMIT - 4 {
                return Err(ParseHelloError::Invalid);
            }
            if hello.len() >= 4 + hello_len {
                break;
            }
        }
        record_at = record_end;
    }
    if hello.len() < 4 || hello[0] != 1 {
        return Err(ParseHelloError::Invalid);
    }
    let hello_len = ((hello[1] as usize) << 16) | ((hello[2] as usize) << 8) | hello[3] as usize;
    if hello.len() < 4 + hello_len {
        return Err(ParseHelloError::Incomplete);
    }
    let mut at = 4;
    take(&hello, &mut at, 2 + 32)?; // legacy version and random
    let session_len = *take(&hello, &mut at, 1)?.first().unwrap() as usize;
    take(&hello, &mut at, session_len)?;
    let cipher_len = read_u16(&hello, &mut at)? as usize;
    if cipher_len == 0 || cipher_len % 2 != 0 {
        return Err(ParseHelloError::Invalid);
    }
    take(&hello, &mut at, cipher_len)?;
    let compression_len = *take(&hello, &mut at, 1)?.first().unwrap() as usize;
    take(&hello, &mut at, compression_len)?;
    let extensions_len = read_u16(&hello, &mut at)? as usize;
    let extensions = take(&hello, &mut at, extensions_len)?;
    let mut ext_at = 0;
    while ext_at < extensions.len() {
        let kind = read_u16(extensions, &mut ext_at)?;
        let len = read_u16(extensions, &mut ext_at)? as usize;
        let extension = take(extensions, &mut ext_at, len)?;
        if kind != 0 {
            continue;
        }
        let mut name_at = 0;
        let list_len = read_u16(extension, &mut name_at)? as usize;
        let names = take(extension, &mut name_at, list_len)?;
        let mut item_at = 0;
        while item_at < names.len() {
            let name_type = *take(names, &mut item_at, 1)?.first().unwrap();
            let name_len = read_u16(names, &mut item_at)? as usize;
            let name = take(names, &mut item_at, name_len)?;
            if name_type == 0 {
                let text = std::str::from_utf8(name).map_err(|_| ParseHelloError::Invalid)?;
                return normalize_dns_name(text).ok_or(ParseHelloError::Invalid);
            }
        }
        return Err(ParseHelloError::Invalid);
    }
    Err(ParseHelloError::Invalid)
}

fn take<'a>(bytes: &'a [u8], at: &mut usize, count: usize) -> Result<&'a [u8], ParseHelloError> {
    let end = at.checked_add(count).ok_or(ParseHelloError::Invalid)?;
    let value = bytes.get(*at..end).ok_or(ParseHelloError::Incomplete)?;
    *at = end;
    Ok(value)
}

fn read_u16(bytes: &[u8], at: &mut usize) -> Result<u16, ParseHelloError> {
    let value = take(bytes, at, 2)?;
    Ok(u16::from_be_bytes([value[0], value[1]]))
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    #[test]
    fn extracts_sni_without_consuming_application_tls() {
        let hello = client_hello("desk-1234.relay.example.com");
        assert_eq!(
            parse_client_hello_sni(&hello).unwrap(),
            "desk-1234.relay.example.com"
        );
        for end in 0..hello.len() {
            assert_eq!(
                parse_client_hello_sni(&hello[..end]),
                Err(ParseHelloError::Incomplete)
            );
        }

        let payload = &hello[5..];
        let split = 23;
        let mut fragmented = Vec::new();
        for part in [&payload[..split], &payload[split..]] {
            fragmented.extend_from_slice(&[22, 3, 1]);
            fragmented.extend_from_slice(&(part.len() as u16).to_be_bytes());
            fragmented.extend_from_slice(part);
        }
        assert_eq!(
            parse_client_hello_sni(&fragmented).unwrap(),
            "desk-1234.relay.example.com",
        );
    }

    #[test]
    fn route_is_only_one_valid_label_below_public_domain() {
        assert_eq!(
            route_from_sni("desk-1234.relay.example.com", "relay.example.com"),
            Some("desk-1234")
        );
        assert_eq!(
            route_from_sni("relay.example.com", "relay.example.com"),
            None
        );
        assert_eq!(
            route_from_sni("bad.name.relay.example.com", "relay.example.com"),
            None
        );
        assert_eq!(
            route_from_sni("DESK-1234.relay.example.com", "relay.example.com"),
            None
        );
    }

    #[tokio::test]
    async fn phone_bytes_round_trip_without_consuming_the_tls_hello() {
        let token = "a-connector-token-that-is-long-enough";
        let token_sha256 = format!("{:x}", Sha256::digest(token.as_bytes()));
        let connector_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let connector_addr = connector_listener.local_addr().unwrap();
        let ingress_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ingress_addr = ingress_listener.local_addr().unwrap();
        let config = RelayConfig {
            connector_listen: connector_addr,
            ingress_listen: ingress_addr,
            public_domain: "relay.example.com".into(),
            routes: vec![RouteConfig {
                id: "desk-1234".into(),
                token_sha256,
            }],
        };
        let state = RelayState::from_config(&config).unwrap();
        let server = tokio::spawn(run_with_listeners(
            state,
            connector_listener,
            ingress_listener,
        ));

        let mut request = format!("ws://{connector_addr}/v1/connect/desk-1234")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let (mut connector, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let hello = client_hello("desk-1234.relay.example.com");
        let mut phone = TcpStream::connect(ingress_addr).await.unwrap();
        phone.write_all(&hello).await.unwrap();

        let open = connector.next().await.unwrap().unwrap().into_data();
        let stream_id = match Frame::decode(&open).unwrap() {
            Frame::Open { stream_id } => stream_id,
            frame => panic!("expected open, got {frame:?}"),
        };
        let mut relayed = Vec::new();
        while relayed.len() < hello.len() {
            let bytes = connector.next().await.unwrap().unwrap().into_data();
            match Frame::decode(&bytes).unwrap() {
                Frame::Data {
                    stream_id: actual,
                    bytes,
                } => {
                    assert_eq!(actual, stream_id);
                    relayed.extend_from_slice(&bytes);
                }
                frame => panic!("expected data, got {frame:?}"),
            }
        }
        assert_eq!(
            relayed, hello,
            "SNI inspection must be a non-consuming peek"
        );

        let reply = b"opaque encrypted reply".to_vec();
        connector
            .send(tokio_tungstenite::tungstenite::Message::Binary(
                Frame::Data {
                    stream_id,
                    bytes: reply.clone(),
                }
                .encode()
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        let mut received = vec![0u8; reply.len()];
        phone.read_exact(&mut received).await.unwrap();
        assert_eq!(received, reply);

        server.abort();
    }

    fn client_hello(host: &str) -> Vec<u8> {
        let mut names = vec![0];
        names.extend_from_slice(&(host.len() as u16).to_be_bytes());
        names.extend_from_slice(host.as_bytes());
        let mut sni = Vec::new();
        sni.extend_from_slice(&(names.len() as u16).to_be_bytes());
        sni.extend_from_slice(&names);
        let mut extensions = Vec::new();
        extensions.extend_from_slice(&0u16.to_be_bytes());
        extensions.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&sni);
        let mut body = Vec::new();
        body.extend_from_slice(&[3, 3]);
        body.extend_from_slice(&[0; 32]);
        body.push(0);
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(1);
        body.push(0);
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend_from_slice(&extensions);
        let mut handshake = vec![1, 0, 0, body.len() as u8];
        handshake.extend_from_slice(&body);
        let mut record = vec![22, 3, 1];
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);
        record
    }
}
