pub mod auth;
pub mod model;
pub mod relay;
pub mod server;
pub mod terminal;
pub mod uploads;

use auth::{DeviceStore, PendingPairing, TrustedDevice};
use qrcode::{EcLevel, QrCode};
use relay::{RelayConfig, RelayConnectionState, RelayConnectorHandle, RelayEnrollmentDraft};
use serde::Serialize;
use server::{
    GatewayHandle, RemoteGateway, RemoteServices, TlsIdentity, MAX_ADVERTISED_HOSTS,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// The pairing payload version. A phone that does not know this number stops
/// rather than guessing at a field layout that governs trust.
pub const PAIRING_VERSION: u8 = 1;
pub const RELAY_PAIRING_VERSION: u8 = 2;
pub const RELAY_AUTH_PAIRING_VERSION: u8 = 3;
pub(crate) const DEFAULT_RELAY_SERVER: &str = "https://control.34-23-107-73.sslip.io:8443";
const DEFAULT_RELAY_PUBLIC_DOMAIN: &str = "34-23-107-73.sslip.io";
const ENROLLMENT_LIFETIME: Duration = Duration::from_secs(300);

// --- The pairing URI ---------------------------------------------------

/// A parsed `aiterm://pair` payload.
///
/// The desktop both writes and reads this: the tests parse what the desktop
/// emits, so the two phones-eye and desktop-eye views of the format cannot
/// drift apart silently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingUri {
    pub version: u8,
    /// Candidate addresses in the desktop's preferred order.
    pub hosts: Vec<String>,
    pub port: u16,
    /// base64url SHA-256 of the listener's SPKI.
    pub fingerprint: String,
    pub secret: Vec<u8>,
    pub name: String,
    pub relay_host: Option<String>,
    pub relay_port: Option<u16>,
    pub relay_authorization_digest: Option<Vec<u8>>,
}

impl PairingUri {
    pub fn parse(payload: &str) -> Option<Self> {
        let query = payload.strip_prefix("aiterm://pair?")?;
        let mut version = None;
        let mut hosts = Vec::new();
        let mut port = None;
        let mut fingerprint = None;
        let mut secret = None;
        let mut name = String::new();
        let mut relay_host = None;
        let mut relay_port = None;
        let mut relay_authorization_digest = None;

        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            let value = percent_decode(value)?;
            match key {
                "v" => version = value.parse::<u8>().ok(),
                "h" => {
                    if hosts.len() == MAX_ADVERTISED_HOSTS {
                        return None;
                    }
                    hosts.push(value);
                }
                "p" => port = value.parse::<u16>().ok(),
                "f" => fingerprint = Some(value),
                "s" => {
                    secret = base64::Engine::decode(
                        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                        value,
                    )
                    .ok()
                }
                "n" => name = value,
                "r" => relay_host = Some(value),
                "q" => relay_port = value.parse::<u16>().ok(),
                "a" => {
                    relay_authorization_digest = base64::Engine::decode(
                        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                        value,
                    ).ok()
                }
                // An unknown key means a payload written by a build that knows
                // something this one does not. Ignoring it is safe only
                // because every field that grants trust is required below.
                _ => {}
            }
        }

        let version = version?;
        if !matches!(version, PAIRING_VERSION | RELAY_PAIRING_VERSION | RELAY_AUTH_PAIRING_VERSION)
            || hosts.is_empty()
        {
            return None;
        }
        let has_relay = relay_host.is_some() && relay_port.is_some();
        if (relay_host.is_some() != relay_port.is_some())
            || (matches!(version, RELAY_PAIRING_VERSION | RELAY_AUTH_PAIRING_VERSION) != has_relay)
            || ((version == RELAY_AUTH_PAIRING_VERSION)
                != relay_authorization_digest.as_ref().is_some_and(|value| value.len() == 32))
        {
            return None;
        }
        Some(Self {
            version,
            hosts,
            port: port?,
            fingerprint: fingerprint?,
            secret: secret?,
            name,
            relay_host,
            relay_port,
            relay_authorization_digest,
        })
    }
}

