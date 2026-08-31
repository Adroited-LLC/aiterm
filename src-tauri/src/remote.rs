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
//! bytes and never owns one.
//!
//! Reaching the desktop from outside the house is the desktop's own job —
//! there is no relay and no third party in the path. While remote access is
//! on, the desktop asks the router (UPnP IGD) to map its port, learns its
//! public address, and puts LAN and public addresses in the QR; the phone
//! tries them in order. The listener is TLS with a self-signed identity the
//! desktop mints once and keeps; the QR carries the certificate's SHA-256
//! and the phone trusts that certificate and nothing else. The token is what
//! stops a stranger who can reach the port; repeated bad tokens from one
//! address are refused for a while.
//!
//! State on disk is one file, `remote.json` in the aiterm data directory,
//! owner-readable only. Turning remote access off keeps the token, so the
//! phone reconnects without a new QR; "forget phones" rotates it.

use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast;

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
    /// A tab's activity changed: "working" | "attention" | "idle".
    Activity { session_id: String, activity: String },
    /// A file in a session's workspace was created, modified or deleted.
    FileChanged { session_id: String, path: String, kind: String },
    /// A second-agent relay changed state; `session_id` is the first
    /// agent's session — where the phone is looking.
    Relay {
        session_id: String,
        b_session_id: Option<String>,
        b_name: String,
        phase: String,
        round: u32,
        rounds: u32,
        note: String,
    },
    Ping,
}

struct Running {
    port: u16,
    handle: axum_server::Handle<SocketAddr>,
    /// Set false to end the UPnP renewal loop.
    upnp_alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// What the router said, for the panel and the QR.
#[derive(Clone, Default)]
struct Reach {
    /// "off" | "searching" | "mapped" | "no_router" | "refused"
    upnp: String,
    public_ip: Option<IpAddr>,
}

/// A phone holding the event socket open — the definition of "connected".
#[derive(Clone, Serialize)]
pub struct ClientInfo {
    pub id: u64,
    /// What the phone calls itself ("Google Pixel 10 Pro XL").
    pub device: String,
    pub os: String,
    pub app: String,
    pub address: String,
    /// Unix seconds.
    pub since: u64,
}

pub struct RemoteState {
    config: Mutex<Config>,
    running: Mutex<Option<Running>>,
    reach: Mutex<Reach>,
    clients: Mutex<HashMap<u64, ClientInfo>>,
    next_client: std::sync::atomic::AtomicU64,
    /// Last good answer per usage source. A service that rate-limits the
    /// question this minute still had a number a minute ago.
    usage_cache: Mutex<HashMap<String, crate::usage::UsageSource>>,
    /// Why the last start failed, for the settings panel. Cleared on success.
    last_error: Mutex<Option<String>>,
    /// Bad tokens per address: (failures, first failure). See `auth`.
    strikes: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    /// Provider model catalogs, cached — the phone asks on every connect
    /// and OpenRouter's /models is not a thing to curl that often.
    models_cache: Mutex<HashMap<String, (Instant, Vec<String>)>>,
    /// Live preview tickets: unguessable path → what it serves. Minted by
    /// the authed API, honored without a bearer — a WebView can't attach
    /// headers to subresource requests, but it can keep a secret path.
    previews: Mutex<HashMap<String, Preview>>,
    events: broadcast::Sender<Event>,
}

#[derive(Clone)]
enum PreviewTarget {
    /// Reverse-proxy to a server on the desktop's loopback.
    Port(u16),
    /// Serve files out of a folder (an agent-built static page).
    Dir(PathBuf),
}

struct Preview {
    target: PreviewTarget,
    expires: Instant,
}

impl Default for RemoteState {
    fn default() -> Self {
        RemoteState {
            config: Mutex::new(load_config()),
            running: Mutex::new(None),
            reach: Mutex::new(Reach { upnp: "off".into(), public_ip: None }),
            clients: Mutex::new(HashMap::new()),
            next_client: std::sync::atomic::AtomicU64::new(1),
            usage_cache: Mutex::new(HashMap::new()),
            last_error: Mutex::new(None),
            strikes: Mutex::new(HashMap::new()),
            models_cache: Mutex::new(HashMap::new()),
            previews: Mutex::new(HashMap::new()),
            events: broadcast::channel(64).0,
        }
    }
}

// ---------------------------------------------------------------- identity

/// The listener's certificate, minted once and kept beside the config. Its
/// SHA-256 is what the phone pins, so regenerating it means pairing again —
/// which is why it is only ever generated when the files are absent.
struct Identity {
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
    fingerprint: String,
}

fn identity() -> Result<Identity, String> {
    let dir = config_path().and_then(|p| p.parent().map(|d| d.to_path_buf())).ok_or("no data dir")?;
    let cert_path = dir.join("remote-cert.pem");
    let key_path = dir.join("remote-key.pem");
    let (cert_pem, key_pem) = match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        (Ok(c), Ok(k)) => (c, k),
        _ => {
            let ck = rcgen::generate_simple_self_signed(vec!["aiterm".to_string()])
                .map_err(|e| format!("could not create a certificate: {e}"))?;
            let c = ck.cert.pem().into_bytes();
            let k = ck.signing_key.serialize_pem().into_bytes();
            let _ = std::fs::create_dir_all(&dir);
            std::fs::write(&cert_path, &c).map_err(|e| e.to_string())?;
            std::fs::write(&key_path, &k).map_err(|e| e.to_string())?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
            (c, k)
        }
    };
    let der = pem_to_der(&cert_pem).ok_or("certificate file is not PEM")?;
    use sha2::Digest;
    let fingerprint = sha2::Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
    Ok(Identity { cert_pem, key_pem, fingerprint })
}

fn pem_to_der(pem: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(pem).ok()?;
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    base64_decode(&body)
}

/// Standard base64 (what PEM uses), decoded without a crate.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = T.iter().position(|&t| t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- reachability

