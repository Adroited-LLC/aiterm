//! Remote access — a phone as a second client of the session model.
//!
//! Off by default. When on, the desktop listens on one port for plain HTTP
//! and one WebSocket, behind a bearer token the phone learns from a QR. The
//! phone sees exactly what the sidebar sees (`list_sessions`), reads a
//! session as the conversation `session_conversation` already assembles for
//! every backend, and sends a line of input into the tab that is running it.
//! When it asks to open or start a session, the desktop opens the tab — so
//! both screens always agree about what is running.
//!
//! What this deliberately is not: a terminal. The phone never receives PTY
//! bytes and never owns one. And it is not a security boundary of its own —
//! there is no TLS here. The LAN, or the VPN (Tailscale) that carries this
//! port off it, is the transport security; the token is what stops a
//! neighbour on that network. Forwarding the port to the internet is not a
//! supported configuration.
//!
//! State on disk is one file, `remote.json` in the aiterm data directory,
//! owner-readable only. Turning remote access off keeps the token, so the
//! phone reconnects without a new QR; "forget phones" rotates it.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{broadcast, oneshot};

const DEFAULT_PORT: u16 = 8877;
/// Bumped when a phone would misread an older desktop. The phone checks it.
const API_VERSION: u32 = 1;

// ---------------------------------------------------------------- config

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub enabled: bool,
    pub port: u16,
    pub token: String,
    /// What the phone shows for this machine. The hostname, unless edited.
    pub name: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: false,
            port: DEFAULT_PORT,
            token: new_token(),
            name: hostname(),
        }
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "aiterm".into())
}

/// 32 bytes from the kernel, written as hex. Hex rather than base64 so the
/// token is safe in a URL query and a QR without any escaping to get wrong.
fn new_token() -> String {
    let mut buf = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn config_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("aiterm").join("remote.json"))
}

fn load_config() -> Config {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &Config) {
    let Some(path) = config_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(json) = serde_json::to_string_pretty(cfg) else { return };
    if std::fs::write(&path, json).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

// ---------------------------------------------------------------- state

/// What the desktop pushes to every connected phone. The phone treats each
/// as "go and look again", not as data: the truth is always a GET away.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Transcripts changed on disk — the list, or a conversation, moved.
    SessionsChanged,
    /// A tab's process ended. `session_id` is the session the tab was bound
    /// to, when it was bound to one.
    SessionExit { session_id: Option<String>, code: Option<u32> },
    /// A session raised a desktop notification — it is waiting on a person.
    Attention { title: String, body: String },
    Ping,
}

struct Running {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
}

pub struct RemoteState {
    config: Mutex<Config>,
    running: Mutex<Option<Running>>,
    /// Why the last start failed, for the settings panel. Cleared on success.
    last_error: Mutex<Option<String>>,
    events: broadcast::Sender<Event>,
}

impl Default for RemoteState {
    fn default() -> Self {
        RemoteState {
            config: Mutex::new(load_config()),
            running: Mutex::new(None),
            last_error: Mutex::new(None),
            events: broadcast::channel(64).0,
        }
    }
}

/// Push an event to every connected phone. Cheap and never fails: with no
/// phone listening the event is dropped, which is the right outcome.
pub fn notify(app: &AppHandle, event: Event) {
    if let Some(state) = app.try_state::<RemoteState>() {
        let _ = state.events.send(event);
    }
}

/// Called once at startup: resume listening if it was on last time.
pub fn autostart(app: &AppHandle) {
    let enabled = app.state::<RemoteState>().config.lock().unwrap().enabled;
    if enabled {
        if let Err(e) = start(app) {
            crate::diag!("remote", "not listening: {e}");
        }
    }
}

fn start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<RemoteState>();
    if state.running.lock().unwrap().is_some() {
        return Ok(());
    }
    let port = state.config.lock().unwrap().port;
    // Bound synchronously so "port in use" is an error the panel shows,
    // not a log line inside a task nobody reads.
    let std_listener = std::net::TcpListener::bind(("0.0.0.0", port))
        .map_err(|e| format!("could not listen on port {port}: {e}"))?;
    std_listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let router = router(app.clone());
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(std_listener) {
            Ok(l) => l,
            Err(e) => {
                crate::diag!("remote", "listener handoff failed: {e}");
                return;
            }
        };
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await;
    });
    *state.running.lock().unwrap() = Some(Running { port, stop: Some(stop_tx) });
    *state.last_error.lock().unwrap() = None;
    crate::diag!("remote", "listening on port {port}");
    Ok(())
}

fn stop(app: &AppHandle) {
    let state = app.state::<RemoteState>();
    let taken = state.running.lock().unwrap().take();
    if let Some(mut running) = taken {
        if let Some(tx) = running.stop.take() {
            let _ = tx.send(());
        }
        crate::diag!("remote", "stopped listening on port {}", running.port);
    }
}

// ---------------------------------------------------------------- commands

#[derive(Serialize)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub name: String,
    /// Addresses a phone might reach this machine on, best first.
    pub addresses: Vec<String>,
    pub error: Option<String>,
}