/// Build the payload a QR encodes.
///
/// Addresses repeat as separate `h` parameters rather than sharing a
/// delimited field: an IPv6 literal is full of colons, and any separator
/// chosen for one address family eventually corrupts the other.
pub fn pairing_payload(
    hosts: &[IpAddr],
    port: u16,
    fingerprint: &str,
    secret: &[u8],
    name: &str,
) -> String {
    pairing_payload_with_relay(hosts, port, fingerprint, secret, name, None)
}

pub fn pairing_payload_with_relay(
    hosts: &[IpAddr],
    port: u16,
    fingerprint: &str,
    secret: &[u8],
    name: &str,
    relay: Option<(&str, u16)>,
) -> String {
    pairing_payload_with_relay_authorization(hosts, port, fingerprint, secret, name, relay, None)
}

pub fn pairing_payload_with_relay_authorization(
    hosts: &[IpAddr],
    port: u16,
    fingerprint: &str,
    secret: &[u8],
    name: &str,
    relay: Option<(&str, u16)>,
    relay_authorization_digest: Option<&[u8; 32]>,
) -> String {
    let version = if relay_authorization_digest.is_some() {
        RELAY_AUTH_PAIRING_VERSION
    } else if relay.is_some() {
        RELAY_PAIRING_VERSION
    } else {
        PAIRING_VERSION
    };
    let mut payload = format!("aiterm://pair?v={version}");
    for host in hosts {
        payload.push_str("&h=");
        payload.push_str(&percent_encode(&host.to_string()));
    }
    payload.push_str(&format!("&p={port}"));
    payload.push_str("&f=");
    payload.push_str(&percent_encode(fingerprint));
    payload.push_str("&s=");
    payload.push_str(&base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        secret,
    ));
    payload.push_str("&n=");
    payload.push_str(&percent_encode(name));
    if let Some((host, relay_port)) = relay {
        payload.push_str("&r=");
        payload.push_str(&percent_encode(host));
        payload.push_str(&format!("&q={relay_port}"));
    }
    if let Some(digest) = relay_authorization_digest {
        payload.push_str("&a=");
        payload.push_str(&base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            digest,
        ));
    }
    payload
}

/// Render a payload to a bare SVG element.
///
/// Rendered here rather than in the renderer process on purpose: this is the
/// only form in which the enrollment secret reaches the UI. A secret that
/// exists as a string in the webview can be lifted out of devtools, a
/// clipboard, or a crash report; one that only ever existed as drawn
/// geometry cannot.
///
/// Error correction is deliberately `M` rather than the densest level: a
/// phone camera reading a screen at an angle, through glare, needs the
/// redundancy more than the payload needs the smaller image.
pub fn pairing_qr_svg(payload: &str) -> Option<String> {
    let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::M).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    // The prolog is valid in a standalone file and invalid inside a DOM node,
    // which is where this is going.
    Some(match svg.find("<svg") {
        Some(start) => svg[start..].to_string(),
        None => svg,
    })
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

// --- Where the gateway may listen --------------------------------------

/// The addresses worth offering as bind candidates, in the order given.
///
/// Loopback is excluded because a phone cannot reach it, and a link-local
/// address because nothing routes a phone over one. Offering either produces
/// a listener that starts cleanly and then cannot be connected to, which is
/// the most expensive kind of failure to diagnose.
pub fn shareable_addresses(found: Vec<IpAddr>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for address in found {
        if !is_shareable_address(address) {
            continue;
        }
        let text = address.to_string();
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

fn is_shareable_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
        IpAddr::V6(v6) => {
            !v6.is_loopback() && !v6.is_unspecified() && (v6.segments()[0] & 0xffc0) != 0xfe80
        }
    }
}

