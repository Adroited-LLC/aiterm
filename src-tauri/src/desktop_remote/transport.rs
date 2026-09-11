//! The same pinned TLS + P-256 challenge protocol used by Android.
use super::store::Desktop;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    tungstenite::{protocol::WebSocketConfig, Message},
    Connector, MaybeTlsStream, WebSocketStream,
};

pub(super) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub(super) const LIMIT: usize = 1024 * 1024;

#[derive(Debug)]
struct PinnedKey([u8; 32]);
impl ServerCertVerifier for PinnedKey {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
            .map_err(|_| rustls::Error::General("Invalid device certificate".into()))?;
        let digest: [u8; 32] = Sha256::digest(parsed.public_key().raw).into();
        if digest != self.0 {
            return Err(rustls::Error::General(
                "Device certificate fingerprint changed".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signed,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signed,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub(super) fn endpoint(host: &str, port: u16) -> Result<String, String> {
    if host.is_empty()
        || host.len() > 253
        || port == 0
        || host
            .chars()
            .any(|c| c.is_whitespace() || "/\\@?#%".contains(c))
    {
        return Err("Invalid desktop address".into());
    }
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let url = format!("wss://{host}:{port}/v1/ws");
    url::Url::parse(&url).map_err(|_| "Invalid desktop address".to_string())?;
    Ok(url)
}

pub(super) async fn open(desktop: &Desktop) -> Result<Socket, String> {
    let pin: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&desktop.fingerprint)
        .map_err(|_| "Invalid device fingerprint")?
        .try_into()
        .map_err(|_| "Invalid device fingerprint length")?;
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(PinnedKey(pin)))
    .with_no_client_auth();
    let mut routes: Vec<_> = desktop
        .hosts
        .iter()
        .map(|h| (h.clone(), desktop.port))
        .collect();
    if let Some((host, port)) = &desktop.relay {
        if !routes.contains(&(host.clone(), *port)) {
            routes.push((host.clone(), *port));
        }
    }
    let mut last = "No saved desktop routes".to_string();
    for (host, port) in routes {
        let address = endpoint(&host, port)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(LIMIT))
            .max_frame_size(Some(LIMIT));
        match tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async_tls_with_config(
                address,
                Some(config),
                false,
                Some(Connector::Rustls(Arc::new(tls.clone()))),
            ),
        )
        .await
        {
            Ok(Ok((socket, _))) => return Ok(socket),
            Ok(Err(e)) => last = format!("Could not connect to the pinned desktop: {e}"),
            Err(_) => last = "Desktop connection timed out".into(),
        }
    }
    Err(last)
}

pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    crate::remote::model::encode_terminal_frame(value).map_err(|e| e.to_string())
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    if bytes.is_empty() || bytes.len() >= LIMIT {
        return Err("Invalid remote frame size".into());
    }
    crate::remote::model::decode_exact(bytes).map_err(|_| "Invalid remote frame".to_string())
}
pub(super) async fn send<T: Serialize>(socket: &mut Socket, value: &T) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.send(Message::Binary(encode(value)?.into())),
    )
    .await
    .map_err(|_| "Desktop write timed out")?
    .map_err(|e| e.to_string())
}
pub(super) async fn receive(socket: &mut Socket) -> Result<Vec<u8>, String> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Binary(bytes))) if bytes.len() < LIMIT => return Ok(bytes.to_vec()),
            Some(Ok(Message::Ping(_))) => socket.flush().await.map_err(|e| e.to_string())?,
            Some(Ok(Message::Pong(_))) => {}
            _ => return Err("Desktop connection closed".into()),
        }
    }
}
#[derive(Deserialize)]
pub(super) struct Hello {
    pub kind: String,
    #[serde(default, with = "serde_bytes")]
    pub nonce: Vec<u8>,
    #[serde(default)]
    pub device_id: String,
}
#[derive(Serialize)]
struct Proof<'a> {
    kind: &'static str,
    device_id: &'a str,
    #[serde(with = "serde_bytes")]
    signature_der: Vec<u8>,
}
pub(super) async fn challenge(socket: &mut Socket) -> Result<(), String> {
    let hello: Hello = decode(
        &tokio::time::timeout(Duration::from_secs(10), receive(socket))
            .await
            .map_err(|_| "Desktop authentication timed out")??,
    )?;
    if hello.kind != "auth.challenge" || hello.nonce.len() != 32 {
        return Err("Invalid desktop authentication challenge".into());
    }
    Ok(())
}
pub(super) async fn authenticate(socket: &mut Socket, desktop: &Desktop) -> Result<(), String> {
    let hello: Hello = decode(
        &tokio::time::timeout(Duration::from_secs(10), receive(socket))
            .await
            .map_err(|_| "Desktop authentication timed out")??,
    )?;
    if hello.kind != "auth.challenge" || hello.nonce.len() != 32 {
        return Err("Invalid desktop authentication challenge".into());
    }
    let key = SigningKey::from_slice(&desktop.key).map_err(|_| "Invalid saved device identity")?;
    let signature: Signature = key.sign(&hello.nonce);
    send(
        socket,
        &Proof {
            kind: "auth.proof",
            device_id: &desktop.device_id,
            signature_der: signature.to_der().as_bytes().to_vec(),
        },
    )
    .await?;
    let reply: Hello = decode(
        &tokio::time::timeout(Duration::from_secs(10), receive(socket))
            .await
            .map_err(|_| "Desktop authentication timed out")??,
    )?;
    if reply.kind != "auth.ok" {
        return Err("Device access denied. Pair with this desktop again.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_reject_credentials_paths_and_queries() {
        for h in [
            "a/b",
            "user@host",
            "host?x=1",
            "host#x",
            "host\\x",
            "host name",
        ] {
            assert!(endpoint(h, 8443).is_err());
        }
        assert_eq!(endpoint("::1", 8443).unwrap(), "wss://[::1]:8443/v1/ws");
    }
    #[test]
    fn pin_verifier_rejects_another_valid_certificate() {
        let a = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let b = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(a.cert.der()).unwrap();
        let pin = PinnedKey(Sha256::digest(cert.public_key().raw).into());
        let name = ServerName::try_from("localhost").unwrap();
        let now = UnixTime::now();
        assert!(pin
            .verify_server_cert(a.cert.der(), &[], &name, &[], now)
            .is_ok());
        assert!(pin
            .verify_server_cert(b.cert.der(), &[], &name, &[], now)
            .is_err());
    }
}
