use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

const STORE_VERSION: u8 = 1;
const ENROLLMENT_LIFETIME: Duration = Duration::from_secs(300);
const DEVICES_FILE: &str = "trusted-devices.json";
/// How long a submitted pairing request, and the answer the desktop gave
/// it, stay in memory. A phone that walks away mid-pairing must not pin a
/// record for the life of the process.
const PAIRING_RETENTION: Duration = Duration::from_secs(300);

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PendingPairing {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    pub requested_at: u64,
}

struct PendingDevice {
    view: PendingPairing,
    public_key: String,
    expires_at: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingOutcome {
    Approved(TrustedDevice),
    Denied,
}

#[derive(Default)]
struct StoreState {
    devices: Vec<StoredDevice>,
    enrollments: Vec<PendingEnrollment>,
    pending_pairings: Vec<PendingDevice>,
    pairing_outcomes: HashMap<String, (PairingOutcome, SystemTime)>,
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
                enrollments: Vec::new(),
                pending_pairings: Vec::new(),
                pairing_outcomes: HashMap::new(),
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
        state.enrollments.retain(|item| item.expires_at >= now);
        state
            .enrollments
            .push(PendingEnrollment { secret, expires_at });
        Ok(EnrollmentQr { secret })
    }

    pub fn submit_pairing_at(
        &self,
        secret: &[u8],
        device_name: &str,
        public_key: &[u8],
        now: SystemTime,
    ) -> Result<PendingPairing, AuthError> {
        let mut state = self.state.lock().map_err(|_| AuthError::poisoned())?;
        prune_pairings(&mut state, now);
        let Some(position) = state
            .enrollments
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
        let pending = state.enrollments.remove(position);
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
        let view = PendingPairing {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            fingerprint,
            requested_at: unix_seconds(now),
        };
        state.pending_pairings.push(PendingDevice {
            view: view.clone(),
            public_key: URL_SAFE_NO_PAD.encode(canonical_bytes),
            expires_at: now + PAIRING_RETENTION,
        });
        Ok(view)
    }

    pub fn approve_pairing_at(
        &self,
        request_id: &str,
        now: SystemTime,
    ) -> Result<TrustedDevice, AuthError> {
        let mut state = self.state.lock().map_err(|_| AuthError::poisoned())?;
        let position = state
            .pending_pairings
            .iter()
            .position(|pairing| pairing.view.id == request_id)
            .ok_or_else(|| {
                AuthError::new("pairing.unknown_request", "pairing request does not exist")
            })?;
        let pending = state.pending_pairings.remove(position);
        let view = TrustedDevice {
            id: uuid::Uuid::new_v4().to_string(),
            name: pending.view.name,
            fingerprint: pending.view.fingerprint,
            created_at: unix_seconds(now),
            last_seen_at: None,
        };
        let stored = StoredDevice {
            view: view.clone(),
            public_key: pending.public_key,
        };
        let mut devices = state.devices.clone();
        // One phone, one row. Pairing a phone that is already trusted — after
        // a reinstall, or because the user simply scanned again — used to add
        // a second row holding the same key, so revoking the row the user
        // could see left a working credential behind.
        devices.retain(|device| device.view.fingerprint != stored.view.fingerprint);
        devices.push(stored);
        persist_devices(&self.root.join(DEVICES_FILE), &devices)?;
        state.devices = devices;
        state.pairing_outcomes.insert(
            request_id.to_string(),
            (PairingOutcome::Approved(view.clone()), now + PAIRING_RETENTION),
        );
        Ok(view)
    }

    pub fn approve_at(
        &self,
        secret: &[u8],
        device_name: &str,
        public_key: &[u8],
        now: SystemTime,
    ) -> Result<TrustedDevice, AuthError> {
        let pending = self.submit_pairing_at(secret, device_name, public_key, now)?;
        self.approve_pairing_at(&pending.id, now)
    }