const UPNP_LEASE_SECS: u32 = 3600;

/// Ask the router for the port, and keep asking while we are on. Runs on
/// its own thread: IGD discovery is a blocking multicast search and the
/// renewal is a sleep loop. Every outcome is written to `reach` for the
/// panel; none of them is an error the listener cares about — a desktop
/// with no cooperative router still serves the LAN.
fn keep_port_mapped(app: AppHandle, port: u16, alive: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    let set = |upnp: &str, ip: Option<IpAddr>| {
        if let Some(state) = app.try_state::<RemoteState>() {
            *state.reach.lock().unwrap() = Reach { upnp: upnp.into(), public_ip: ip };
        }
    };
    set("searching", None);
    let options = igd_next::SearchOptions { timeout: Some(Duration::from_secs(4)), ..Default::default() };
    let gateway = match igd_next::search_gateway(options) {
        Ok(g) => g,
        Err(e) => {
            crate::diag!("remote", "no UPnP router: {e}");
            set("no_router", None);
            return;
        }
    };
    // The address the router can reach us on: whichever interface routes to it.
    let local_ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect(gateway.addr).map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let local = SocketAddr::new(local_ip, port);
    let mut ip = gateway.get_external_ip().ok();
    while alive.load(Ordering::Relaxed) {
        match gateway.add_port(igd_next::PortMappingProtocol::TCP, port, local, UPNP_LEASE_SECS, "aiterm remote") {
            Ok(()) => {
                ip = gateway.get_external_ip().ok().or(ip);
                set("mapped", ip);
            }
            Err(e) => {
                crate::diag!("remote", "router refused the port mapping: {e}");
                set("refused", ip);
            }
        }
        // Renew well inside the lease; wake often enough that stop is prompt.
        let mut slept = 0;
        while alive.load(Ordering::Relaxed) && slept < UPNP_LEASE_SECS / 2 {
            std::thread::sleep(Duration::from_secs(5));
            slept += 5;
        }
    }
    let _ = gateway.remove_port(igd_next::PortMappingProtocol::TCP, port);
    set("off", None);
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
    // Tokio adopts the socket and requires it non-blocking; handing it a
    // blocking one panics the accept loop, silently, on a worker thread.
    std_listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let id = identity()?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let handle = axum_server::Handle::<SocketAddr>::new();
    let router = router(app.clone());
    let served = handle.clone();
    tauri::async_runtime::spawn(async move {
        let tls = match axum_server::tls_rustls::RustlsConfig::from_pem(id.cert_pem, id.key_pem).await {
            Ok(t) => t,
            Err(e) => {
                crate::diag!("remote", "TLS setup failed: {e}");
                return;
            }
        };
        let server = match axum_server::from_tcp_rustls(std_listener, tls) {
            Ok(s) => s,
            Err(e) => {
                crate::diag!("remote", "listener handoff failed: {e}");
                return;
            }
        };
        let r = server
            .handle(served)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await;
        if let Err(e) = r {
            crate::diag!("remote", "listener ended: {e}");
        }
    });
    let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let app = app.clone();
        let alive = alive.clone();
        std::thread::spawn(move || keep_port_mapped(app, port, alive));
    }
    *state.running.lock().unwrap() = Some(Running { port, handle, upnp_alive: alive });
    *state.last_error.lock().unwrap() = None;
    crate::diag!("remote", "listening (TLS) on port {port}");
    Ok(())
}

