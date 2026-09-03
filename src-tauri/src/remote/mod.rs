pub mod auth;
pub mod direct;
pub mod model;
pub mod relay;
pub mod server;
pub mod terminal;
pub mod uploads;

use auth::{
    set_private_permissions, write_private_file, DeviceStore, PendingPairing, TrustedDevice,
};
use direct::DirectTunnelService;
use qrcode::{EcLevel, QrCode};
use relay::{
    RelayConfig, RelayConnectionState, RelayConnectorHandle, RelayEnrollmentDraft,
    RelayServerConfig,
};
use serde::{Deserialize, Serialize};
use server::{GatewayHandle, RemoteGateway, RemoteServices, TlsIdentity, MAX_ADVERTISED_HOSTS};
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
const STARTUP_CONFIG_FILE: &str = "startup.json";
const DEFAULT_REMOTE_PORT: u16 = 8443;
const ENROLLMENT_LIFETIME: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteNetworkStack {
    #[default]
    Aiterm,
    Iroh,
}

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
                    )
                    .ok()
                }
                // An unknown key means a payload written by a build that knows
                // something this one does not. Ignoring it is safe only
                // because every field that grants trust is required below.
                _ => {}
            }
        }

        let version = version?;
        if !matches!(
            version,
            PAIRING_VERSION | RELAY_PAIRING_VERSION | RELAY_AUTH_PAIRING_VERSION
        ) || hosts.is_empty()
        {
            return None;
        }
        let has_relay = relay_host.is_some() && relay_port.is_some();
        if (relay_host.is_some() != relay_port.is_some())
            || (matches!(version, RELAY_PAIRING_VERSION | RELAY_AUTH_PAIRING_VERSION) != has_relay)
            || ((version == RELAY_AUTH_PAIRING_VERSION)
                != relay_authorization_digest
                    .as_ref()
                    .is_some_and(|value| value.len() == 32))
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

/// The only remote state restored when AITerm opens. Relay credentials remain
/// in `relay.json`; this file records the user's explicit decision to bring
/// the listener and that saved private route back online automatically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteStartupConfig {
    enabled: bool,
    address: String,
    port: u16,
    #[serde(default)]
    network_stack: RemoteNetworkStack,
    #[serde(default)]
    iroh_secret: String,
    #[serde(default)]
    iroh_relay_url: Option<String>,
}

impl Default for RemoteStartupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            address: String::new(),
            port: DEFAULT_REMOTE_PORT,
            network_stack: RemoteNetworkStack::Aiterm,
            iroh_secret: String::new(),
            iroh_relay_url: None,
        }
    }
}

impl RemoteStartupConfig {
    fn validated(self) -> Result<Self, String> {
        if self.port < 1024 {
            return Err("remote access port must be between 1024 and 65535".into());
        }
        if self.enabled {
            let address: IpAddr = self
                .address
                .parse()
                .map_err(|_| "the saved remote access address is invalid".to_string())?;
            if !is_shareable_address(address) {
                return Err("remote access will not bind loopback or a link-local address".into());
            }
        }
        if let Some(url) = self.iroh_relay_url.as_deref() {
            url.parse::<iroh::RelayUrl>()
                .map_err(|error| format!("invalid Iroh relay URL: {error}"))?;
        }
        Ok(self)
    }

    fn load(root: &std::path::Path) -> Result<Self, String> {
        let path = root.join(STARTUP_CONFIG_FILE);
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(format!("could not read remote startup setting: {error}")),
        };
        serde_json::from_slice::<Self>(&bytes)
            .map_err(|error| format!("remote startup setting is corrupt: {error}"))?
            .validated()
    }

    fn save(&self, root: &std::path::Path) -> Result<(), String> {
        let validated = self.clone().validated()?;
        std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
        set_private_permissions(root, 0o700).map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec_pretty(&validated).map_err(|error| error.to_string())?;
        let temporary = root.join(format!(
            ".{STARTUP_CONFIG_FILE}.{}.tmp",
            uuid::Uuid::new_v4()
        ));
        write_private_file(&temporary, &bytes).map_err(|error| error.to_string())?;
        if let Err(error) = std::fs::rename(&temporary, root.join(STARTUP_CONFIG_FILE)) {
            let _ = std::fs::remove_file(temporary);
            return Err(error.to_string());
        }
        Ok(())
    }
}

