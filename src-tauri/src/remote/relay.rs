use super::auth::{set_private_permissions, write_private_file};
use aiterm_relay_protocol::Frame;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message;

const CONFIG_FILE: &str = "relay.json";
const MAX_STREAMS: usize = 128;
const STREAM_QUEUE: usize = 32;
const OUTGOING_QUEUE: usize = 256;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    pub connector_url: String,
    pub public_host: String,
    pub public_port: u16,
    pub route_id: String,
    pub token: String,
}

impl RelayConfig {
    pub fn validate(&self) -> Result<(), String> {
        let url = url::Url::parse(&self.connector_url)
            .map_err(|_| "relay connector URL is invalid".to_string())?;
        if !matches!(url.scheme(), "ws" | "wss")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("relay connector URL must be a ws:// or wss:// URL without credentials, query, or fragment".into());
        }
        if url.scheme() == "ws"
            && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
        {
            return Err("a non-local relay connector must use wss://".into());
        }
        if normalize_dns_name(&self.public_host).as_deref() != Some(self.public_host.as_str()) {
            return Err("relay public host must be a lowercase DNS name".into());
        }
        if self.public_port == 0 {
            return Err("relay public port is invalid".into());
        }
        if !valid_route_id(&self.route_id) {
            return Err("relay route id is invalid".into());
        }
        if !self.public_host.starts_with(&format!("{}.", self.route_id)) {
            return Err("relay public host must begin with the route id".into());
        }
        if !(43..=128).contains(&self.token.len())
            || !self.token.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err("relay connector token is invalid".into());
        }
        Ok(())
    }

    fn websocket_url(&self) -> String {
        format!(
            "{}/{}",
            self.connector_url.trim_end_matches('/'),
            self.route_id
        )
    }

    pub fn load(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join(CONFIG_FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("could not read relay settings: {error}")),
        };
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("relay settings are corrupt: {error}"))?;
        config.validate()?;
        Ok(Some(config))
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        self.validate()?;
        std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
        set_private_permissions(root, 0o700).map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        write_private_file(&root.join(CONFIG_FILE), &bytes).map_err(|error| error.to_string())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayConnectionState {
    #[default]
    Off,
    Connecting,
    Connected,
    Retrying,
}

pub struct RelayConnectorHandle {
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
    state: watch::Receiver<RelayConnectionState>,
}

impl RelayConnectorHandle {
    pub fn start(config: RelayConfig, local: SocketAddr) -> Self {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (state_tx, state_rx) = watch::channel(RelayConnectionState::Connecting);
        let task = tokio::spawn(run(config, local, shutdown_rx, state_tx));
        Self {
            shutdown: Some(shutdown_tx),
            task,
            state: state_rx,
        }
    }

    pub fn state(&self) -> RelayConnectionState {
        *self.state.borrow()
    }

    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = self.task.await;
    }
}

async fn run(
    config: RelayConfig,
    local: SocketAddr,
    mut shutdown: oneshot::Receiver<()>,
    state: watch::Sender<RelayConnectionState>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let _ = state.send(if backoff == Duration::from_secs(1) {
            RelayConnectionState::Connecting
        } else {
            RelayConnectionState::Retrying
        });
        let connected = tokio::select! {
            _ = &mut shutdown => break,
            result = connect_once(&config, local, &state) => result,
        };
        if let Err(error) = connected {
            crate::diag!("remote", "relay connector: {error}");
        }
        let was_connected = *state.borrow() == RelayConnectionState::Connected;
        let _ = state.send(RelayConnectionState::Retrying);
        tokio::select! {
            _ = &mut shutdown => break,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = if was_connected {
            Duration::from_secs(1)
        } else {
            (backoff * 2).min(Duration::from_secs(30))
        };
    }
    let _ = state.send(RelayConnectionState::Off);
}