fn status_of(app: &AppHandle) -> RemoteStatus {
    let state = app.state::<RemoteState>();
    let cfg = state.config.lock().unwrap().clone();
    let running = state.running.lock().unwrap().is_some();
    let error = state.last_error.lock().unwrap().clone();
    RemoteStatus {
        enabled: cfg.enabled,
        running,
        port: cfg.port,
        name: cfg.name,
        addresses: addresses(),
        error,
    }
}

/// IPv4 addresses on real interfaces, ordered so the one a phone is most
/// likely to share comes first: a Tailscale address (100.64/10) beats the
/// LAN, which beats anything else. Loopback is never a candidate — a phone
/// cannot reach it.
fn addresses() -> Vec<String> {
    let mut found: Vec<(u8, String)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(v4) => {
                let o = v4.octets();
                let rank = if o[0] == 100 && (64..128).contains(&o[1]) {
                    0
                } else if o[0] == 10 || o[0] == 192 && o[1] == 168 || o[0] == 172 && (16..32).contains(&o[1]) {
                    1
                } else if o[0] == 169 && o[1] == 254 {
                    return None;
                } else {
                    2
                };
                // Container bridges are reachable by nothing a person holds.
                if i.name.starts_with("docker") || i.name.starts_with("br-") || i.name.starts_with("virbr") {
                    return None;
                }
                Some((rank, v4.to_string()))
            }
            _ => None,
        })
        .collect();
    found.sort();
    found.dedup();
    found.into_iter().map(|(_, a)| a).collect()
}

#[tauri::command]
pub fn remote_status(app: AppHandle) -> RemoteStatus {
    status_of(&app)
}

#[tauri::command]
pub fn remote_set_enabled(app: AppHandle, on: bool) -> RemoteStatus {
    {
        let state = app.state::<RemoteState>();
        let mut cfg = state.config.lock().unwrap();
        cfg.enabled = on;
        save_config(&cfg);
    }
    if on {
        if let Err(e) = start(&app) {
            *app.state::<RemoteState>().last_error.lock().unwrap() = Some(e);
        }
    } else {
        stop(&app);
    }
    status_of(&app)
}

/// Forget every phone: a new token, and every open connection dropped by
/// the next request it makes. Pairing again is a new QR.
#[tauri::command]
pub fn remote_rotate_token(app: AppHandle) -> RemoteStatus {
    let state = app.state::<RemoteState>();
    let mut cfg = state.config.lock().unwrap();
    cfg.token = new_token();
    save_config(&cfg);
    drop(cfg);
    status_of(&app)
}

#[tauri::command]
pub fn remote_set_name(app: AppHandle, name: String) -> RemoteStatus {
    let state = app.state::<RemoteState>();
    let mut cfg = state.config.lock().unwrap();
    let name = name.trim();
    cfg.name = if name.is_empty() { hostname() } else { name.to_string() };
    save_config(&cfg);
    drop(cfg);
    status_of(&app)
}

#[derive(Serialize)]
pub struct PairPayload {
    pub uri: String,
    pub svg: String,
}

