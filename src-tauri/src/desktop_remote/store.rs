use crate::remote::{
    auth::{set_private_permissions, write_private_file},
    PairingUri,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::SigningKey;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Desktop {
    pub id: String,
    pub name: String,
    pub hosts: Vec<String>,
    pub port: u16,
    pub fingerprint: String,
    pub relay: Option<(String, u16)>,
    pub device_id: String,
    pub key: Vec<u8>,
}
#[derive(Clone, Serialize)]
pub struct DesktopView {
    pub id: String,
    pub name: String,
    pub address: String,
    pub fingerprint: String,
}
impl Desktop {
    pub fn view(&self) -> DesktopView {
        DesktopView {
            id: self.id.clone(),
            name: self.name.clone(),
            address: self.hosts.first().cloned().unwrap_or_default(),
            fingerprint: self.fingerprint.clone(),
        }
    }
    pub fn from_invite(invite: &PairingUri) -> Result<Self, String> {
        if invite.secret.len() != 32
            || URL_SAFE_NO_PAD
                .decode(&invite.fingerprint)
                .map_err(|_| "Invalid device fingerprint")?
                .len()
                != 32
            || invite.hosts.len() > crate::remote::server::MAX_ADVERTISED_HOSTS
        {
            return Err("Invalid pairing file".into());
        }
        for host in &invite.hosts {
            super::transport::endpoint(host, invite.port)?;
        }
        let relay = invite.relay_host.clone().zip(invite.relay_port);
        if let Some((h, p)) = &relay {
            super::transport::endpoint(h, *p)?;
        }
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: invite.name.chars().take(128).collect(),
            hosts: invite.hosts.clone(),
            port: invite.port,
            fingerprint: invite.fingerprint.clone(),
            relay,
            device_id: String::new(),
            key: SigningKey::random(&mut OsRng).to_bytes().to_vec(),
        })
    }
}
pub(super) fn root() -> Result<PathBuf, String> {
    let p = dirs::data_dir()
        .ok_or("No app data directory")?
        .join("aiterm/desktop-connections");
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    set_private_permissions(&p, 0o700).map_err(|e| e.to_string())?;
    Ok(p)
}
pub(super) fn load() -> Result<Vec<Desktop>, String> {
    load_at(&root()?.join("desktops.json"))
}
fn load_at(path: &Path) -> Result<Vec<Desktop>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.to_string()),
    };
    if bytes.len() > 1024 * 1024 {
        return Err("Saved desktop list is too large".into());
    }
    let desktops: Vec<Desktop> =
        serde_json::from_slice(&bytes).map_err(|_| "Saved desktop connections are invalid")?;
    for d in &desktops {
        if d.device_id.is_empty() || SigningKey::from_slice(&d.key).is_err() {
            return Err("Saved desktop identity is invalid".into());
        }
    }
    Ok(desktops)
}
pub(super) fn save(desktops: &[Desktop]) -> Result<(), String> {
    save_at(&root()?.join("desktops.json"), desktops)
}
fn save_at(path: &Path, desktops: &[Desktop]) -> Result<(), String> {
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    write_private_file(
        &temp,
        &serde_json::to_vec(desktops).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(temp);
        return Err(e.to_string());
    }
    Ok(())
}
pub(super) fn relay_key() -> Result<SigningKey, String> {
    let path = root()?.join("relay-authority.key");
    match std::fs::read(&path) {
        Ok(b) => SigningKey::from_slice(&b).map_err(|_| "Invalid saved relay identity".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SigningKey::random(&mut OsRng);
            write_private_file(&path, &key.to_bytes()).map_err(|e| e.to_string())?;
            Ok(key)
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_round_trip_is_private_and_renderer_never_receives_key() {
        let root =
            std::env::temp_dir().join(format!("aiterm-client-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("desktops.json");
        let desktop = Desktop {
            id: "test".into(),
            name: "Desktop".into(),
            hosts: vec!["localhost".into()],
            port: 8443,
            fingerprint: URL_SAFE_NO_PAD.encode([7; 32]),
            relay: None,
            device_id: "approved".into(),
            key: SigningKey::random(&mut OsRng).to_bytes().to_vec(),
        };
        save_at(&path, &[desktop.clone()]).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded[0].key, desktop.key);
        assert_eq!(loaded[0].device_id, desktop.device_id);
        let public = serde_json::to_value(loaded[0].view()).unwrap();
        assert!(public.get("key").is_none());
        assert!(public.get("device_id").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        save_at(&path, &[]).unwrap();
        assert!(load_at(&path).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
