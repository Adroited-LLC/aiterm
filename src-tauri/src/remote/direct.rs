use super::relay::RelayConfig;
use super::server::TlsIdentity;
use aiterm_relay_protocol::{
    DirectCookie, DirectId, DirectPacket, DIRECT_COOKIE_BYTES, DIRECT_ID_BYTES,
    MAX_DIRECT_PACKET_BYTES,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use quinn::{Endpoint, EndpointConfig, ServerConfig, TokioRuntime, TransportConfig, VarInt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Semaphore;

const PREPARE_TIMEOUT: Duration = Duration::from_secs(5);
const RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(12);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ATTEMPTS: usize = 8;
const ALPN: &[u8] = b"aiterm-direct-tunnel/2";

#[derive(Clone, Debug, Serialize)]
pub struct DirectOffer {
    pub id: String,
    pub cookie: String,
    pub host: String,
    pub port: u16,
    pub expires_in_millis: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayDirectOffer {
    id: String,
    desktop_cookie: String,
    phone_cookie: String,
    host: String,
    port: u16,
    expires_in_millis: u64,
}

pub struct DirectTunnelService {
    relay: RelayConfig,
    identity: TlsIdentity,
    local_target: SocketAddr,
    attempts: Arc<Semaphore>,
}

impl DirectTunnelService {
    pub fn new(relay: RelayConfig, identity: TlsIdentity, local_target: SocketAddr) -> Self {
        Self {
            relay,
            identity,
            local_target,
            attempts: Arc::new(Semaphore::new(MAX_ATTEMPTS)),
        }
    }

    pub async fn prepare(&self) -> Result<DirectOffer, String> {
        let permit = self
            .attempts
            .clone()
            .try_acquire_owned()
            .map_err(|_| "too many direct connection attempts are active".to_string())?;
        let relay_offer = self.request_offer().await?;
        let id = decode_fixed::<DIRECT_ID_BYTES>(&relay_offer.id, "rendezvous id")?;
        let desktop_cookie = decode_fixed::<DIRECT_COOKIE_BYTES>(
            &relay_offer.desktop_cookie,
            "desktop rendezvous cookie",
        )?;
        let phone_cookie: DirectCookie = decode_fixed::<DIRECT_COOKIE_BYTES>(
            &relay_offer.phone_cookie,
            "phone rendezvous cookie",
        )?;
        if relay_offer.port == 0 || relay_offer.expires_in_millis == 0 {
            return Err("the relay returned an invalid direct endpoint".into());
        }
        let relay_address = resolve_udp(&relay_offer.host, relay_offer.port).await?;
        let bind_address = match relay_address {
            SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        };
        let socket = UdpSocket::bind(bind_address)
            .await
            .map_err(|_| "could not open a direct UDP socket".to_string())?;
        bind_desktop(&socket, relay_address, id, desktop_cookie).await?;

        let identity = self.identity.clone();
        let local_target = self.local_target;
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = run_attempt(
                socket,
                relay_address,
                id,
                phone_cookie,
                identity,
                local_target,
            )
            .await
            {
                crate::diag!("remote", "direct tunnel attempt ended: {error}");
            }
        });
        Ok(DirectOffer {
            id: relay_offer.id,
            cookie: relay_offer.phone_cookie,
            host: relay_offer.host,
            port: relay_offer.port,
            expires_in_millis: relay_offer.expires_in_millis,
        })
    }

    async fn request_offer(&self) -> Result<RelayDirectOffer, String> {
        let mut url = url::Url::parse(&self.relay.connector_url)
            .map_err(|_| "stored relay connector URL is invalid".to_string())?;
        url.set_scheme(if url.scheme() == "wss" {
            "https"
        } else {
            "http"
        })
        .map_err(|_| "stored relay connector URL is invalid".to_string())?;
        url.set_path(&format!("/v1/direct/{}", self.relay.route_id));
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(PREPARE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "could not initialize direct connection setup".to_string())?;
        let response = client
            .post(url)
            .bearer_auth(&self.relay.token)
            .send()
            .await
            .map_err(|_| "the relay could not prepare a direct connection".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "the relay rejected direct connection setup with status {}",
                response.status().as_u16()
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "the relay returned an invalid direct response".to_string())?;
        if bytes.len() > 4096 {
            return Err("the relay returned an oversized direct response".into());
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| "the relay returned an invalid direct response".to_string())
    }
}

fn decode_fixed<const N: usize>(value: &str, label: &str) -> Result<[u8; N], String> {
    URL_SAFE_NO_PAD
        .decode(value.as_bytes())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| format!("the relay returned an invalid {label}"))
}

async fn resolve_udp(host: &str, port: u16) -> Result<SocketAddr, String> {
    tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "the relay direct endpoint could not be resolved".to_string())?
        .next()
        .ok_or_else(|| "the relay direct endpoint could not be resolved".to_string())
}

