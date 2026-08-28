use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

const STORE_VERSION: u8 = 1;
const ENROLLMENT_LIFETIME: Duration = Duration::from_secs(300);
const DEVICES_FILE: &str = "trusted-devices.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    pub created_at: u64,
    pub last_seen_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredDevice {
    #[serde(flatten)]
    view: TrustedDevice,
    public_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedStore {
    version: u8,
    devices: Vec<StoredDevice>,
}

struct PendingEnrollment {
    secret: [u8; 32],
    expires_at: SystemTime,
}

#[derive(Default)]
struct StoreState {
    devices: Vec<StoredDevice>,
    pending: Vec<PendingEnrollment>,
}

pub struct DeviceStore {
    root: PathBuf,
    state: Mutex<StoreState>,
}

#[derive(Clone, Debug)]
pub struct EnrollmentQr {
    secret: [u8; 32],
}

impl EnrollmentQr {
    pub fn secret(&self) -> &[u8] {
        &self.secret
    }
}

impl DeviceStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, AuthError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root).map_err(AuthError::storage)?;
        set_private_permissions(&root, 0o700).map_err(AuthError::storage)?;
        let devices = load_devices(&root.join(DEVICES_FILE))?;
        Ok(Self {
            root,
            state: Mutex::new(StoreState {
                devices,
                pending: Vec::new(),
            }),
        })
    }

    pub fn begin_enrollment_at(&self, now: SystemTime) -> Result<EnrollmentQr, AuthError> {
        let mut secret = [0u8; 32];
        OsRng.fill_bytes(&mut secret);
        let expires_at = now
            .checked_add(ENROLLMENT_LIFETIME)
            .ok_or_else(|| AuthError::new("pairing.invalid_time", "pairing time overflow"))?;
        let mut state = self.state.lock().map_err(|_| AuthError::poisoned())?;
        state.pending.retain(|item| item.expires_at >= now);
        state.pending.push(PendingEnrollment { secret, expires_at });
        Ok(EnrollmentQr { secret })
    }

    pub fn approve_at(
        &self,
        secret: &[u8],
        device_name: &str,
        public_key: &[u8],
        now: SystemTime,
    ) -> Result<TrustedDevice, AuthError> {
        let mut state = self.state.lock().map_err(|_| AuthError::poisoned())?;
        let Some(position) = state
            .pending
            .iter()
            .position(|item| bool::from(item.secret.as_slice().ct_eq(secret)))
        else {
            return Err(AuthError::new(
                "pairing.invalid_or_consumed",
                "pairing secret is invalid or already used",
            ));
        };
        // Consume before validation: a failed or denied request never gets a
        // second attempt with the same QR secret.
        let pending = state.pending.remove(position);
        if now > pending.expires_at {
            return Err(AuthError::new(
                "pairing.expired",
                "pairing secret has expired",
            ));
        }

        let name = device_name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(AuthError::new(
                "pairing.invalid_device_name",
                "device name must contain between 1 and 64 characters",
            ));
        }
        let verifying_key = VerifyingKey::from_sec1_bytes(public_key).map_err(|_| {
            AuthError::new("pairing.invalid_public_key", "invalid P-256 public key")
        })?;
        let canonical_key = verifying_key.to_encoded_point(true);
        let canonical_bytes = canonical_key.as_bytes();
        let fingerprint = URL_SAFE_NO_PAD.encode(Sha256::digest(canonical_bytes));
        let view = TrustedDevice {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            fingerprint,
            created_at: unix_seconds(now),
            last_seen_at: None,
        };
        let stored = StoredDevice {
            view: view.clone(),
            public_key: URL_SAFE_NO_PAD.encode(canonical_bytes),
        };
        let mut devices = state.devices.clone();
        devices.push(stored);
        persist_devices(&self.root.join(DEVICES_FILE), &devices)?;
        state.devices = devices;
        Ok(view)
    }

    pub fn verify_proof(
        &self,
        device_id: &str,
        nonce: &[u8],
        signature_der: &[u8],
    ) -> Result<(), AuthError> {
        let state = self.state.lock().map_err(|_| AuthError::poisoned())?;
        let device = state
            .devices
            .iter()
            .find(|device| device.view.id == device_id)
            .ok_or_else(|| AuthError::new("auth.unknown_device", "device is not trusted"))?;
        let public_key = URL_SAFE_NO_PAD
            .decode(&device.public_key)
            .map_err(|_| AuthError::new("auth.invalid_device_record", "invalid stored key"))?;
        let verifying_key = VerifyingKey::from_sec1_bytes(&public_key)
            .map_err(|_| AuthError::new("auth.invalid_device_record", "invalid stored key"))?;
        let signature = Signature::from_der(signature_der)
            .map_err(|_| AuthError::new("auth.invalid_proof", "invalid device proof"))?;
        verifying_key
            .verify(nonce, &signature)
            .map_err(|_| AuthError::new("auth.invalid_proof", "invalid device proof"))
    }

    pub fn list_devices(&self) -> Vec<TrustedDevice> {
        self.state
            .lock()
            .map(|state| {
                state
                    .devices
                    .iter()
                    .map(|device| device.view.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn revoke(&self, device_id: &str) -> Result<bool, AuthError> {
        let mut state = self.state.lock().map_err(|_| AuthError::poisoned())?;
        let mut devices = state.devices.clone();
        let original_len = devices.len();
        devices.retain(|device| device.view.id != device_id);
        if devices.len() == original_len {
            return Ok(false);
        }
        persist_devices(&self.root.join(DEVICES_FILE), &devices)?;
        state.devices = devices;
        Ok(true)
    }
}

fn load_devices(path: &Path) -> Result<Vec<StoredDevice>, AuthError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(AuthError::storage(error)),
    };
    let persisted: PersistedStore = serde_json::from_str(&text)
        .map_err(|error| AuthError::new("auth.invalid_store", error.to_string()))?;
    if persisted.version != STORE_VERSION {
        return Err(AuthError::new(
            "auth.unsupported_store_version",
            "unsupported trusted-device store version",
        ));
    }
    for device in &persisted.devices {
        let bytes = URL_SAFE_NO_PAD
            .decode(&device.public_key)
            .map_err(|_| AuthError::new("auth.invalid_store", "invalid stored public key"))?;
        VerifyingKey::from_sec1_bytes(&bytes)
            .map_err(|_| AuthError::new("auth.invalid_store", "invalid stored public key"))?;
    }
    Ok(persisted.devices)
}

fn persist_devices(path: &Path, devices: &[StoredDevice]) -> Result<(), AuthError> {
    let persisted = PersistedStore {
        version: STORE_VERSION,
        devices: devices.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&persisted)
        .map_err(|error| AuthError::new("auth.store_failed", error.to_string()))?;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    write_private_file(&temp, &bytes).map_err(AuthError::storage)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        std::fs::remove_file(&temp).ok();
        return Err(AuthError::storage(error));
    }
    Ok(())
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
fn set_private_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthError {
    code: &'static str,
    message: String,
}

impl AuthError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn storage(error: std::io::Error) -> Self {
        Self::new("auth.store_failed", error.to_string())
    }

    fn poisoned() -> Self {
        Self::new("auth.store_unavailable", "trusted-device store lock failed")
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AuthError {}