fn stop(app: &AppHandle) {
    let state = app.state::<RemoteState>();
    let taken = state.running.lock().unwrap().take();
    if let Some(running) = taken {
        running.upnp_alive.store(false, std::sync::atomic::Ordering::Relaxed);
        running.handle.graceful_shutdown(Some(Duration::from_secs(2)));
        state.clients.lock().unwrap().clear();
        let _ = app.emit("remote://clients", ());
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
    /// What the router said: "off" | "searching" | "mapped" | "no_router" | "refused".
    pub upnp: String,
    /// The address the internet sees, when the router told us.
    pub public_address: Option<String>,
    /// SHA-256 of the listener certificate, hex — what a paired phone pins.
    pub fingerprint: Option<String>,
    /// Phones holding the event socket open right now.
    pub clients: Vec<ClientInfo>,
    pub error: Option<String>,
}

fn status_of(app: &AppHandle) -> RemoteStatus {
    let state = app.state::<RemoteState>();
    let cfg = state.config.lock().unwrap().clone();
    let running = state.running.lock().unwrap().is_some();
    let error = state.last_error.lock().unwrap().clone();
    let reach = state.reach.lock().unwrap().clone();
    let mut clients: Vec<ClientInfo> = state.clients.lock().unwrap().values().cloned().collect();
    clients.sort_by_key(|c| c.since);
    RemoteStatus {
        enabled: cfg.enabled,
        running,
        port: cfg.port,
        name: cfg.name,
        addresses: addresses(),
        upnp: reach.upnp,
        public_address: reach.public_ip.map(|ip| ip.to_string()),
        fingerprint: identity().ok().map(|i| i.fingerprint),
        clients,
        error,
    }
}

/// Change the port. Takes effect at once when listening: the listener and
/// the router mapping both move. Paired phones keep working only if they
/// scan again — the port is in the QR — so the panel says so.
#[tauri::command]
pub fn remote_set_port(app: AppHandle, port: u16) -> Result<RemoteStatus, String> {
    if port < 1024 {
        return Err("Pick a port from 1024 to 65535".into());
    }
    let was_running = {
        let state = app.state::<RemoteState>();
        let mut cfg = state.config.lock().unwrap();
        if cfg.port == port {
            return Ok(status_of(&app));
        }
        cfg.port = port;
        save_config(&cfg);
        let running = state.running.lock().unwrap().is_some();
        running
    };
    if was_running {
        stop(&app);
        // The old socket closes asynchronously; give it a moment before rebinding.
        std::thread::sleep(Duration::from_millis(300));
        if let Err(e) = start(&app) {
            *app.state::<RemoteState>().last_error.lock().unwrap() = Some(e.clone());
            return Err(e);
        }
    }
    Ok(status_of(&app))
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
/// `aiterm://pair?v=1&p=<port>&t=<token>&f=<cert sha256>&n=<name>&h=<addr>&h=<addr>…`
/// `h` repeats, LAN addresses first and the public one last; the phone
/// tries them in order and keeps the one that answers. `f` is the listener
/// certificate the phone will trust and nothing else. The token is the only
/// secret and this is the only place it leaves the desktop.
#[tauri::command]
pub fn remote_pair_payload(app: AppHandle) -> Result<PairPayload, String> {
    let state = app.state::<RemoteState>();
    if state.running.lock().unwrap().is_none() {
        return Err("Turn remote access on first".into());
    }
    let cfg = state.config.lock().unwrap().clone();
    let public_ip = state.reach.lock().unwrap().public_ip;
    let mut addrs = addresses();
    if let Some(ip) = public_ip {
        addrs.push(ip.to_string());
    }
    if addrs.is_empty() {
        return Err("No network address a phone could reach".into());
    }
    let fingerprint = identity()?.fingerprint;
    let mut uri = format!(
        "aiterm://pair?v={API_VERSION}&p={}&t={}&f={}&n={}",
        cfg.port,
        cfg.token,
        fingerprint,
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
        .route("/v1/usage", get(usage))
        .route("/v1/agents", get(agents))
        .route("/v1/uploads", post(upload).layer(axum::extract::DefaultBodyLimit::max(UPLOAD_LIMIT + 1024)))
        .route("/v1/search", get(search))
        .route("/v1/files", get(file))
        .route("/v1/sessions", get(sessions).post(new_session))
        .route("/v1/sessions/{id}/artifacts", get(artifacts))
        .route("/v1/sessions/{id}/files", get(session_files))
        .route("/v1/sessions/{id}/changes", get(session_changes))
        .route("/v1/browse", get(browse))
        .route("/v1/dirs", post(make_dir))
        .route("/v1/sessions/{id}", get(detail))
        .route("/v1/sessions/{id}/conversation", get(conversation))
        .route("/v1/sessions/{id}/open", post(open))
        .route("/v1/sessions/{id}/input", post(input))
        .route("/v1/sessions/{id}/rename", post(rename))
        .route("/v1/sessions/{id}/star", post(star))
        .route("/v1/sessions/{id}/bringin", post(bring_in))
        .route("/v1/sessions/{id}/interrupt", post(interrupt))
        .route("/v1/sessions/{id}/stop", post(stop_session))
        .route("/v1/events", get(events))
        .route("/v1/previews", post(make_preview))
        .layer(middleware::from_fn_with_state(ctx.clone(), auth))
        // Below the auth layer on purpose: preview paths carry their own
        // unguessable ticket. See `make_preview`.
        .route("/p/{ticket}/", get(preview_root))
        .route("/p/{ticket}/{*rest}", get(preview_rest))
        .with_state(ctx)
}

// ------------------------------------------------------------- previews
//
// "The agent built a web page — show me." Two shapes: a folder of static
// files (the usual index.html landing page), or a dev server the agent
// started on the desktop's loopback. Either way the phone can't reach it
// directly, and a WebView can't send Authorization headers for the page's
// images and stylesheets — so the authed API mints a ticket, and the
// ticket IS the credential: an unguessable path prefix, expiring, serving
// exactly one target.

const PREVIEW_TTL: Duration = Duration::from_secs(3600);

#[derive(Deserialize)]
struct PreviewBody {
    port: Option<u16>,
    dir: Option<String>,
}

async fn make_preview(State(ctx): State<Ctx>, Json(b): Json<PreviewBody>) -> Response {
    let target = if let Some(p) = b.port {
        PreviewTarget::Port(p)
    } else if let Some(d) = b.dir {
        let Ok(real) = std::path::PathBuf::from(&d).canonicalize() else {
            return err(StatusCode::NOT_FOUND, "no such folder");
        };
        // Home, plus claude's scratchpads — where throwaway demo pages
        // land by convention (/tmp/claude-<uid>/<project>/scratchpad).
        let scratch = real.to_string_lossy().starts_with("/tmp/claude-");
        if !real.is_dir() || !(under_home(&real) || scratch) {
            return err(StatusCode::FORBIDDEN, "only folders under home");
        }
        PreviewTarget::Dir(real)
    } else {
        return err(StatusCode::BAD_REQUEST, "port or dir required");
    };
    let ticket = new_token();
    let state = ctx.app.state::<RemoteState>();
    let mut p = state.previews.lock().unwrap();
    p.retain(|_, v| v.expires > Instant::now());
    p.insert(ticket.clone(), Preview { target, expires: Instant::now() + PREVIEW_TTL });
    Json(serde_json::json!({ "path": format!("/p/{ticket}/") })).into_response()
}

async fn preview_root(
    State(ctx): State<Ctx>,
    Path(ticket): Path<String>,
    axum::extract::RawQuery(q): axum::extract::RawQuery,
) -> Response {
    serve_preview(ctx, ticket, String::new(), q).await
}

async fn preview_rest(
    State(ctx): State<Ctx>,
    Path((ticket, rest)): Path<(String, String)>,
    axum::extract::RawQuery(q): axum::extract::RawQuery,
) -> Response {
    serve_preview(ctx, ticket, rest, q).await
}

async fn serve_preview(ctx: Ctx, ticket: String, rest: String, query: Option<String>) -> Response {
    let target = {
        let state = ctx.app.state::<RemoteState>();
        let p = state.previews.lock().unwrap();
        match p.get(&ticket) {
            Some(v) if v.expires > Instant::now() => v.target.clone(),
            _ => return err(StatusCode::NOT_FOUND, "preview expired — reopen it from the app"),
        }
    };
    match target {
        PreviewTarget::Dir(dir) => {
            if rest.contains("..") {
                return err(StatusCode::FORBIDDEN, "no");
            }
            let mut path = dir.join(rest.trim_start_matches('/'));
            if rest.is_empty() || path.is_dir() {
                path = path.join("index.html");
            }
            match std::fs::read(&path) {
                Ok(bytes) => ([(axum::http::header::CONTENT_TYPE, mime_of(&path))], bytes).into_response(),
                Err(_) => err(StatusCode::NOT_FOUND, "not found"),
            }
        }
        PreviewTarget::Port(port) => proxy_local(port, &rest, query.as_deref()).await,
    }
}

/// One GET against a loopback server, HTTP/1.0 so the body is
/// close-delimited — no client crate, no chunked parsing. Dev servers
/// (python http.server, vite, uvicorn) all answer 1.0 happily.
async fn proxy_local(port: u16, rest: &str, query: Option<&str>) -> Response {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let path = format!(
        "/{}{}",
        rest.trim_start_matches('/'),
        query.map(|q| format!("?{q}")).unwrap_or_default()
    );
    let work = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.ok()?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.ok()?;
        Some(buf)
    };
    let Ok(Some(raw)) = tokio::time::timeout(Duration::from_secs(20), work).await else {
        return err(StatusCode::BAD_GATEWAY, format!("nothing answered on port {port}"));
    };
    // Split head from body, pull status and content-type, pass the rest on.
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(0);
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let body = raw[(split + 4).min(raw.len())..].to_vec();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .and_then(|c| StatusCode::from_u16(c).ok())
        .unwrap_or(StatusCode::OK);
    let ctype = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
        .map(|l| l[13..].trim().to_string())
        .unwrap_or_else(|| "application/octet-stream".into());
    (status, [(axum::http::header::CONTENT_TYPE, ctype)], body).into_response()
}

fn mime_of(p: &std::path::Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "mp4" => "video/mp4",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Listening TCP ports owned by a process tree — how a session's dev
/// server is noticed. `ss` names the pid per listener; /proc names each
/// pid's parent; the intersection is "this session is serving something".
fn ports_of_tree(root: u32) -> Vec<u16> {
    let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
            // ppid is the 2nd field after the parenthesised comm.
            let Some(after) = stat.rsplit(')').next() else { continue };
            let mut it = after.split_whitespace();
            let _state = it.next();
            if let Some(ppid) = it.next().and_then(|p| p.parse::<u32>().ok()) {
                kids.entry(ppid).or_default().push(pid);
            }
        }
    }
    let mut tree = std::collections::HashSet::new();
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        if tree.insert(p) {
            if let Some(c) = kids.get(&p) {
                stack.extend(c);
            }
        }
    }
    let Ok(out) = std::process::Command::new("ss").args(["-ltnpH"]).output() else { return vec![] };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut ports = Vec::new();
    for line in text.lines() {
        let Some(pid) = line.split("pid=").nth(1).and_then(|r| r.split(&[',', ')'][..]).next()).and_then(|p| p.parse::<u32>().ok()) else { continue };
        if !tree.contains(&pid) {
            continue;
        }
        // Local address is the 4th column; the port follows the last ':'.
        if let Some(addr) = line.split_whitespace().nth(3) {
            if let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }
    ports.sort_unstable();
    ports
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

const STRIKES_ALLOWED: u32 = 20;
const STRIKES_WINDOW: Duration = Duration::from_secs(600);

/// Every route, the WebSocket included. The token is read per request so a
/// rotation takes effect on the next call, with nothing to restart. An
/// address that keeps presenting bad tokens is refused outright for a
/// while — the port may be reachable from the internet, and a 256-bit
/// token is only unguessable if guessing is slow.
async fn auth(
    State(ctx): State<Ctx>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    req: Request,
    next: Next,
) -> Response {
    let state = ctx.app.state::<RemoteState>();
    {
        let mut strikes = state.strikes.lock().unwrap();
        if let Some((n, since)) = strikes.get(&peer.ip()).copied() {
            if since.elapsed() > STRIKES_WINDOW {
                strikes.remove(&peer.ip());
            } else if n >= STRIKES_ALLOWED {
                return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({ "error": "too many bad tokens" }))).into_response();
            }
        }
    }
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
        .or(q.token);
    let expected = state.config.lock().unwrap().token.clone();
    let ok = presented.is_some_and(|p| constant_eq(p.as_bytes(), expected.as_bytes()));
    if !ok {
        let mut strikes = state.strikes.lock().unwrap();
        let e = strikes.entry(peer.ip()).or_insert((0, Instant::now()));
        e.0 += 1;
        if e.0 == 1 || e.0 == STRIKES_ALLOWED {
            crate::diag!("remote", "bad token from {} ({} so far)", peer.ip(), e.0);
        }
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
    let state = ctx.app.state::<RemoteState>();
    let cfg = state.config.lock().unwrap().clone();
    // The addresses the desktop answers on right now, LAN first, public
    // last — the same list the QR carries. The phone refreshes its
    // candidates from this on every connect, so a DHCP move or a new
    // public IP never strands it with only stale addresses.
    let mut hosts = addresses();
    if let Some(ip) = state.reach.lock().unwrap().public_ip {
        hosts.push(ip.to_string());
    }
    Json(serde_json::json!({
        "api": API_VERSION,
        "name": cfg.name,
        "version": env!("CARGO_PKG_VERSION"),
        "hosts": hosts,
    }))
    .into_response()
}

async fn agents(State(ctx): State<Ctx>) -> Response {
    let mut list = serde_json::to_value(crate::agents::agent_choices()).unwrap_or_default();
    // Every keyed provider joins as a choice of its own — "OpenRouter"
    // beside the CLIs — carrying its FULL catalog: starred models first,
    // then everything the provider publishes. The id wears an api: prefix
    // so bring-in and new-session know the kind.
    if let Some(arr) = list.as_array_mut() {
        for p in crate::providers::load_providers() {
            if p.api_key.is_empty() {
                continue;
            }
            let catalog = cached_models(&ctx, &p.id).await;
            let mut models = p.startup_models.clone();
            for m in catalog {
                if !models.contains(&m) {
                    models.push(m);
                }
            }
            if models.is_empty() {
                continue;
            }
            arr.push(serde_json::json!({
                "id": format!("api:{}", p.id),
                "display_name": p.name,
                "models": models.iter().map(|m| serde_json::json!({
                    "id": m,
                    "display_name": m.split('/').next_back().unwrap_or(m),
                    "efforts": [],
                })).collect::<Vec<_>>(),
                "mints_session_id": false,
            }));
        }
    }
    Json(list).into_response()
}

/// A provider's /models ids, at most one real fetch per ten minutes.
async fn cached_models(ctx: &Ctx, provider_id: &str) -> Vec<String> {
    {
        let state = ctx.app.state::<RemoteState>();
        let cache = state.models_cache.lock().unwrap();
        if let Some((at, v)) = cache.get(provider_id) {
            if at.elapsed() < Duration::from_secs(600) {
                return v.clone();
            }
        }
    }
    let id = provider_id.to_string();
    let fetched = crate::run_blocking(move || crate::providers::provider_models(id).unwrap_or_default()).await;
    let state = ctx.app.state::<RemoteState>();
    state.models_cache.lock().unwrap().insert(provider_id.to_string(), (Instant::now(), fetched.clone()));
    fetched
}

/// The sidebar's list, plus what the phone renders as state: `running` (a
/// process holds this session), `open` (a desktop tab is bound to it —
/// input will land), and `activity` for open ones — "working" while the
/// agent reports progress, "attention" when it rang for a person, else
/// "idle". The desktop's terminal is the source of all three.
async fn sessions(State(ctx): State<Ctx>) -> Response {
    let sessions = crate::sessions::list_sessions().await;
    let running = crate::sessions::running_session_ids().await;
    let ptys = ctx.app.state::<crate::pty::PtyManager>();
    let open = ptys.bound_sessions();
    let mut activity: HashMap<String, String> = ptys.activities().into_iter().collect();
    // A terminal that reports nothing (Codex has no progress sequence) is
    // not idle — ask its transcript. Only for sessions with a live process.
    let candidates: Vec<String> = open.iter().chain(running.iter()).cloned().collect();
    let busy = crate::run_blocking(move || {
        candidates
            .into_iter()
            .filter_map(|id| transcript_state(&id).map(|s| (id, s)))
            .collect::<Vec<(String, &'static str)>>()
    })
    .await;
    for (id, st) in busy {
        let e = activity.entry(id).or_insert_with(|| "idle".into());
        if e == "idle" {
            *e = st.into();
        }
    }
    // Which sessions produced files, for the phone's "has files" filter.
    let with_files: Vec<String> = crate::changes::sessions_with_files(&ctx.app).into_iter().collect();
    // Dev servers: listening ports owned by each open session's process
    // tree, so the phone can offer a live preview of what's being built.
    let port_roots: Vec<(String, u32)> = open
        .iter()
        .filter_map(|id| ptys.child_pid_for_session(id).map(|pid| (id.clone(), pid)))
        .collect();
    let ports: HashMap<String, Vec<u16>> = crate::run_blocking(move || {
        port_roots
            .into_iter()
            .filter_map(|(id, pid)| {
                let p = ports_of_tree(pid);
                if p.is_empty() { None } else { Some((id, p)) }
            })
            .collect()
    })
    .await;
    Json(serde_json::json!({
        "sessions": sessions,
        "running": running,
        "open": open,
        "activity": activity,
        "with_files": with_files,
        "ports": ports,
        "stars": crate::sessions::load_stars(),
        "brought_in": crate::sessions::load_brought_in(),
    }))
    .into_response()
}

/// Does the transcript say a turn is in progress? Codex writes
/// `task_started` and `task_complete` events; Claude's last message is the
/// person's until the assistant answers. Reads only the tail of the file.
fn transcript_busy(session_id: &str) -> bool {
    transcript_state(session_id).is_some()
}

/// `Some("working")`, `Some("attention")` — codex mid-approval — or `None`.
/// Public within the crate: the pty layer consults it before believing a
/// quiet terminal means an idle session.
/// Codex writes nothing while its approval prompt is up, so "waiting on a
/// person" is read as: a turn in progress whose last act is a tool call
/// with no output, and a transcript that has gone quiet.
pub(crate) fn transcript_state(session_id: &str) -> Option<&'static str> {
    let list = crate::agents::backends();
    let Some((_, path)) = crate::agents::owner_in(&list, session_id) else { return None };
    let stale = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e > Duration::from_secs(45));
    let Ok(mut f) = std::fs::File::open(&path) else { return None };
    use std::io::{Read, Seek, SeekFrom};
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(128 * 1024);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return None;
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return None;
    }
    let mut state: Option<bool> = None;
    let mut pending_call = false;
    for line in buf.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("event_msg") => match v.pointer("/payload/type").and_then(|t| t.as_str()) {
                Some("task_started") => { state = Some(true); pending_call = false }
                Some("task_complete") | Some("turn_aborted") => { state = Some(false); pending_call = false }
                _ => {}
            },
            Some("response_item") => match v.pointer("/payload/type").and_then(|t| t.as_str()) {
                Some("custom_tool_call") | Some("function_call") => pending_call = true,
                Some("custom_tool_call_output") | Some("function_call_output") => pending_call = false,
                _ => {}
            },
            Some("user") => {
                // A tool result is Claude talking to itself, not a new ask.
                let is_result = v
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                    .is_some_and(|a| a.iter().all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")));
                if !is_result {
                    state = Some(true);
                } else {
                    state = Some(true); // mid-turn: the model has a result to act on
                }
            }
            Some("assistant") => {
                // Text without a tool call ends the turn; a tool call means
                // more to come. Claude nests tool_use in /message/content;
                // grok puts tool_calls at the top of the line.
                let claude_tool = v
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                    .is_some_and(|a| a.iter().any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use")));
                let grok_tool = v
                    .get("tool_calls")
                    .and_then(|c| c.as_array())
                    .is_some_and(|a| !a.is_empty());
                state = Some(claude_tool || grok_tool);
            }
            // Grok writes tool results as their own lines: the model has a
            // result to act on, so the turn is still going.
            Some("tool_result") => state = Some(true),
            _ => {}
        }
    }
    match state {
        Some(true) if pending_call && stale => Some("attention"),
        Some(true) => Some("working"),
        _ => None,
    }
}