/// Freeze the exact, preferred-order hosts a listener certificate and every
/// invite created during that listener's lifetime will share.
fn advertised_hosts(selected: IpAddr, found: Vec<IpAddr>) -> Result<Vec<IpAddr>, String> {
    if !is_shareable_address(selected) {
        return Err("remote access will not bind loopback or a link-local address".into());
    }

    let mut hosts = vec![selected];
    for candidate in found {
        if !is_shareable_address(candidate) || hosts.contains(&candidate) {
            continue;
        }
        if hosts.len() == MAX_ADVERTISED_HOSTS {
            return Err(format!(
                "remote access found more than {MAX_ADVERTISED_HOSTS} shareable addresses"
            ));
        }
        hosts.push(candidate);
    }
    Ok(hosts)
}

fn local_addresses() -> Vec<IpAddr> {
    if_addrs::get_if_addrs()
        .map(|interfaces| interfaces.into_iter().map(|item| item.ip()).collect())
        .unwrap_or_default()
}

fn desktop_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "AITerm desktop".to_string())
}

pub(crate) fn state_root() -> Result<std::path::PathBuf, String> {
    dirs::data_dir()
        .map(|dir| dir.join("aiterm/remote"))
        .ok_or_else(|| "no data directory for remote access state".to_string())
}

fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// --- Desktop state and commands ----------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RemoteStatusView {
    pub enabled: bool,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub fingerprint: Option<String>,
    pub relay: RelayStatusView,
}

#[derive(Clone, Debug, Serialize)]
pub struct RelayStatusView {
    pub configured: bool,
    pub connector_url: Option<String>,
    pub public_host: Option<String>,
    pub public_port: Option<u16>,
    pub route_id: Option<String>,
    pub state: RelayConnectionState,
}

#[derive(Clone, Debug, Serialize)]
pub struct PairingInviteView {
    pub svg: String,
    /// Epoch milliseconds, from the clock the gateway expires the secret on.
    pub expires_at: u64,
}

#[derive(Default)]
struct Inner {
    /// Opened on first use. A user who never turns remote access on never
    /// gets a trusted-device file created behind their back.
    devices: Option<Arc<DeviceStore>>,
    gateway: Option<GatewayHandle>,
    bound: Option<SocketAddr>,
    fingerprint: Option<String>,
    advertised_hosts: Option<Vec<IpAddr>>,
    relay_config: Option<RelayConfig>,
    relay_config_loaded: bool,
    relay: Option<RelayConnectorHandle>,
    starting: bool,
}

#[derive(Default)]
pub struct RemoteState {
    inner: Mutex<Inner>,
}

impl Inner {
    fn load_relay_config(&mut self) -> Result<(), String> {
        if !self.relay_config_loaded {
            self.relay_config = RelayConfig::load(&state_root()?)?;
            self.relay_config_loaded = true;
        }
        Ok(())
    }

    fn devices(&mut self) -> Result<Arc<DeviceStore>, String> {
        if let Some(devices) = &self.devices {
            return Ok(devices.clone());
        }
        let store =
            DeviceStore::open(state_root()?.join("devices")).map_err(|error| error.to_string())?;
        let store = Arc::new(store);
        self.devices = Some(store.clone());
        Ok(store)
    }

    fn status(&self) -> RemoteStatusView {
        RemoteStatusView {
            enabled: self.gateway.is_some(),
            address: self.bound.map(|addr| addr.ip().to_string()),
            port: self.bound.map(|addr| addr.port()),
            fingerprint: self.fingerprint.clone(),
            relay: RelayStatusView {
                configured: self.relay_config.is_some(),
                connector_url: self.relay_config.as_ref().map(|value| value.connector_url.clone()),
                public_host: self.relay_config.as_ref().map(|value| value.public_host.clone()),
                public_port: self.relay_config.as_ref().map(|value| value.public_port),
                route_id: self.relay_config.as_ref().map(|value| value.route_id.clone()),
                state: self.relay.as_ref().map(RelayConnectorHandle::state).unwrap_or_default(),
            },
        }
    }

    #[cfg(test)]
    fn pairing_payload(&self, secret: &[u8], name: &str) -> Result<String, String> {
        self.pairing_payload_with_draft(secret, name, None)
    }

