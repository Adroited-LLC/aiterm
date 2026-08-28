use aiterm_lib::remote::auth::DeviceStore;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use rand_core::OsRng;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

fn private_test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("aiterm-remote-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).expect("test state directory should be created");
    path
}

fn public_key_bytes(signing_key: &SigningKey) -> Vec<u8> {
    signing_key
        .verifying_key()
        .to_encoded_point(true)
        .as_bytes()
        .to_vec()
}

#[test]
fn enrollment_can_be_approved_exactly_once_before_expiry() {
    let root = private_test_dir("approve-once");
    let store = DeviceStore::open(&root).expect("empty device store should open");
    let started = UNIX_EPOCH + Duration::from_secs(1_000);
    let enrollment = store
        .begin_enrollment_at(started)
        .expect("enrollment should start");
    let key = SigningKey::random(&mut OsRng);

    let device = store
        .approve_at(
            enrollment.secret(),
            "Matt's phone",
            &public_key_bytes(&key),
            started + Duration::from_secs(299),
        )
        .expect("unexpired enrollment should approve");

    assert_eq!(device.name, "Matt's phone");
    assert_eq!(store.list_devices().len(), 1);
    assert_eq!(
        store
            .approve_at(
                enrollment.secret(),
                "replay",
                &public_key_bytes(&key),
                started + Duration::from_secs(299),
            )
            .unwrap_err()
            .code(),
        "pairing.invalid_or_consumed"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn expired_enrollment_is_consumed_and_cannot_be_retried() {
    let root = private_test_dir("expired");
    let store = DeviceStore::open(&root).unwrap();
    let started = UNIX_EPOCH + Duration::from_secs(2_000);
    let enrollment = store.begin_enrollment_at(started).unwrap();
    let key = SigningKey::random(&mut OsRng);
    let expired = started + Duration::from_secs(301);

    assert_eq!(
        store
            .approve_at(
                enrollment.secret(),
                "late phone",
                &public_key_bytes(&key),
                expired,
            )
            .unwrap_err()
            .code(),
        "pairing.expired"
    );
    assert_eq!(
        store
            .approve_at(
                enrollment.secret(),
                "late phone",
                &public_key_bytes(&key),
                expired,
            )
            .unwrap_err()
            .code(),
        "pairing.invalid_or_consumed"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn device_identity_requires_a_signature_from_the_paired_key() {
    let root = private_test_dir("proof");
    let store = DeviceStore::open(&root).unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(3_000);
    let enrollment = store.begin_enrollment_at(now).unwrap();
    let paired_key = SigningKey::random(&mut OsRng);
    let impostor_key = SigningKey::random(&mut OsRng);
    let device = store
        .approve_at(
            enrollment.secret(),
            "phone",
            &public_key_bytes(&paired_key),
            now,
        )
        .unwrap();
    let nonce = b"fresh server nonce";
    let valid: Signature = paired_key.sign(nonce);
    let invalid: Signature = impostor_key.sign(nonce);

    store
        .verify_proof(&device.id, nonce, valid.to_der().as_bytes())
        .expect("paired key should prove possession");
    assert_eq!(
        store
            .verify_proof(&device.id, nonce, invalid.to_der().as_bytes())
            .unwrap_err()
            .code(),
        "auth.invalid_proof"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn revoked_device_is_removed_from_disk_and_cannot_authenticate_after_restart() {
    let root = private_test_dir("revoke");
    let store = DeviceStore::open(&root).unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(4_000);
    let enrollment = store.begin_enrollment_at(now).unwrap();
    let key = SigningKey::random(&mut OsRng);
    let device = store
        .approve_at(enrollment.secret(), "phone", &public_key_bytes(&key), now)
        .unwrap();
    assert!(store.revoke(&device.id).expect("revocation should persist"));
    drop(store);

    let restarted = DeviceStore::open(&root).expect("persisted store should reload");
    let signature: Signature = key.sign(b"nonce after restart");
    assert!(restarted.list_devices().is_empty());
    assert_eq!(
        restarted
            .verify_proof(
                &device.id,
                b"nonce after restart",
                signature.to_der().as_bytes(),
            )
            .unwrap_err()
            .code(),
        "auth.unknown_device"
    );
    std::fs::remove_dir_all(root).ok();
}

#[cfg(unix)]
#[test]
fn persisted_device_store_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let root = private_test_dir("permissions");
    let store = DeviceStore::open(&root).unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(5_000);
    let enrollment = store.begin_enrollment_at(now).unwrap();
    let key = SigningKey::random(&mut OsRng);
    store
        .approve_at(enrollment.secret(), "phone", &public_key_bytes(&key), now)
        .unwrap();

    let dir_mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    let file_mode = std::fs::metadata(root.join("trusted-devices.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    assert_eq!(file_mode, 0o600);
    std::fs::remove_dir_all(root).ok();
}