/// The same numbers the desktop's usage strip shows: plan limits per engine
/// and provider balances. Slow — it asks each service — so the phone asks
/// rarely and shows the last answer.
async fn usage(State(ctx): State<Ctx>) -> Response {
    let fresh = crate::run_blocking(crate::usage::usage_report).await;
    let state = ctx.app.state::<RemoteState>();
    let mut cache = state.usage_cache.lock().unwrap();
    let report: Vec<crate::usage::UsageSource> = fresh
        .into_iter()
        .map(|u| {
            if u.state == "ok" {
                cache.insert(u.id.clone(), u.clone());
                u
            } else {
                cache.get(&u.id).cloned().unwrap_or(u)
            }
        })
        .collect();
    Json(report).into_response()
}

const UPLOAD_LIMIT: usize = 25 * 1024 * 1024;

/// A file from the phone — a screenshot, a document — lands on the desktop's
/// disk, and the phone puts its path in the message. That is how a CLI agent
/// takes an attachment: by reading it where it sits.
async fn upload(headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let raw = headers.get("x-filename").and_then(|v| v.to_str().ok()).unwrap_or("upload");
    let name: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>()
        .trim_matches('.')
        .chars()
        .take(80)
        .collect();
    let name = if name.is_empty() { "upload".to_string() } else { name };
    if body.len() > UPLOAD_LIMIT {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "25 MB at most");
    }
    let Some(dir) = dirs::data_dir().map(|d| d.join("aiterm").join("uploads")) else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "no data dir");
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("{stamp}-{name}"));
    if let Err(e) = std::fs::write(&path, &body) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    Json(serde_json::json!({ "path": path.to_string_lossy(), "bytes": body.len() })).into_response()
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