    fn pairing_payload_with_draft(
        &self,
        secret: &[u8],
        name: &str,
        draft: Option<&RelayEnrollmentDraft>,
    ) -> Result<String, String> {
        let (Some(bound), Some(fingerprint), Some(hosts)) = (
            self.bound,
            self.fingerprint.as_deref(),
            self.advertised_hosts.as_deref(),
        ) else {
            return Err("turn remote access on before pairing a phone".into());
        };
        if hosts.is_empty()
            || hosts.len() > MAX_ADVERTISED_HOSTS
            || hosts.first().copied() != Some(bound.ip())
        {
            return Err("remote listener advertisement state is inconsistent".into());
        }
        let relay = self.relay_config.as_ref()
            .map(|config| (config.public_host.as_str(), config.public_port))
            .or_else(|| draft.map(RelayEnrollmentDraft::public_endpoint));
        Ok(pairing_payload_with_relay_authorization(
            hosts,
            bound.port(),
            fingerprint,
            secret,
            name,
            relay,
            draft.map(RelayEnrollmentDraft::authorization_digest),
        ))
    }
}

#[tauri::command]
pub async fn remote_status(
    state: tauri::State<'_, RemoteState>,
) -> Result<RemoteStatusView, String> {
    let mut inner = state.inner.lock().await;
    inner.load_relay_config()?;
    Ok(inner.status())
}

#[tauri::command]
pub async fn remote_relay_configure(
    state: tauri::State<'_, RemoteState>,
    connector_url: String,
    public_host: String,
    public_port: u16,
    route_id: String,
    token: Option<String>,
) -> Result<RemoteStatusView, String> {
    let mut inner = state.inner.lock().await;
    inner.load_relay_config()?;
    if inner.gateway.is_some() || inner.starting {
        return Err("turn remote access off before changing relay settings".into());
    }
    let token = token.filter(|value| !value.is_empty())
        .or_else(|| inner.relay_config.as_ref().map(|value| value.token.clone()))
        .ok_or_else(|| "enter the relay connector token".to_string())?;
    let config = RelayConfig {
        connector_url,
        public_host,
        public_port,
        route_id,
        token,
        managed: false,
    };
    config.save(&state_root()?)?;
    inner.relay_config = Some(config);
    inner.relay_config_loaded = true;
    Ok(inner.status())
}

#[tauri::command]
pub async fn remote_relay_clear(
    state: tauri::State<'_, RemoteState>,
) -> Result<RemoteStatusView, String> {
    let config = {
        let mut inner = state.inner.lock().await;
        inner.load_relay_config()?;
        if inner.gateway.is_some() || inner.starting {
            return Err("turn remote access off before removing relay settings".into());
        }
        inner.starting = true;
        inner.relay_config.clone()
    };
    let removal = match config {
        Some(config) => config.deprovision().await,
        None => Ok(()),
    };
    let mut inner = state.inner.lock().await;
    inner.starting = false;
    removal?;
    let path = state_root()?.join("relay.json");
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("could not remove relay settings: {error}")),
    }
    inner.relay_config = None;
    inner.relay_config_loaded = true;
    Ok(inner.status())
}

#[tauri::command]
pub async fn remote_interfaces(
    _state: tauri::State<'_, RemoteState>,
) -> Result<Vec<String>, String> {
    Ok(shareable_addresses(local_addresses()))
}