async fn connect_once(
    config: &RelayConfig,
    local: SocketAddr,
    state: &watch::Sender<RelayConnectionState>,
) -> Result<(), String> {
    let mut request = config
        .websocket_url()
        .into_client_request()
        .map_err(|error| format!("invalid connector request: {error}"))?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", config.token)
            .parse()
            .map_err(|_| "invalid connector token".to_string())?,
    );
    let (socket, _) =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| "connection timed out".to_string())?
            .map_err(|error| format!("connection failed: {error}"))?;
    let _ = state.send(RelayConnectionState::Connected);
    let (mut sink, mut source) = socket.split();
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<Frame>(OUTGOING_QUEUE);
    let (done_tx, mut done_rx) = mpsc::channel::<u64>(MAX_STREAMS);
    let mut streams = HashMap::<u64, mpsc::Sender<Vec<u8>>>::new();
    let mut tasks = JoinSet::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let result = loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if send_frame(&mut sink, Frame::Ping).await.is_err() { break Err("relay heartbeat failed".into()); }
            }
            Some(stream_id) = done_rx.recv() => { streams.remove(&stream_id); }
            outbound = outgoing_rx.recv() => {
                let Some(frame) = outbound else { break Err("relay writer stopped".into()) };
                if send_frame(&mut sink, frame).await.is_err() { break Err("relay write failed".into()); }
            }
            inbound = source.next() => {
                let Some(Ok(message)) = inbound else { break Err("relay connection closed".into()) };
                let Message::Binary(bytes) = message else {
                    if matches!(message, Message::Close(_)) { break Err("relay connection closed".into()); }
                    continue;
                };
                let frame = Frame::decode(&bytes).map_err(|error| error.to_string())?;
                match frame {
                    Frame::Open { stream_id } => {
                        if streams.contains_key(&stream_id) || streams.len() >= MAX_STREAMS {
                            let _ = outgoing_tx.send(Frame::Close { stream_id, reason: b"stream limit".to_vec() }).await;
                            continue;
                        }
                        let (tx, rx) = mpsc::channel(STREAM_QUEUE);
                        streams.insert(stream_id, tx);
                        let outgoing = outgoing_tx.clone();
                        let done = done_tx.clone();
                        tasks.spawn(async move { bridge_local(stream_id, local, rx, outgoing, done).await });
                    }
                    Frame::Data { stream_id, bytes } => {
                        let Some(stream) = streams.get(&stream_id).cloned() else { continue };
                        if stream.try_send(bytes).is_err() {
                            streams.remove(&stream_id);
                            let _ = outgoing_tx.try_send(Frame::Close {
                                stream_id,
                                reason: b"desktop too slow".to_vec(),
                            });
                        }
                    }
                    Frame::Close { stream_id, .. } => { streams.remove(&stream_id); }
                    Frame::Ping => { let _ = outgoing_tx.send(Frame::Pong).await; }
                    Frame::Pong => {}
                }
            }
        }
    };
    streams.clear();
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    result
}

async fn send_frame<S>(sink: &mut S, frame: Frame) -> Result<(), String>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    let bytes = frame.encode().map_err(|error| error.to_string())?;
    sink.send(Message::Binary(bytes.into()))
        .await
        .map_err(|error| error.to_string())
}

async fn bridge_local(
    stream_id: u64,
    local: SocketAddr,
    mut inbound: mpsc::Receiver<Vec<u8>>,
    outgoing: mpsc::Sender<Frame>,
    done: mpsc::Sender<u64>,
) {
    let socket = match tokio::time::timeout(LOCAL_CONNECT_TIMEOUT, TcpStream::connect(local)).await
    {
        Ok(Ok(socket)) => socket,
        _ => {
            let _ = outgoing
                .send(Frame::Close {
                    stream_id,
                    reason: b"desktop unavailable".to_vec(),
                })
                .await;
            let _ = done.send(stream_id).await;
            return;
        }
    };
    let (mut reader, mut writer) = socket.into_split();
    let mut buffer = vec![0u8; aiterm_relay_protocol::MAX_DATA_BYTES];
    loop {
        tokio::select! {
            read = reader.read(&mut buffer) => match read {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if outgoing.send(Frame::Data { stream_id, bytes: buffer[..count].to_vec() }).await.is_err() { break; }
                }
            },
            bytes = inbound.recv() => match bytes {
                Some(bytes) if writer.write_all(&bytes).await.is_ok() => {}
                _ => break,
            }
        }
    }
    let _ = outgoing
        .send(Frame::Close {
            stream_id,
            reason: Vec::new(),
        })
        .await;
    let _ = done.send(stream_id).await;
}

fn normalize_dns_name(value: &str) -> Option<String> {
    let normalized = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > 253
        || normalized.split('.').any(|label| {
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
        Some(normalized)
    }
}

fn valid_route_id(value: &str) -> bool {
    (8..=63).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn valid_config() -> RelayConfig {
        RelayConfig {
            connector_url: "wss://control.relay.example.com/v1/connect".into(),
            public_host: "desk-1234.relay.example.com".into(),
            public_port: 443,
            route_id: "desk-1234".into(),
            token: "x".repeat(43),
        }
    }

    #[test]
    fn config_rejects_ambiguous_or_unsafe_endpoints() {
        assert!(valid_config().validate().is_ok());
        let mut value = valid_config();
        value.connector_url = "https://relay.example.com".into();
        assert!(value.validate().is_err());
        let mut value = valid_config();
        value.connector_url = "ws://relay.example.com/v1/connect".into();
        assert!(value.validate().is_err());
        let mut value = valid_config();
        value.connector_url = "ws://127.0.0.1:8080/v1/connect".into();
        assert!(value.validate().is_ok());
        let mut value = valid_config();
        value.public_host = "Desk.relay.example.com".into();
        assert!(value.validate().is_err());
        let mut value = valid_config();
        value.token = "short".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn persisted_config_round_trips_without_exposing_a_view() {
        let root =
            std::env::temp_dir().join(format!("aiterm-relay-config-{}", uuid::Uuid::new_v4()));
        let config = valid_config();
        config.save(&root).unwrap();
        assert_eq!(RelayConfig::load(&root).unwrap(), Some(config));
        fs::remove_dir_all(root).unwrap();
    }
}