/// Prefer the address the person selected, but do not let a missing VPN or a
/// changed DHCP lease strand the relay after a reboot. The listener binds the
/// whole address family; this chosen address is its local connector target.
fn startup_listener(
    config: &RemoteStartupConfig,
    found: Vec<IpAddr>,
) -> Result<(String, u16), String> {
    let shareable = shareable_addresses(found);
    let address = shareable
        .iter()
        .find(|candidate| candidate.as_str() == config.address)
        .or_else(|| shareable.first())
        .cloned()
        .ok_or_else(|| "remote access found no shareable LAN or VPN address".to_string())?;
    Ok((address, config.port))
}

fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn relay_certificate_dns(
    relay_config: Option<&RelayConfig>,
    relay_server: Option<&RelayServerConfig>,
) -> Vec<String> {
    relay_config
        .map(|config| vec![config.public_host.clone()])
        .or_else(|| relay_server.map(|server| vec![format!("*.{}", server.public_domain)]))
        .unwrap_or_else(|| vec![format!("*.{DEFAULT_RELAY_PUBLIC_DOMAIN}")])
}

// --- Desktop state and commands ----------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RemoteStatusView {
    pub enabled: bool,
    pub start_on_launch: bool,
    pub network_stack: RemoteNetworkStack,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub fingerprint: Option<String>,
    pub relay_server: String,
    pub relay: RelayStatusView,
    pub iroh_node: Option<String>,
    pub iroh_active: bool,
    pub iroh_relay_url: Option<String>,
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
    relay_server: Option<RelayServerConfig>,
    relay_config_loaded: bool,
    relay: Option<RelayConnectorHandle>,
    iroh: Option<crate::iroh_tunnel::Tunnel>,
    startup: RemoteStartupConfig,
    startup_loaded: bool,
    starting: bool,
}

#[derive(Default)]
pub struct RemoteState {
    inner: Mutex<Inner>,
}