#[tauri::command]
pub async fn remote_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, RemoteState>,
    tabs: tauri::State<'_, Arc<crate::tabs::TabRegistry>>,
    services: tauri::State<'_, crate::services::ApplicationServices>,
    address: String,
    port: u16,
) -> Result<RemoteStatusView, String> {
    let ip: IpAddr = address
        .parse()
        .map_err(|_| format!("{address} is not an address this desktop can bind"))?;
    if !is_shareable_address(ip) {
        return Err("remote access will not bind loopback or a link-local address".into());
    }
    if port < 1024 {
        return Err("remote access port must be between 1024 and 65535".into());
    }
    let devices = {
        let mut inner = state.inner.lock().await;
        inner.load_relay_config()?;
        if inner.gateway.is_some() {
            return Ok(inner.status());
        }
        if inner.starting {
            return Err("remote access is already starting".to_string());
        }
        inner.starting = true;
        match inner.devices() {
            Ok(devices) => devices,
            Err(error) => {
                inner.starting = false;
                return Err(error);
            }
        }
    };
    let relay_config = state.inner.lock().await.relay_config.clone();
    let advertised_hosts = match advertised_hosts(ip, local_addresses()) {
        Ok(hosts) => hosts,
        Err(error) => {
            state.inner.lock().await.starting = false;
            return Err(error);
        }
    };
    let identity = match state_root().and_then(|root| {
        let relay_dns = relay_config.as_ref()
            .map(|config| vec![config.public_host.clone()])
            .unwrap_or_else(|| vec![format!("*.{DEFAULT_RELAY_PUBLIC_DOMAIN}")]);
        TlsIdentity::load_or_create_with_dns(root.join("tls"), &advertised_hosts, &relay_dns)
            .map_err(|e| e.to_string())
    }) {
        Ok(identity) => identity,
        Err(error) => {
            state.inner.lock().await.starting = false;
            return Err(error);
        }
    };
    // Bind the selected address family, not one interface. The selected IP
    // stays the preferred route while LAN/VPN transitions keep working.
    let listen_ip = match ip {
        IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
    };
    let relay_host = relay_config.as_ref().map(|config| config.public_host.clone());
    let relay_port = relay_config.as_ref().map(|config| config.public_port);
    let routes = advertised_hosts.iter().map(ToString::to_string).collect();
    let started = RemoteGateway::start(
        SocketAddr::new(listen_ip, port),
        devices,
        identity,
        RemoteServices::from_application_services(tabs.inner().clone(), services.inner())
            .with_app_handle(app)
            .with_gateway_routes(routes, port, relay_host, relay_port),
    )
    .await;
    let mut inner = state.inner.lock().await;
    inner.starting = false;
    let gateway = started.map_err(|error| error.to_string())?;
    let gateway_port = gateway.local_addr().port();
    let local_target = SocketAddr::new(ip, gateway_port);
    inner.bound = Some(local_target);
    inner.fingerprint = Some(gateway.spki_fingerprint().to_string());
    inner.advertised_hosts = Some(advertised_hosts);
    if let Some(config) = relay_config {
        inner.relay = Some(RelayConnectorHandle::start(config, local_target));
    }
    inner.gateway = Some(gateway);
    Ok(inner.status())
}

#[tauri::command]
pub async fn remote_stop(state: tauri::State<'_, RemoteState>) -> Result<RemoteStatusView, String> {
    let (gateway, relay) = {
        let mut inner = state.inner.lock().await;
        if inner.starting {
            return Err("remote access is still starting".to_string());
        }
        inner.bound = None;
        inner.fingerprint = None;
        inner.advertised_hosts = None;
        (inner.gateway.take(), inner.relay.take())
    };
    // Closing the listener is not a statement about any phone: trusted
    // devices stay trusted, and revocation stays an explicit act.
    if let Some(relay) = relay { relay.stop().await; }
    if let Some(gateway) = gateway {
        gateway.stop().await.map_err(|error| error.to_string())?;
    }
    Ok(state.inner.lock().await.status())
}

#[tauri::command]
pub async fn remote_begin_pairing(
    state: tauri::State<'_, RemoteState>,
) -> Result<PairingInviteView, String> {
    let (payload, now) = begin_pairing_payload(&state).await?;
    let svg = pairing_qr_svg(&payload).ok_or("the pairing payload could not be rendered")?;
    Ok(PairingInviteView {
        svg,
        expires_at: unix_millis(now + ENROLLMENT_LIFETIME),
    })
}