/// The desktop's own full-text index over transcripts — the same answer
/// the sidebar's search box gives.
async fn search(Query(q): Query<SearchQuery>) -> Response {
    if q.q.trim().is_empty() {
        return Json(Vec::<crate::sessions::Session>::new()).into_response();
    }
    Json(crate::indexer::search_sessions(q.q).await).into_response()
}

/// Files a session wrote, by tool and time — what the desktop's panel lists.
async fn artifacts(Path(id): Path<String>) -> Response {
    Json(crate::sessions::session_artifacts(id).await).into_response()
}

#[derive(Serialize)]
struct FileEntry {
    path: String,
    name: String,
    bytes: u64,
    /// Unix seconds, last modified.
    modified: u64,
    /// How we know about it: "wrote" (a tool call in the transcript) or
    /// "changed" (newer than the session in its folder).
    via: String,
}

const FILES_CAP: usize = 300;
const FILES_DEPTH: usize = 9;
const SKIP_DIRS: &[&str] = &["node_modules", "target", ".git", ".cache", ".gradle", ".venv", "venv", "__pycache__", "dist", "build", ".next", ".kotlin"];

/// Everything a session produced, as best the desktop can tell: files its
/// transcript says it wrote, plus every file in its folder newer than the
/// session itself. The second list is what makes this work for any
/// harness — Codex, a shell script, an image model — not only the ones
/// whose tool calls we parse. Newest first.
async fn session_files(State(ctx): State<Ctx>, Path(id): Path<String>) -> Response {
    let wrote = crate::sessions::session_artifacts(id.clone()).await;
    let detail = crate::detail::session_detail(id.clone()).await;
    let ledger = crate::changes::for_session(&ctx.app, &id);
    let sid = id;
    let out = crate::run_blocking(move || {
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<FileEntry> = Vec::new();
        for a in wrote {
            if let Some(e) = file_entry(std::path::Path::new(&a.path), "wrote") {
                if seen.insert(e.path.clone()) {
                    out.push(e);
                }
            }
        }
        // The ledger's word first: what the filesystem saw this session do.
        for c in ledger {
            if c.kind == "deleted" {
                continue;
            }
            if let Some(e) = file_entry(std::path::Path::new(&c.path), if c.kind == "created" { "made" } else { "edited" }) {
                if seen.insert(e.path.clone()) {
                    out.push(e);
                }
            }
        }
        // Where a harness puts what it makes outside the project: Codex's
        // image generation writes to its own directory, keyed by session.
        for dir in harness_output_dirs(&sid) {
            walk_recent(&dir, 0, 0, &mut |e| {
                if out.len() < FILES_CAP && seen.insert(e.path.clone()) {
                    out.push(FileEntry { via: "made".into(), ..e });
                }
            });
        }
        if let Some(d) = detail {
            // Files changed while the session was alive — from its start to
            // a little after its last word. A session that ended yesterday
            // does not get credit for what another one did today in the
            // same folder; a session running now keeps gaining files.
            let since = d.started.as_deref().and_then(parse_iso_secs).unwrap_or(0);
            let until = d.last_active.as_deref().and_then(parse_iso_secs).map(|t| t + 15 * 60).unwrap_or(u64::MAX);
            if let Some(cwd) = d.cwd.as_deref() {
                walk_recent(std::path::Path::new(cwd), since, 0, &mut |e| {
                    if e.modified <= until && out.len() < FILES_CAP && seen.insert(e.path.clone()) {
                        out.push(e);
                    }
                });
            }
        }
        // What the agent made or said it wrote comes first; the folder's
        // other changes follow. Newest first within each.
        let rank = |v: &str| match v { "made" => 0, "edited" => 1, "wrote" => 2, _ => 3 };
        out.sort_by(|a, b| rank(&a.via).cmp(&rank(&b.via)).then(b.modified.cmp(&a.modified)));
        out
    })
    .await;
    Json(out).into_response()
}