impl Inner {
    fn load_relay_config(&mut self) -> Result<(), String> {
        if !self.relay_config_loaded {
            let root = state_root()?;
            self.relay_config = RelayConfig::load(&root)?;
            self.relay_server = Some(match RelayServerConfig::load(&root)? {
                Some(server) => server,
                None => match self
                    .relay_config
                    .as_ref()
                    .and_then(RelayServerConfig::from_route)
                {
                    Some(server) => server,
                    None => {
                        RelayServerConfig::known(DEFAULT_RELAY_SERVER, DEFAULT_RELAY_PUBLIC_DOMAIN)?
                    }
                },
            });
            self.relay_config_loaded = true;
        }
        if !self.startup_loaded {
            self.startup = match RemoteStartupConfig::load(&state_root()?) {
                Ok(config) => config,
                Err(error) => {
                    // This preference grants no trust. Fail closed without
                    // preventing manual remote access from being repaired.
                    crate::diag!("remote", "ignoring invalid launch setting: {error}");
                    RemoteStartupConfig::default()
                }
            };
            self.startup_loaded = true;
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
            start_on_launch: self.startup.enabled,
            network_stack: self.startup.network_stack,
            address: self.bound.map(|addr| addr.ip().to_string()),
            port: self.bound.map(|addr| addr.port()),
            fingerprint: self.fingerprint.clone(),
            relay_server: self
                .relay_server
                .as_ref()
                .map(|value| value.control_origin.clone())
                .unwrap_or_else(|| DEFAULT_RELAY_SERVER.to_string()),
            relay: RelayStatusView {
                configured: self.relay_config.is_some(),
                connector_url: self
                    .relay_config
                    .as_ref()
                    .map(|value| value.connector_url.clone()),
                public_host: self
                    .relay_config
                    .as_ref()
                    .map(|value| value.public_host.clone()),
                public_port: self.relay_config.as_ref().map(|value| value.public_port),
                route_id: self
                    .relay_config
                    .as_ref()
                    .map(|value| value.route_id.clone()),
                state: self
                    .relay
                    .as_ref()
                    .map(RelayConnectorHandle::state)
                    .unwrap_or_default(),
            },
            iroh_node: crate::iroh_tunnel::node_id_of(&self.startup.iroh_secret),
            iroh_active: self.iroh.is_some(),
            iroh_relay_url: self.startup.iroh_relay_url.clone(),
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
        let relay = (self.startup.network_stack == RemoteNetworkStack::Aiterm)
            .then(|| {
                self.relay_config
                    .as_ref()
                    .map(|config| (config.public_host.as_str(), config.public_port))
                    .or_else(|| draft.map(RelayEnrollmentDraft::public_endpoint))
            })
            .flatten();
        let mut payload = pairing_payload_with_relay_authorization(
            hosts,
            bound.port(),
            fingerprint,
            secret,
            name,
            relay,
            draft.map(RelayEnrollmentDraft::authorization_digest),
        );
        if self.startup.network_stack == RemoteNetworkStack::Iroh {
            let node = crate::iroh_tunnel::node_id_of(&self.startup.iroh_secret)
                .ok_or_else(|| "the Iroh identity is unavailable".to_string())?;
            payload.push_str("&m=iroh&i=");
            payload.push_str(&percent_encode(&node));
            if let Some(url) = self.startup.iroh_relay_url.as_deref() {
                payload.push_str("&j=");
                payload.push_str(&percent_encode(url));
            }
        } else {
            payload.push_str("&m=aiterm");
        }
        Ok(payload)
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
    let token = token
        .filter(|value| !value.is_empty())
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
pub async fn remote_relay_server_set(
    state: tauri::State<'_, RemoteState>,
    server: String,
) -> Result<RemoteStatusView, String> {
    {
        let mut inner = state.inner.lock().await;
        inner.load_relay_config()?;
        if inner.gateway.is_some() || inner.starting {
            return Err("turn remote access off before changing the relay server".into());
        }
        if inner.relay_config.is_some() {
            return Err("remove the current relay route before changing its server".into());
        }
        inner.starting = true;
    }
    let discovered = RelayServerConfig::discover(server.trim()).await;
    let mut inner = state.inner.lock().await;
    inner.starting = false;
    let selected = discovered?;
    selected.save(&state_root()?)?;
    inner.relay_server = Some(selected);
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
        if inner.startup.enabled && inner.startup.network_stack == RemoteNetworkStack::Aiterm {
            inner.startup.enabled = false;
            inner.startup.save(&state_root()?)?;
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
pub async fn remote_start_on_launch_set(
    state: tauri::State<'_, RemoteState>,
    enabled: bool,
    address: String,
    port: u16,
) -> Result<RemoteStatusView, String> {
    let mut inner = state.inner.lock().await;
    inner.load_relay_config()?;
    if enabled
        && inner.startup.network_stack == RemoteNetworkStack::Aiterm
        && inner.relay_config.is_none()
    {
        return Err("set up a relay before enabling automatic startup".into());
    }
    let config = RemoteStartupConfig {
        enabled,
        address,
        port,
        network_stack: inner.startup.network_stack,
        iroh_secret: inner.startup.iroh_secret.clone(),
        iroh_relay_url: inner.startup.iroh_relay_url.clone(),
    }
    .validated()?;
    config.save(&state_root()?)?;
    inner.startup = config;
    inner.startup_loaded = true;
    Ok(inner.status())
}

#[tauri::command]
pub async fn remote_network_stack_set(
    state: tauri::State<'_, RemoteState>,
    network_stack: RemoteNetworkStack,
) -> Result<RemoteStatusView, String> {
    let mut inner = state.inner.lock().await;
    inner.load_relay_config()?;
    if inner.gateway.is_some() || inner.starting {
        return Err("turn remote access off before changing the network stack".into());
    }
    inner.startup.network_stack = network_stack;
    if network_stack == RemoteNetworkStack::Iroh
        && crate::iroh_tunnel::secret_from_hex(&inner.startup.iroh_secret).is_none()
    {
        inner.startup.iroh_secret = crate::iroh_tunnel::new_secret_hex();
    }
    inner.startup.save(&state_root()?)?;
    Ok(inner.status())
}

#[tauri::command]
pub async fn remote_iroh_relay_url_set(
    state: tauri::State<'_, RemoteState>,
    url: Option<String>,
) -> Result<RemoteStatusView, String> {
    let mut inner = state.inner.lock().await;
    inner.load_relay_config()?;
    if inner.gateway.is_some() || inner.starting {
        return Err("turn remote access off before changing the Iroh relay".into());
    }
    let url = url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(value) = url.as_deref() {
        value
            .parse::<iroh::RelayUrl>()
            .map_err(|error| format!("invalid Iroh relay URL: {error}"))?;
    }
    inner.startup.iroh_relay_url = url;
    inner.startup.save(&state_root()?)?;
    Ok(inner.status())
}

#[tauri::command]
pub async fn remote_interfaces(
    _state: tauri::State<'_, RemoteState>,
) -> Result<Vec<String>, String> {
    Ok(shareable_addresses(local_addresses()))
}

async fn start_remote(
    app: tauri::AppHandle,
    state: &RemoteState,
    tabs: Arc<crate::tabs::TabRegistry>,
    services: crate::services::ApplicationServices,
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
        if inner.startup.network_stack == RemoteNetworkStack::Iroh
            && crate::iroh_tunnel::secret_from_hex(&inner.startup.iroh_secret).is_none()
        {
            inner.startup.iroh_secret = crate::iroh_tunnel::new_secret_hex();
            if let Err(error) = inner.startup.save(&state_root()?) {
                return Err(error);
            }
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
    let (relay_config, relay_server, network_stack, iroh_secret, iroh_relay_url) = {
        let inner = state.inner.lock().await;
        (
            inner.relay_config.clone(),
            inner.relay_server.clone(),
            inner.startup.network_stack,
            inner.startup.iroh_secret.clone(),
            inner.startup.iroh_relay_url.clone(),
        )
    };
    let advertised_hosts = match advertised_hosts(ip, local_addresses()) {
        Ok(hosts) => hosts,
        Err(error) => {
            state.inner.lock().await.starting = false;
            return Err(error);
        }
    };
    let identity = match state_root().and_then(|root| {
        let relay_dns = relay_certificate_dns(relay_config.as_ref(), relay_server.as_ref());
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
    let listen_ip = match network_stack {
        RemoteNetworkStack::Iroh => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        RemoteNetworkStack::Aiterm => match ip {
            IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        },
    };
    let relay_host = (network_stack == RemoteNetworkStack::Aiterm)
        .then(|| {
            relay_config
                .as_ref()
                .map(|config| config.public_host.clone())
        })
        .flatten();
    let relay_port = (network_stack == RemoteNetworkStack::Aiterm)
        .then(|| relay_config.as_ref().map(|config| config.public_port))
        .flatten();
    let routes = advertised_hosts.iter().map(ToString::to_string).collect();
    let mut remote_services = RemoteServices::from_application_services(tabs, &services)
        .with_app_handle(app)
        .with_gateway_routes(routes, port, relay_host, relay_port);
    if network_stack == RemoteNetworkStack::Aiterm {
        if let Some(config) = relay_config.as_ref() {
            remote_services =
                remote_services.with_direct_tunnel(Arc::new(DirectTunnelService::new(
                    config.clone(),
                    identity.clone(),
                    SocketAddr::new(ip, port),
                )));
        }
    }
    let started = RemoteGateway::start(
        SocketAddr::new(listen_ip, port),
        devices,
        identity,
        remote_services,
    )
    .await;
    let gateway = match started {
        Ok(gateway) => gateway,
        Err(error) => {
            state.inner.lock().await.starting = false;
            return Err(error.to_string());
        }
    };
    let gateway_port = gateway.local_addr().port();
    // `bound` is the advertised/connector address. In Iroh mode the actual
    // socket is loopback-only, but the selected address remains in the
    // certificate and pairing record for a future switch back to AITerm.
    let local_target = SocketAddr::new(ip, gateway_port);
    let iroh = if network_stack == RemoteNetworkStack::Iroh {
        let secret = match crate::iroh_tunnel::secret_from_hex(&iroh_secret) {
            Some(secret) => secret,
            None => {
                let _ = gateway.stop().await;
                state.inner.lock().await.starting = false;
                return Err("the Iroh identity is invalid".into());
            }
        };
        match crate::iroh_tunnel::start(secret, gateway_port, iroh_relay_url).await {
            Ok(tunnel) => Some(tunnel),
            Err(error) => {
                let _ = gateway.stop().await;
                state.inner.lock().await.starting = false;
                return Err(error);
            }
        }
    } else {
        None
    };
    let mut inner = state.inner.lock().await;
    inner.starting = false;
    inner.bound = Some(local_target);
    inner.fingerprint = Some(gateway.spki_fingerprint().to_string());
    inner.advertised_hosts = Some(advertised_hosts);
    if network_stack == RemoteNetworkStack::Aiterm {
        if let Some(config) = relay_config {
            inner.relay = Some(RelayConnectorHandle::start(config, local_target));
        }
    }
    inner.iroh = iroh;
    inner.gateway = Some(gateway);
    Ok(inner.status())
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
    start_remote(
        app,
        state.inner(),
        tabs.inner().clone(),
        services.inner().clone(),
        address,
        port,
    )
    .await
}

/// Restore remote access only after an explicit opt-in. Failure is diagnostic,
/// never fatal to the desktop: a missing interface or an offline relay should
/// not stop AITerm itself from opening.
pub fn start_on_launch(
    app: tauri::AppHandle,
    tabs: Arc<crate::tabs::TabRegistry>,
    services: crate::services::ApplicationServices,
) {
    let root = match state_root() {
        Ok(root) => root,
        Err(error) => {
            crate::diag!("remote", "could not read launch setting: {error}");
            return;
        }
    };
    let config = match RemoteStartupConfig::load(&root) {
        Ok(config) if config.enabled => config,
        Ok(_) => return,
        Err(error) => {
            crate::diag!("remote", "could not read launch setting: {error}");
            return;
        }
    };
    if config.network_stack == RemoteNetworkStack::Aiterm {
        match RelayConfig::load(&root) {
            Ok(Some(_)) => {}
            Ok(None) => {
                crate::diag!(
                    "remote",
                    "automatic startup skipped: no relay route is configured"
                );
                return;
            }
            Err(error) => {
                crate::diag!("remote", "automatic startup skipped: {error}");
                return;
            }
        }
    }
    let (address, port) = match startup_listener(&config, local_addresses()) {
        Ok(listener) => listener,
        Err(error) => {
            crate::diag!("remote", "automatic startup skipped: {error}");
            return;
        }
    };
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;
        let state = app.state::<RemoteState>();
        match start_remote(app.clone(), state.inner(), tabs, services, address, port).await {
            Ok(status) => crate::diag!(
                "remote",
                "started automatically on {}:{}; relay={:?}",
                status.address.as_deref().unwrap_or("?"),
                status.port.unwrap_or_default(),
                status.relay.state,
            ),
            Err(error) => crate::diag!("remote", "automatic startup failed: {error}"),
        }
    });
}

#[tauri::command]
pub async fn remote_stop(state: tauri::State<'_, RemoteState>) -> Result<RemoteStatusView, String> {
    let (gateway, relay, iroh) = {
        let mut inner = state.inner.lock().await;
        if inner.starting {
            return Err("remote access is still starting".to_string());
        }
        inner.bound = None;
        inner.fingerprint = None;
        inner.advertised_hosts = None;
        (inner.gateway.take(), inner.relay.take(), inner.iroh.take())
    };
    // Closing the listener is not a statement about any phone: trusted
    // devices stay trusted, and revocation stays an explicit act.
    if let Some(relay) = relay {
        relay.stop().await;
    }
    if let Some(iroh) = iroh {
        crate::iroh_tunnel::stop(iroh).await;
    }
    if let Some(gateway) = gateway {
        gateway.stop().await.map_err(|error| error.to_string())?;
    }
    Ok(state.inner.lock().await.status())
}

#[tauri::command]
pub async fn remote_begin_pairing(
    state: tauri::State<'_, RemoteState>,
) -> Result<PairingInviteView, String> {
    let (devices, fingerprint, needs_relay, relay_server) = {
        let mut inner = state.inner.lock().await;
        inner.load_relay_config()?;
        if inner.gateway.is_none() {
            return Err("turn remote access on before pairing a phone".into());
        }
        (
            inner.devices()?,
            inner
                .fingerprint
                .clone()
                .ok_or("the remote identity is unavailable")?,
            inner.startup.network_stack == RemoteNetworkStack::Aiterm
                && inner.relay_config.is_none(),
            inner
                .relay_server
                .as_ref()
                .map(|value| value.control_origin.clone())
                .unwrap_or_else(|| DEFAULT_RELAY_SERVER.to_string()),
        )
    };
    let draft = if needs_relay {
        match RelayConfig::prepare_enrollment(&relay_server, &fingerprint).await {
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
            inner.startup.network_stack == RemoteNetworkStack::Aiterm
                && inner.relay_config.is_none()
        };
        if should_register {
            match enrollment
                .draft
                .register(&enrollment.authority_public_key, &enrollment.signature_der)
                .await
            {
                Ok(config) => {
                    config.save(&state_root()?)?;
                    let mut inner = state.inner.lock().await;
                    if inner.relay_config.is_none() {
                        if let Some(gateway) = &inner.gateway {
                            gateway.set_relay_route(config.public_host.clone(), config.public_port);
                        }
                        if inner.startup.network_stack == RemoteNetworkStack::Aiterm {
                            if let Some(local_target) = inner.bound {
                                inner.relay =
                                    Some(RelayConnectorHandle::start(config.clone(), local_target));
                            }
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

    fn temporary_remote_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("aiterm-remote-startup-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn relay_startup_is_opt_in_and_round_trips_its_listener() {
        let root = temporary_remote_root();
        assert_eq!(
            RemoteStartupConfig::load(&root).unwrap(),
            RemoteStartupConfig::default()
        );

        let selected = RemoteStartupConfig {
            enabled: true,
            address: "10.0.0.151".into(),
            port: 9443,
            ..Default::default()
        };
        selected.save(&root).unwrap();
        assert_eq!(RemoteStartupConfig::load(&root).unwrap(), selected);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn iroh_identity_is_absent_until_that_stack_is_selected() {
        let config = RemoteStartupConfig::default();
        assert_eq!(config.network_stack, RemoteNetworkStack::Aiterm);
        assert!(config.iroh_secret.is_empty());
        assert!(crate::iroh_tunnel::node_id_of(&config.iroh_secret).is_none());

        let not_started_iroh = RemoteStartupConfig {
            network_stack: RemoteNetworkStack::Iroh,
            ..Default::default()
        };
        assert!(not_started_iroh.validated().unwrap().iroh_secret.is_empty());
    }

    #[test]
    fn pairing_invite_advertises_only_the_selected_network_stack() {
        let selected: IpAddr = "10.0.0.151".parse().unwrap();
        let mut inner = Inner {
            bound: Some(SocketAddr::new(selected, 8443)),
            fingerprint: Some("stable-spki-pin".to_string()),
            advertised_hosts: Some(vec![selected]),
            relay_config: Some(RelayConfig {
                connector_url: "wss://control.example.com/v1/connect".into(),
                public_host: "desktop.relay.example.com".into(),
                public_port: 443,
                route_id: "desktop".into(),
                token: "x".repeat(43),
                managed: true,
            }),
            ..Inner::default()
        };
        inner.startup.network_stack = RemoteNetworkStack::Aiterm;
        let native = inner.pairing_payload(b"secret", "desktop").unwrap();
        assert!(native.contains("&m=aiterm"));
        assert!(native.contains("&r=desktop.relay.example.com"));
        assert!(!native.contains("&i="));

        inner.startup.network_stack = RemoteNetworkStack::Iroh;
        inner.startup.iroh_secret = crate::iroh_tunnel::new_secret_hex();
        inner.startup.iroh_relay_url = Some("https://relay.example.com".into());
        let iroh = inner.pairing_payload(b"secret", "desktop").unwrap();
        assert!(iroh.contains("&m=iroh"));
        assert!(iroh.contains("&i="));
        assert!(iroh.contains("&j=https%3A%2F%2Frelay.example.com"));
        assert!(!iroh.contains("&r="));
    }

    #[test]
    fn relay_startup_prefers_the_saved_address_then_survives_network_changes() {
        let config = RemoteStartupConfig {
            enabled: true,
            address: "10.8.0.2".into(),
            port: 8443,
            ..Default::default()
        };
        assert_eq!(
            startup_listener(
                &config,
                vec!["192.168.1.20".parse().unwrap(), "10.8.0.2".parse().unwrap()],
            )
            .unwrap(),
            ("10.8.0.2".into(), 8443),
        );
        assert_eq!(
            startup_listener(&config, vec!["192.168.1.21".parse().unwrap()]).unwrap(),
            ("192.168.1.21".into(), 8443),
            "a missing VPN or a changed DHCP lease must not strand the relay",
        );
    }

    #[test]
    fn relay_startup_rejects_unsafe_listener_settings() {
        assert!(RemoteStartupConfig {
            enabled: true,
            address: "127.0.0.1".into(),
            port: 8443,
            ..Default::default()
        }
        .validated()
        .is_err());
        assert!(RemoteStartupConfig {
            enabled: true,
            address: "10.0.0.151".into(),
            port: 443,
            ..Default::default()
        }
        .validated()
        .is_err());
    }

    #[test]
    fn selected_relay_server_domain_is_covered_before_a_route_exists() {
        let server = RelayServerConfig::known(
            "https://control.custom.example.com",
            "relay.custom.example.com",
        )
        .unwrap();
        assert_eq!(
            relay_certificate_dns(None, Some(&server)),
            vec!["*.relay.custom.example.com"]
        );

        let route = RelayConfig {
            connector_url: "wss://control.custom.example.com/v1/connect".into(),
            public_host: "desktop-1234.relay.custom.example.com".into(),
            public_port: 443,
            route_id: "desktop-1234".into(),
            token: "x".repeat(43),
            managed: true,
        };
        assert_eq!(
            relay_certificate_dns(Some(&route), Some(&server)),
            vec!["desktop-1234.relay.custom.example.com"]
        );
    }

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
        let later_scan =
            advertised_hosts(selected, vec![selected, "172.16.40.7".parse().unwrap()]).unwrap();
        assert_eq!(later_scan, vec![selected, "172.16.40.7".parse().unwrap()]);

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
        let hosts = vec![
            "192.168.1.20".parse().unwrap(),
            "100.90.1.2".parse().unwrap(),
        ];
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
        assert_eq!(
            parsed.relay_host.as_deref(),
            Some("desk-1234.relay.example.com")
        );
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
        assert_eq!(
            parsed.relay_authorization_digest.as_deref(),
            Some(digest.as_slice())
        );
    }
}