/// Mint the gateway's own pairing payload — enrollment secret, relay draft
/// when no relay route exists yet — and return it with the instant the
/// enrollment began. Both pairing commands build on this so a combined QR
/// carries exactly what a plain one does.
async fn begin_pairing_payload(
    state: &tauri::State<'_, RemoteState>,
) -> Result<(String, SystemTime), String> {
    let (devices, fingerprint, needs_relay) = {
        let mut inner = state.inner.lock().await;
        inner.load_relay_config()?;
        if inner.gateway.is_none() {
            return Err("turn remote access on before pairing a phone".into());
        }
        (
            inner.devices()?,
            inner.fingerprint.clone().ok_or("the remote identity is unavailable")?,
            inner.relay_config.is_none(),
        )
    };
    let draft = if needs_relay {
        match RelayConfig::prepare_enrollment(DEFAULT_RELAY_SERVER, &fingerprint).await {
            Ok(draft) => Some(draft),
            Err(error) => {
                tracing::warn!(error = %error, "relay setup was unavailable during pairing");
                None
            }
        }
    } else {
        None
    };
    let now = SystemTime::now();
    let enrollment = devices
        .begin_enrollment_with_relay_at(now, draft)
        .map_err(|error| error.to_string())?;
    let inner = state.inner.lock().await;
    if inner.gateway.is_none() {
        return Err("remote access stopped while the pairing code was being prepared".into());
    }
    let payload = inner.pairing_payload_with_draft(
        enrollment.secret(),
        &desktop_name(),
        enrollment.relay(),
    )?;
    Ok((payload, now))
}

/// One QR that pairs either phone app. The gateway's own enrollment payload
/// (`v`/`h`/`p`/`f`/`s`/`n`) is minted exactly as `remote_begin_pairing` does
/// it; the phone-listener's fields ride behind under their own names
/// (`tp`/`tt`/`tf`/`z`) when that listener is running. Each app reads its own
/// fields and — with the tolerant parsers — skips the other's. Rendered to
/// SVG here for the same reason as the plain invite: neither secret may exist
/// as a string in the webview.
#[tauri::command]
pub async fn remote_begin_pairing_combined(
    app: tauri::AppHandle,
    state: tauri::State<'_, RemoteState>,
) -> Result<PairingInviteView, String> {
    let (mut payload, now) = begin_pairing_payload(&state).await?;
    if let Some(ext) = crate::remote_api::pair_extension(&app).await {
        payload.push_str(&ext);
    }
    let svg = pairing_qr_svg(&payload).ok_or("the pairing payload could not be rendered")?;
    Ok(PairingInviteView {
        svg,
        expires_at: unix_millis(now + ENROLLMENT_LIFETIME),
    })
}

#[tauri::command]
pub async fn remote_pending_pairings(
    state: tauri::State<'_, RemoteState>,
) -> Result<Vec<PendingPairing>, String> {
    let devices = state.inner.lock().await.devices()?;
    devices.prune_at(SystemTime::now());
    Ok(devices.list_pending_pairings())
}

#[tauri::command]
pub async fn remote_approve_device(
    state: tauri::State<'_, RemoteState>,
    request_id: String,
) -> Result<TrustedDevice, String> {
    let devices = state.inner.lock().await.devices()?;
    let relay_enrollment = devices
        .pending_relay_enrollment(&request_id)
        .map_err(|error| error.to_string())?;
    let device = devices
        .approve_pairing_at(&request_id, SystemTime::now())
        .map_err(|error| error.to_string())?;
    if let Some(enrollment) = relay_enrollment {
        let should_register = {
            let mut inner = state.inner.lock().await;
            inner.load_relay_config()?;
            inner.relay_config.is_none()
        };
        if should_register {
            match enrollment.draft.register(
                &enrollment.authority_public_key,
                &enrollment.signature_der,
            ).await {
                Ok(config) => {
                    config.save(&state_root()?)?;
                    let mut inner = state.inner.lock().await;
                    if inner.relay_config.is_none() {
                        if let Some(gateway) = &inner.gateway {
                            gateway.set_relay_route(config.public_host.clone(), config.public_port);
                        }
                        if let Some(local_target) = inner.bound {
                            inner.relay = Some(RelayConnectorHandle::start(config.clone(), local_target));
                        }
                        inner.relay_config = Some(config);
                        inner.relay_config_loaded = true;
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "phone pairing succeeded but relay registration failed");
                }
            }
        }
    }
    Ok(device)
}