use crate::changes::harness_output_dirs;

fn file_entry(path: &std::path::Path, via: &str) -> Option<FileEntry> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let modified = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(FileEntry {
        path: path.to_string_lossy().into_owned(),
        name: path.file_name()?.to_string_lossy().into_owned(),
        bytes: md.len(),
        modified,
        via: via.into(),
    })
}

fn walk_recent(dir: &std::path::Path, since: u64, depth: usize, push: &mut dyn FnMut(FileEntry)) {
    if depth > FILES_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            walk_recent(&path, since, depth + 1, push);
        } else if ft.is_file() {
            if let Some(e) = file_entry(&path, "changed") {
                if e.modified >= since {
                    push(e);
                }
            }
        }
    }
}

/// "2026-08-29T14:36:55.009Z" → seconds. Enough of ISO 8601 for a
/// transcript's own timestamps; anything else is "since forever".
fn parse_iso_secs(s: &str) -> Option<u64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = rest.trim_end_matches('Z');
    let time = time.split(['+', '-']).next().unwrap_or(time);
    let mut t = time.split(':');
    let (h, mi) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let sec = t.next().and_then(|x| x.split('.').next()).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    // Days from civil (Howard Hinnant), no calendar crate needed.
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 + sec;
    u64::try_from(secs).ok()
}