    pub fn list_pending_pairings(&self) -> Vec<PendingPairing> {
        self.state
            .lock()
            .map(|state| {
                state
                    .pending_pairings
                    .iter()
                    .map(|pairing| pairing.view.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn take_pairing_outcome(&self, request_id: &str) -> Option<PairingOutcome> {
        self.state
            .lock()
            .ok()?
            .pairing_outcomes
            .remove(request_id)
            .map(|(outcome, _)| outcome)
    }

    /// Drop enrollments, pairing requests, and collected answers that have
    /// aged out. Called on every pairing submission; the gateway also calls it
    /// on a timer so an idle desktop does not hold stale state.
    pub fn prune_at(&self, now: SystemTime) {
        if let Ok(mut state) = self.state.lock() {
            prune_state(&mut state, now);
        }
    }

    /// How many in-flight pairing records the store is holding. Exposed so the
    /// desktop can show, and tests can assert, that nothing accumulates.
    pub fn pending_state_len(&self) -> usize {
        self.state
            .lock()
            .map(|state| {
                state.enrollments.len() + state.pending_pairings.len() + state.pairing_outcomes.len()
            })
            .unwrap_or(0)
    }

    pub fn deny_pairing(&self, request_id: &str) -> Result<bool, AuthError> {
        self.deny_pairing_at(request_id, SystemTime::now())
    }

    pub fn deny_pairing_at(&self, request_id: &str, now: SystemTime) -> Result<bool, AuthError> {
        let mut state = self.state.lock().map_err(|_| AuthError::poisoned())?;
        let original_len = state.pending_pairings.len();
        state
            .pending_pairings
            .retain(|pairing| pairing.view.id != request_id);
        if state.pending_pairings.len() == original_len {
            return Ok(false);
        }
        state
            .pairing_outcomes
            .insert(request_id.to_string(), (PairingOutcome::Denied, now + PAIRING_RETENTION));
        Ok(true)
    }

    pub fn verify_proof(
        &self,
        device_id: &str,
        nonce: &[u8],
        signature_der: &[u8],
    ) -> Result<(), AuthError> {
        self.verify_proof_at(device_id, nonce, signature_der, SystemTime::now())
    }

    pub fn verify_proof_at(
        &self,
        device_id: &str,
        nonce: &[u8],
        signature_der: &[u8],
        now: SystemTime,
    ) -> Result<(), AuthError> {
        let mut state = self.state.lock().map_err(|_| AuthError::poisoned())?;
        let position = state
            .devices
            .iter()
            .position(|device| device.view.id == device_id)
            .ok_or_else(|| AuthError::new("auth.unknown_device", "device is not trusted"))?;
        let public_key = URL_SAFE_NO_PAD
            .decode(&state.devices[position].public_key)
            .map_err(|_| AuthError::new("auth.invalid_device_record", "invalid stored key"))?;
        let verifying_key = VerifyingKey::from_sec1_bytes(&public_key)
            .map_err(|_| AuthError::new("auth.invalid_device_record", "invalid stored key"))?;
        let signature = Signature::from_der(signature_der)
            .map_err(|_| AuthError::new("auth.invalid_proof", "invalid device proof"))?;
        verifying_key
            .verify(nonce, &signature)
            .map_err(|_| AuthError::new("auth.invalid_proof", "invalid device proof"))?;

        // Only a proof that actually verified counts as a sighting. Recording
        // the attempt instead would let anyone who knows a device id keep the
        // settings panel showing a phone as freshly connected.
        let seen = unix_seconds(now);
        if state.devices[position].view.last_seen_at != Some(seen) {
            let mut devices = state.devices.clone();
            devices[position].view.last_seen_at = Some(seen);
            persist_devices(&self.root.join(DEVICES_FILE), &devices)?;
            state.devices = devices;
        }
        Ok(())
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

fn prune_state(state: &mut StoreState, now: SystemTime) {
    state.enrollments.retain(|item| item.expires_at >= now);
    prune_pairings(state, now);
}

fn prune_pairings(state: &mut StoreState, now: SystemTime) {
    state.pending_pairings.retain(|item| item.expires_at >= now);
    state
        .pairing_outcomes
        .retain(|_, (_, expires_at)| *expires_at >= now);
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
pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
pub(crate) fn set_private_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub(crate) fn set_private_permissions(_path: &Path, _mode: u32) -> std::io::Result<()> {
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