async fn bind_desktop(
    socket: &UdpSocket,
    relay: SocketAddr,
    id: DirectId,
    cookie: DirectCookie,
) -> Result<(), String> {
    let binding = DirectPacket::BindDesktop { id, cookie }.encode();
    let deadline = tokio::time::Instant::now() + PREPARE_TIMEOUT;
    let mut bytes = [0u8; MAX_DIRECT_PACKET_BYTES];
    loop {
        socket
            .send_to(&binding, relay)
            .await
            .map_err(|_| "the relay direct endpoint could not be reached".to_string())?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("the relay direct endpoint did not answer".into());
        }
        match tokio::time::timeout(
            remaining.min(Duration::from_millis(500)),
            socket.recv_from(&mut bytes),
        )
        .await
        {
            Ok(Ok((count, source))) if source == relay => {
                if DirectPacket::decode(&bytes[..count]).ok() == Some(DirectPacket::Bound { id }) {
                    return Ok(());
                }
            }
            Ok(Ok(_)) | Err(_) => continue,
            Ok(Err(_)) => return Err("the direct UDP socket failed".into()),
        }
    }
}

async fn run_attempt(
    socket: UdpSocket,
    relay: SocketAddr,
    id: DirectId,
    phone_cookie: DirectCookie,
    identity: TlsIdentity,
    local_target: SocketAddr,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + RENDEZVOUS_TIMEOUT;
    let mut bytes = [0u8; MAX_DIRECT_PACKET_BYTES];
    let peer = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("phone did not join the direct rendezvous".into());
        }
        match tokio::time::timeout(remaining, socket.recv_from(&mut bytes)).await {
            Ok(Ok((count, source))) if source == relay => {
                if let Ok(DirectPacket::Peer {
                    id: packet_id,
                    address,
                }) = DirectPacket::decode(&bytes[..count])
                {
                    if packet_id == id {
                        break address;
                    }
                }
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => return Err("the direct UDP socket failed".into()),
            Err(_) => return Err("phone did not join the direct rendezvous".into()),
        }
    };
    let probe = DirectPacket::Probe { id }.encode();
    for _ in 0..3 {
        let _ = socket.send_to(&probe, peer).await;
    }
    let std_socket = socket
        .into_std()
        .map_err(|_| "could not activate the direct UDP socket".to_string())?;
    let mut server_config = direct_server_config(&identity)?;
    let mut transport = TransportConfig::default();
    transport.max_concurrent_bidi_streams(VarInt::from_u32(1));
    transport.max_concurrent_uni_streams(VarInt::from_u32(0));
    transport.keep_alive_interval(Some(Duration::from_secs(10)));
    // Keep the internet-safe 1200-byte baseline across cellular and VPN paths.
    transport.mtu_discovery_config(None);
    server_config.transport_config(Arc::new(transport));
    let endpoint = Endpoint::new(
        EndpointConfig::default(),
        Some(server_config),
        std_socket,
        Arc::new(TokioRuntime),
    )
    .map_err(|_| "could not start the direct QUIC endpoint".to_string())?;
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    let (connection, mut to_phone, mut from_phone) = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("direct QUIC connection timed out".into());
        }
        let incoming = tokio::time::timeout(remaining, endpoint.accept())
            .await
            .map_err(|_| "direct QUIC connection timed out".to_string())?
            .ok_or_else(|| "direct QUIC endpoint stopped".to_string())?;
        let connection = match incoming.await {
            Ok(connection) => connection,
            Err(_) => continue,
        };
        let address = connection.remote_address();
        if address == relay {
            connection.close(VarInt::from_u32(1), b"relay path is not direct");
            continue;
        }
        let stream_remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let stream_timeout = stream_remaining.min(STREAM_TIMEOUT);
        let Ok(Ok((to_phone, mut from_phone))) =
            tokio::time::timeout(stream_timeout, connection.accept_bi()).await
        else {
            connection.close(VarInt::from_u32(1), b"missing tunnel stream");
            continue;
        };
        let mut presented_cookie = [0u8; DIRECT_COOKIE_BYTES];
        let cookie_ok =
            tokio::time::timeout(stream_timeout, from_phone.read_exact(&mut presented_cookie))
                .await
                .ok()
                .and_then(Result::ok)
                .is_some()
                && bool::from(presented_cookie.ct_eq(&phone_cookie));
        presented_cookie.fill(0);
        if !cookie_ok {
            connection.close(VarInt::from_u32(1), b"invalid rendezvous proof");
            continue;
        }
        break (connection, to_phone, from_phone);
    };
    crate::diag!("remote", "QUIC tunnel established over direct path");
    let local = TcpStream::connect(local_target)
        .await
        .map_err(|_| "the local remote gateway was unavailable".to_string())?;
    let (mut local_read, mut local_write) = local.into_split();
    let phone_to_desktop = tokio::io::copy(&mut from_phone, &mut local_write);
    let desktop_to_phone = tokio::io::copy(&mut local_read, &mut to_phone);
    let _ = tokio::try_join!(phone_to_desktop, desktop_to_phone);
    let _ = to_phone.finish();
    connection.close(VarInt::from_u32(0), b"tunnel closed");
    endpoint.wait_idle().await;
    Ok(())
}

fn direct_server_config(identity: &TlsIdentity) -> Result<ServerConfig, String> {
    let mut crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(identity.certificate_der().to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                identity.private_key_der().to_vec(),
            )),
        )
        .map_err(|_| "the desktop identity could not secure direct QUIC".to_string())?;
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let quic = quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
        .map_err(|_| "the desktop identity is not valid for QUIC".to_string())?;
    Ok(ServerConfig::with_crypto(Arc::new(quic)))
}