/// The ledger's word on a session: every file it created, modified or
/// deleted, as the filesystem saw it. Newest first.
async fn session_changes(State(ctx): State<Ctx>, Path(id): Path<String>) -> Response {
    Json(crate::changes::for_session(&ctx.app, &id)).into_response()
}

/// One directory of a workspace, for the phone's explorer. Same guard as
/// files: only inside a project folder that has sessions.
async fn browse(Query(q): Query<FileQuery>) -> Response {
    let Ok(real) = std::path::PathBuf::from(&q.path).canonicalize() else {
        return err(StatusCode::NOT_FOUND, "no such folder");
    };
    // Directory listings are allowed anywhere under home — picking where a
    // NEW session lives means walking the tree, not only where sessions
    // already are. Serving file *contents* stays gated the strict way.
    if !real.is_dir() || !(under_home(&real) || file_is_allowed(&real).await) {
        return err(StatusCode::FORBIDDEN, "not a browsable folder");
    }
    match crate::fsx::list_dir(real.to_string_lossy().into_owned()).await {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

fn under_home(p: &std::path::Path) -> bool {
    dirs::home_dir().map(|h| p.starts_with(h)).unwrap_or(false)
}

#[derive(Deserialize)]
struct DirBody {
    path: String,
}

/// Make a directory (and its parents) under home — the phone's "new
/// project folder". Textual check on top of the home guard: no `..`.
async fn make_dir(Json(b): Json<DirBody>) -> Response {
    let path = std::path::PathBuf::from(&b.path);
    if b.path.contains("..") || !under_home(&path) {
        return err(StatusCode::FORBIDDEN, "only under the home folder");
    }
    match std::fs::create_dir_all(&path) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

/// Read one file the agent produced, by path, with ranges (video seeks)
/// and a content type from the extension. Only files inside a project
/// folder that has sessions, or in uploads/, are served — the phone sees
/// what the agents make, not the disk.
async fn file(Query(q): Query<FileQuery>, req: Request) -> Response {
    use tower::ServiceExt;
    let path = std::path::PathBuf::from(&q.path);
    let Ok(real) = path.canonicalize() else {
        return err(StatusCode::NOT_FOUND, "no such file");
    };
    if !real.is_file() || !file_is_allowed(&real).await {
        return err(StatusCode::FORBIDDEN, "not a file an agent produced here");
    }
    match tower_http::services::ServeFile::new(&real).oneshot(req).await {
        Ok(r) => r.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn file_is_allowed(real: &std::path::Path) -> bool {
    let mut always: Vec<PathBuf> = Vec::new();
    if let Some(up) = dirs::data_dir().map(|d| d.join("aiterm").join("uploads")) {
        always.push(up);
    }
    if let Some(home) = dirs::home_dir() {
        always.push(home.join(".codex").join("generated_images"));
        always.push(home.join(".grok").join("sessions"));
    }
    // Claude's scratchpads: session-owned throwaway output, phone-viewable.
    if real.to_string_lossy().starts_with("/tmp/claude-") {
        return true;
    }
    for dir in always {
        if let Ok(dir) = dir.canonicalize() {
            if real.starts_with(&dir) {
                return true;
            }
        }
    }
    let roots: Vec<PathBuf> = crate::sessions::list_sessions()
        .await
        .into_iter()
        .flat_map(|s| [PathBuf::from(s.project_path), PathBuf::from(s.group_path)])
        .collect();
    roots.iter().any(|r| r.canonicalize().map(|r| real.starts_with(r)).unwrap_or(false))
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
    let turns = crate::detail::conversation_rich(id, q.max_chars.unwrap_or(60_000)).await;
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

/// Escape: what stops an agent's current turn in every TUI here, without
/// ending the session. A stop is a different, heavier thing (below).
#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

#[derive(Deserialize)]
struct StarBody {
    on: bool,
}

#[derive(Deserialize)]
struct BringInBody {
    agent_id: String,
    model: Option<String>,
    effort: Option<String>,
    focus: Option<String>,
    rounds: Option<u32>,
    auto: Option<bool>,
}

/// The renderer runs the relay; this is how its state reaches the phones.
#[tauri::command]
pub fn relay_report(
    app: tauri::AppHandle,
    session_id: String,
    b_session_id: Option<String>,
    b_name: String,
    phase: String,
    round: u32,
    rounds: u32,
    note: String,
) {
    if let Some(b) = b_session_id.as_deref() {
        crate::sessions::record_brought_in(b, &session_id);
    }
    notify(&app, Event::Relay { session_id, b_session_id, b_name, phase, round, rounds, note });
}

/// The phone asks for a second agent; the desktop's renderer runs the
/// relay (it owns the tabs the two agents talk through). Needs the session
/// open in a tab — the phone opens it first.
async fn bring_in(State(ctx): State<Ctx>, Path(id): Path<String>, Json(b): Json<BringInBody>) -> Response {
    let ptys = ctx.app.state::<crate::pty::PtyManager>();
    if ptys.pty_for_session(&id).is_none() {
        return err(StatusCode::CONFLICT, "open the session on the desktop first");
    }
    // An api:<provider> id is a model off a provider's startup list, not a
    // CLI — the renderer builds the matching StartChoice.
    let (kind, agent_id, provider_id) = match b.agent_id.strip_prefix("api:") {
        Some(pid) => ("api", String::new(), Some(pid.to_string())),
        None => ("agent", b.agent_id.clone(), None),
    };
    if kind == "api" && b.model.is_none() {
        return err(StatusCode::BAD_REQUEST, "an API choice needs a model");
    }
    let _ = ctx.app.emit(
        "remote://bring-in",
        serde_json::json!({
            "session_id": id,
            "kind": kind,
            "agent_id": agent_id,
            "provider_id": provider_id,
            "model": b.model,
            "effort": b.effort,
            "focus": b.focus.unwrap_or_default(),
            "rounds": b.rounds.unwrap_or(2).clamp(1, 3),
            "auto": b.auto.unwrap_or(false),
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Star or unstar from the phone; both UIs re-read.
async fn star(State(ctx): State<Ctx>, Path(id): Path<String>, Json(b): Json<StarBody>) -> Response {
    match crate::sessions::set_star(&id, b.on) {
        Ok(()) => {
            notify(&ctx.app, Event::SessionsChanged);
            let _ = ctx.app.emit("sessions://changed", ());
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// Rename a session from the phone. The same override store the desktop
/// writes; both UIs hear about it and re-read.
async fn rename(State(ctx): State<Ctx>, Path(id): Path<String>, Json(b): Json<RenameBody>) -> Response {
    match crate::sessions::rename_session(&id, &b.title) {
        Ok(()) => {
            notify(&ctx.app, Event::SessionsChanged);
            let _ = ctx.app.emit("sessions://changed", ());
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn interrupt(State(ctx): State<Ctx>, Path(id): Path<String>) -> Response {
    let ptys = ctx.app.state::<crate::pty::PtyManager>();
    let Some(pty) = ptys.pty_for_session(&id) else {
        return err(StatusCode::CONFLICT, "session is not open in a tab");
    };
    match ptys.write_str(pty, "\x1b") {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn stop_session(State(ctx): State<Ctx>, Path(id): Path<String>) -> Response {
    // A session open in a terminal tab is stopped by ending that tab's
    // process — the roster only knows Claude's daemon sessions, so going
    // through it for a tab (any engine) stopped nothing at all.
    let pty = ctx.app.state::<crate::pty::PtyManager>().pty_for_session(&id);
    if let Some(pty_id) = pty {
        let app = ctx.app.clone();
        crate::run_blocking(move || app.state::<crate::pty::PtyManager>().kill_now(pty_id)).await;
        return StatusCode::NO_CONTENT.into_response();
    }
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
    model: Option<String>,
    effort: Option<String>,
    /// A name for the tab, when the person gave one.
    title: Option<String>,
}

async fn new_session(State(ctx): State<Ctx>, Json(body): Json<NewSessionBody>) -> Response {
    let _ = ctx.app.emit(
        "remote://new-session",
        serde_json::json!({
            "agentId": body.agent_id, "cwd": body.cwd, "prompt": body.prompt,
            "model": body.model, "effort": body.effort, "title": body.title,
        }),
    );
    StatusCode::ACCEPTED.into_response()
}

async fn events(
    State(ctx): State<Ctx>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let state = ctx.app.state::<RemoteState>();
    let rx = state.events.subscribe();
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_string();
    let info = ClientInfo {
        id: state.next_client.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        device: h("x-aiterm-device").chars().take(80).collect::<String>().trim().to_string(),
        os: h("x-aiterm-os").chars().take(40).collect(),
        app: h("x-aiterm-app").chars().take(20).collect(),
        address: peer.ip().to_string(),
        since: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    };
    let app = ctx.app.clone();
    ws.on_upgrade(move |socket| async move {
        let id = info.id;
        crate::diag!("remote", "phone connected: {} ({}) from {}", info.device, info.os, info.address);
        app.state::<RemoteState>().clients.lock().unwrap().insert(id, info);
        let _ = app.emit("remote://clients", ());
        stream_events(socket, rx).await;
        app.state::<RemoteState>().clients.lock().unwrap().remove(&id);
        let _ = app.emit("remote://clients", ());
        crate::diag!("remote", "phone disconnected");
    })
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
    fn pem_decodes_to_the_der_it_wraps() {
        let der = b"\x30\x03\x02\x01\x05";
        let pem = "-----BEGIN CERTIFICATE-----\nMAMCAQU=\n-----END CERTIFICATE-----\n";
        assert_eq!(pem_to_der(pem.as_bytes()).unwrap(), der);
    }

    #[test]
    fn iso_timestamps_become_unix_seconds() {
        assert_eq!(parse_iso_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_secs("2026-08-29T14:36:55.009Z"), Some(1788014215));
    }

    #[test]
    fn the_name_survives_a_uri() {
        assert_eq!(percent_encode("john-laptop"), "john-laptop");
        assert_eq!(percent_encode("John's PC"), "John%27s%20PC");
    }
}