#[tauri::command]
pub async fn remote_deny_device(
    state: tauri::State<'_, RemoteState>,
    request_id: String,
) -> Result<bool, String> {
    let devices = state.inner.lock().await.devices()?;
    devices
        .deny_pairing_at(&request_id, SystemTime::now())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn remote_devices(
    state: tauri::State<'_, RemoteState>,
) -> Result<Vec<TrustedDevice>, String> {
    let devices = state.inner.lock().await.devices()?;
    Ok(devices.list_devices())
}

#[tauri::command]
pub async fn remote_revoke_device(
    state: tauri::State<'_, RemoteState>,
    device_id: String,
) -> Result<bool, String> {
    let devices = state.inner.lock().await.devices()?;
    devices
        .revoke(&device_id)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod listener_advertisement_tests {
    use super::*;

    #[test]
    fn pairing_invite_hosts_are_the_exact_frozen_start_host_list() {
        let selected: IpAddr = "10.0.0.151".parse().unwrap();
        let frozen = advertised_hosts(
            selected,
            vec![
                "192.168.1.99".parse().unwrap(),
                "10.8.0.2".parse().unwrap(),
                selected,
                "10.8.0.2".parse().unwrap(),
            ],
        )
        .unwrap();
        let mut inner = Inner {
            bound: Some(SocketAddr::new(selected, 8443)),
            fingerprint: Some("stable-spki-pin".to_string()),
            ..Inner::default()
        };
        inner.advertised_hosts = Some(frozen);

        // This represents a later interface scan after the listener is live.
        // It must not change an invite produced from the listener's state.
        let later_scan = advertised_hosts(
            selected,
            vec![selected, "172.16.40.7".parse().unwrap()],
        )
        .unwrap();
        assert_eq!(
            later_scan,
            vec![selected, "172.16.40.7".parse().unwrap()]
        );

        let payload = inner
            .pairing_payload(b"enrollment-secret", "desktop")
            .unwrap();
        let invite = PairingUri::parse(&payload).unwrap();
        assert_eq!(
            invite.hosts,
            vec!["10.0.0.151", "192.168.1.99", "10.8.0.2"],
            "an invite must use only the deduplicated hosts frozen at listener start"
        );
    }

    #[test]
    fn relay_invite_is_versioned_and_keeps_direct_routes() {
        let hosts = vec!["192.168.1.20".parse().unwrap(), "100.90.1.2".parse().unwrap()];
        let payload = pairing_payload_with_relay(
            &hosts,
            8443,
            "stable-pin",
            b"secret",
            "desktop",
            Some(("desk-1234.relay.example.com", 443)),
        );
        let parsed = PairingUri::parse(&payload).unwrap();
        assert_eq!(parsed.version, RELAY_PAIRING_VERSION);
        assert_eq!(parsed.hosts, vec!["192.168.1.20", "100.90.1.2"]);
        assert_eq!(parsed.relay_host.as_deref(), Some("desk-1234.relay.example.com"));
        assert_eq!(parsed.relay_port, Some(443));
    }

    #[test]
    fn phone_authorized_relay_invite_binds_a_32_byte_digest() {
        let hosts = ["192.168.1.20".parse().unwrap()];
        let digest = [7u8; 32];
        let payload = pairing_payload_with_relay_authorization(
            &hosts,
            8443,
            "fingerprint",
            b"enrollment-secret",
            "desktop",
            Some(("desktop-1234.relay.example.com", 443)),
            Some(&digest),
        );
        let parsed = PairingUri::parse(&payload).unwrap();
        assert_eq!(parsed.version, RELAY_AUTH_PAIRING_VERSION);
        assert_eq!(parsed.relay_authorization_digest.as_deref(), Some(digest.as_slice()));
    }
}
