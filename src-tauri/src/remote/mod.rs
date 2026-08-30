pub mod auth;
pub mod model;
pub mod server;
pub mod terminal;

use auth::{DeviceStore, PendingPairing, TrustedDevice};
use qrcode::{EcLevel, QrCode};
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
                // An unknown key means a payload written by a build that knows
                // something this one does not. Ignoring it is safe only
                // because every field that grants trust is required below.
                _ => {}
            }
        }

        if version? != PAIRING_VERSION || hosts.is_empty() {
            return None;
        }
        Some(Self {
            version: PAIRING_VERSION,
            hosts,
            port: port?,
            fingerprint: fingerprint?,
            secret: secret?,
            name,
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
    let mut payload = format!("aiterm://pair?v={PAIRING_VERSION}");
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

fn state_root() -> Result<std::path::PathBuf, String> {
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
    starting: bool,
}

#[derive(Default)]
pub struct RemoteState {
    inner: Mutex<Inner>,
}

impl Inner {
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
        }
    }

    fn pairing_payload(&self, secret: &[u8], name: &str) -> Result<String, String> {
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
        Ok(pairing_payload(
            hosts,
            bound.port(),
            fingerprint,
            secret,
            name,
        ))
    }
}

#[tauri::command]
pub async fn remote_status(
    state: tauri::State<'_, RemoteState>,
) -> Result<RemoteStatusView, String> {
    Ok(state.inner.lock().await.status())
}

#[tauri::command]
pub async fn remote_interfaces(
    _state: tauri::State<'_, RemoteState>,
) -> Result<Vec<String>, String> {
    Ok(shareable_addresses(local_addresses()))
}

#[tauri::command]
pub async fn remote_start(
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
    let devices = {
        let mut inner = state.inner.lock().await;
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
    let advertised_hosts = match advertised_hosts(ip, local_addresses()) {
        Ok(hosts) => hosts,
        Err(error) => {
            state.inner.lock().await.starting = false;
            return Err(error);
        }
    };
    let identity = match state_root().and_then(|root| {
        TlsIdentity::load_or_create(root.join("tls"), &advertised_hosts)
            .map_err(|e| e.to_string())
    }) {
        Ok(identity) => identity,
        Err(error) => {
            state.inner.lock().await.starting = false;
            return Err(error);
        }
    };
    let started = RemoteGateway::start(
        SocketAddr::new(ip, port),
        devices,
        identity,
        RemoteServices::from_application_services(tabs.inner().clone(), services.inner()),
    )
    .await;
    let mut inner = state.inner.lock().await;
    inner.starting = false;
    let gateway = started.map_err(|error| error.to_string())?;
    inner.bound = Some(gateway.local_addr());
    inner.fingerprint = Some(gateway.spki_fingerprint().to_string());
    inner.advertised_hosts = Some(advertised_hosts);
    inner.gateway = Some(gateway);
    Ok(inner.status())
}

#[tauri::command]
pub async fn remote_stop(state: tauri::State<'_, RemoteState>) -> Result<RemoteStatusView, String> {
    let gateway = {
        let mut inner = state.inner.lock().await;
        if inner.starting {
            return Err("remote access is still starting".to_string());
        }
        inner.bound = None;
        inner.fingerprint = None;
        inner.advertised_hosts = None;
        inner.gateway.take()
    };
    // Closing the listener is not a statement about any phone: trusted
    // devices stay trusted, and revocation stays an explicit act.
    if let Some(gateway) = gateway {
        gateway.stop().await.map_err(|error| error.to_string())?;
    }
    Ok(state.inner.lock().await.status())
}

#[tauri::command]
pub async fn remote_begin_pairing(
    state: tauri::State<'_, RemoteState>,
) -> Result<PairingInviteView, String> {
    let mut inner = state.inner.lock().await;
    if inner.gateway.is_none() {
        return Err("turn remote access on before pairing a phone".into());
    }
    let devices = inner.devices()?;
    let now = SystemTime::now();
    let enrollment = devices
        .begin_enrollment_at(now)
        .map_err(|error| error.to_string())?;
    let payload = inner.pairing_payload(enrollment.secret(), &desktop_name())?;
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
    devices
        .approve_pairing_at(&request_id, SystemTime::now())
        .map_err(|error| error.to_string())
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
}