/// The QR is one URI and nothing else:
/// `aiterm://pair?v=1&p=<port>&t=<token>&n=<name>&h=<addr>&h=<addr>…`
/// `h` repeats, best address first; the phone tries them in order and keeps
/// the one that answers. The token is the only secret and this is the only
/// place it leaves the desktop.
#[tauri::command]
pub fn remote_pair_payload(app: AppHandle) -> Result<PairPayload, String> {
    let state = app.state::<RemoteState>();
    if state.running.lock().unwrap().is_none() {
        return Err("Turn remote access on first".into());
    }
    let cfg = state.config.lock().unwrap().clone();
    let addrs = addresses();
    if addrs.is_empty() {
        return Err("No network address a phone could reach".into());
    }
    let mut uri = format!(
        "aiterm://pair?v={API_VERSION}&p={}&t={}&n={}",
        cfg.port,
        cfg.token,
        percent_encode(&cfg.name)
    );
    for h in &addrs {
        uri.push_str("&h=");
        uri.push_str(h);
    }
    let code = qrcode::QrCode::new(uri.as_bytes()).map_err(|e| e.to_string())?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    Ok(PairPayload { uri, svg })
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------- server

#[derive(Clone)]
struct Ctx {
    app: AppHandle,
}

fn router(app: AppHandle) -> Router {
    let ctx = Ctx { app };
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/agents", get(agents))
        .route("/v1/sessions", get(sessions).post(new_session))
        .route("/v1/sessions/{id}", get(detail))
        .route("/v1/sessions/{id}/conversation", get(conversation))
        .route("/v1/sessions/{id}/open", post(open))
        .route("/v1/sessions/{id}/input", post(input))
        .route("/v1/sessions/{id}/stop", post(stop_session))
        .route("/v1/events", get(events))
        .layer(middleware::from_fn_with_state(ctx.clone(), auth))
        .with_state(ctx)
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// Every route, the WebSocket included. The token is read per request so a
/// rotation takes effect on the next call, with nothing to restart.
async fn auth(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    req: Request,
    next: Next,
) -> Response {
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
        .or(q.token);
    let expected = ctx.app.state::<RemoteState>().config.lock().unwrap().token.clone();
    let ok = presented.is_some_and(|p| constant_eq(p.as_bytes(), expected.as_bytes()));
    if !ok {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    next.run(req).await
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

async fn status(State(ctx): State<Ctx>) -> Response {
    let cfg = ctx.app.state::<RemoteState>().config.lock().unwrap().clone();
    Json(serde_json::json!({
        "api": API_VERSION,
        "name": cfg.name,
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .into_response()
}

async fn agents() -> Response {
    let list = crate::agents::agent_choices();
    Json(list).into_response()
}

/// The sidebar's list, plus the three facts the phone renders as state:
/// `running` (a process holds this session), `open` (a desktop tab is bound
/// to it — input will land), `live` (the roster's word for it).
async fn sessions(State(ctx): State<Ctx>) -> Response {
    let sessions = crate::sessions::list_sessions().await;
    let running = crate::sessions::running_session_ids().await;
    let open = ctx.app.state::<crate::pty::PtyManager>().bound_sessions();
    Json(serde_json::json!({
        "sessions": sessions,
        "running": running,
        "open": open,
    }))
    .into_response()
}

async fn detail(Path(id): Path<String>) -> Response {
    match crate::detail::session_detail(id).await {
        Some(d) => Json(d).into_response(),
        None => err(StatusCode::NOT_FOUND, "no such session"),
    }
}

#[derive(Deserialize)]
struct ConversationQuery {
    max_chars: Option<usize>,
}

#[derive(Serialize)]
struct Turn {
    role: String,
    text: String,
}

async fn conversation(Path(id): Path<String>, Query(q): Query<ConversationQuery>) -> Response {
    let turns = crate::detail::session_conversation(id, q.max_chars.unwrap_or(60_000)).await;
    let turns: Vec<Turn> = turns.into_iter().map(|(role, text)| Turn { role, text }).collect();
    Json(turns).into_response()
}

/// Open (resume) a session in a desktop tab. The renderer owns tabs, so this
/// is a request to it, answered by `sessions.open` growing on the next list.
async fn open(State(ctx): State<Ctx>, Path(id): Path<String>) -> Response {
    let _ = ctx.app.emit("remote://open-session", serde_json::json!({ "sessionId": id }));
    StatusCode::ACCEPTED.into_response()
}

#[derive(Deserialize)]
struct InputBody {
    text: String,
    /// Press Enter after the text. Default true — a message, not a keystroke.
    enter: Option<bool>,
}

async fn input(State(ctx): State<Ctx>, Path(id): Path<String>, Json(body): Json<InputBody>) -> Response {
    let ptys = ctx.app.state::<crate::pty::PtyManager>();
    let Some(pty) = ptys.pty_for_session(&id) else {
        return err(StatusCode::CONFLICT, "session is not open in a tab — open it first");
    };
    if let Err(e) = ptys.write_str(pty, &body.text) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    if body.enter.unwrap_or(true) {
        // A TUI that just took a paste needs a beat before the Enter, or it
        // reads the two as one and the line sits unsent.
        tokio::time::sleep(Duration::from_millis(60)).await;
        if let Err(e) = ptys.write_str(pty, "\r") {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn stop_session(Path(id): Path<String>) -> Response {
    match crate::sessions::stop_session(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::CONFLICT, e),
    }
}

#[derive(Deserialize)]
struct NewSessionBody {
    agent_id: String,
    cwd: String,
    prompt: Option<String>,
}

async fn new_session(State(ctx): State<Ctx>, Json(body): Json<NewSessionBody>) -> Response {
    let _ = ctx.app.emit(
        "remote://new-session",
        serde_json::json!({ "agentId": body.agent_id, "cwd": body.cwd, "prompt": body.prompt }),
    );
    StatusCode::ACCEPTED.into_response()
}

async fn events(State(ctx): State<Ctx>, ws: WebSocketUpgrade) -> Response {
    let rx = ctx.app.state::<RemoteState>().events.subscribe();
    ws.on_upgrade(move |socket| stream_events(socket, rx))
}

async fn stream_events(mut socket: WebSocket, mut rx: broadcast::Receiver<Event>) {
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.tick().await; // the first tick is immediate; skip it
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let ev = match ev {
                    Ok(ev) => ev,
                    // Fell behind: the phone re-reads on the next event anyway.
                    Err(broadcast::error::RecvError::Lagged(_)) => Event::SessionsChanged,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Ok(text) = serde_json::to_string(&ev) else { continue };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            _ = ping.tick() => {
                let Ok(text) = serde_json::to_string(&Event::Ping) else { continue };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_64_hex_chars_and_never_repeats() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn constant_eq_compares_whole_strings() {
        assert!(constant_eq(b"abc", b"abc"));
        assert!(!constant_eq(b"abc", b"abd"));
        assert!(!constant_eq(b"abc", b"ab"));
    }

    #[test]
    fn the_name_survives_a_uri() {
        assert_eq!(percent_encode("john-laptop"), "john-laptop");
        assert_eq!(percent_encode("John's PC"), "John%27s%20PC");
    }
}
