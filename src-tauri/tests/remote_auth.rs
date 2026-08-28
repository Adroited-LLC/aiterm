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
fn pairing_submission_consumes_the_qr_but_does_not_trust_before_desktop_approval() {
    let root = private_test_dir("pending-approval");
    let store = DeviceStore::open(&root).unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(1_500);
    let enrollment = store.begin_enrollment_at(now).unwrap();
    let key = SigningKey::random(&mut OsRng);

    let pending = store
        .submit_pairing_at(enrollment.secret(), "phone", &public_key_bytes(&key), now)
        .expect("valid QR should create a pending desktop approval");

    assert!(store.list_devices().is_empty());
    assert_eq!(store.list_pending_pairings(), vec![pending.clone()]);
    assert_eq!(
        store
            .submit_pairing_at(enrollment.secret(), "replay", &public_key_bytes(&key), now,)
            .unwrap_err()
            .code(),
        "pairing.invalid_or_consumed"
    );

    let approved = store
        .approve_pairing_at(&pending.id, now)
        .expect("desktop approval should trust the submitted key");
    assert_eq!(approved.name, "phone");
    assert!(store.list_pending_pairings().is_empty());
    assert_eq!(store.list_devices(), vec![approved]);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn desktop_denial_removes_the_pending_request_without_trusting_the_device() {
    use aiterm_lib::remote::auth::PairingOutcome;

    let root = private_test_dir("deny-pending");
    let store = DeviceStore::open(&root).unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(1_700);
    let enrollment = store.begin_enrollment_at(now).unwrap();
    let key = SigningKey::random(&mut OsRng);
    let pending = store
        .submit_pairing_at(enrollment.secret(), "phone", &public_key_bytes(&key), now)
        .unwrap();

    assert!(store.deny_pairing(&pending.id).unwrap());
    assert!(store.list_pending_pairings().is_empty());
    assert!(store.list_devices().is_empty());
    assert_eq!(
        store.take_pairing_outcome(&pending.id),
        Some(PairingOutcome::Denied)
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

#[test]
fn a_successful_proof_records_when_the_device_was_last_seen() {
    let root = private_test_dir("last-seen");
    let store = DeviceStore::open(&root).expect("empty device store should open");
    let paired_at = UNIX_EPOCH + Duration::from_secs(1_000);
    let enrollment = store
        .begin_enrollment_at(paired_at)
        .expect("enrollment should start");
    let key = SigningKey::random(&mut OsRng);
    let device = store
        .approve_at(
            enrollment.secret(),
            "phone",
            &public_key_bytes(&key),
            paired_at,
        )
        .expect("enrollment should approve");
    assert_eq!(
        device.last_seen_at, None,
        "a device that has never connected has not been seen"
    );

    let connected_at = paired_at + Duration::from_secs(60);
    let signature: Signature = key.sign(b"nonce");
    store
        .verify_proof_at(&device.id, b"nonce", signature.to_der().as_bytes(), connected_at)
        .expect("the paired key should prove identity");

    let listed = store
        .list_devices()
        .into_iter()
        .find(|listed| listed.id == device.id)
        .expect("the device should still be trusted");
    assert_eq!(
        listed.last_seen_at,
        Some(1_060),
        "the desktop device list must show when the phone last connected"
    );

    // The timestamp has to survive a desktop restart, or the settings panel
    // shows "never seen" for a phone that connects every day.
    let reopened = DeviceStore::open(&root).expect("device store should reopen");
    assert_eq!(
        reopened
            .list_devices()
            .into_iter()
            .find(|listed| listed.id == device.id)
            .expect("the device should still be trusted")
            .last_seen_at,
        Some(1_060)
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_failed_proof_does_not_record_a_sighting() {
    let root = private_test_dir("last-seen-failure");
    let store = DeviceStore::open(&root).expect("empty device store should open");
    let paired_at = UNIX_EPOCH + Duration::from_secs(1_000);
    let enrollment = store
        .begin_enrollment_at(paired_at)
        .expect("enrollment should start");
    let key = SigningKey::random(&mut OsRng);
    let device = store
        .approve_at(
            enrollment.secret(),
            "phone",
            &public_key_bytes(&key),
            paired_at,
        )
        .expect("enrollment should approve");

    let impostor = SigningKey::random(&mut OsRng);
    let signature: Signature = impostor.sign(b"nonce");
    store
        .verify_proof_at(
            &device.id,
            b"nonce",
            signature.to_der().as_bytes(),
            paired_at + Duration::from_secs(60),
        )
        .expect_err("a signature from another key must not prove identity");

    assert_eq!(
        store
            .list_devices()
            .into_iter()
            .find(|listed| listed.id == device.id)
            .expect("the device should still be trusted")
            .last_seen_at,
        None,
        "a rejected proof must not look like a successful connection"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pairing_the_same_phone_again_replaces_its_row_instead_of_adding_one() {
    let root = private_test_dir("repair");
    let store = DeviceStore::open(&root).expect("empty device store should open");
    let key = SigningKey::random(&mut OsRng);
    let first_at = UNIX_EPOCH + Duration::from_secs(1_000);
    let first = store
        .approve_at(
            store
                .begin_enrollment_at(first_at)
                .expect("enrollment should start")
                .secret(),
            "phone",
            &public_key_bytes(&key),
            first_at,
        )
        .expect("enrollment should approve");

    let second_at = first_at + Duration::from_secs(3_600);
    let second = store
        .approve_at(
            store
                .begin_enrollment_at(second_at)
                .expect("enrollment should start")
                .secret(),
            "phone renamed",
            &public_key_bytes(&key),
            second_at,
        )
        .expect("re-pairing the same key should approve");

    let devices = store.list_devices();
    assert_eq!(
        devices.len(),
        1,
        "one phone must not occupy two rows in the trusted-device list: {devices:?}"
    );
    assert_eq!(devices[0].id, second.id);
    assert_eq!(devices[0].name, "phone renamed");

    // The superseded identity must be worthless, or revoking the row the user
    // can see would leave a second working credential behind.
    let signature: Signature = key.sign(b"nonce");
    store
        .verify_proof_at(
            &first.id,
            b"nonce",
            signature.to_der().as_bytes(),
            second_at,
        )
        .expect_err("the replaced device id must no longer authenticate");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn stale_enrollments_and_uncollected_outcomes_do_not_accumulate() {
    let root = private_test_dir("prune");
    let store = DeviceStore::open(&root).expect("empty device store should open");
    let started = UNIX_EPOCH + Duration::from_secs(1_000);
    for _ in 0..5 {
        store
            .begin_enrollment_at(started)
            .expect("enrollment should start");
    }
    let abandoned = store
        .begin_enrollment_at(started)
        .expect("enrollment should start");
    let key = SigningKey::random(&mut OsRng);
    let pending = store
        .submit_pairing_at(
            abandoned.secret(),
            "phone",
            &public_key_bytes(&key),
            started,
        )
        .expect("pairing should be submitted");
    store
        .deny_pairing_at(&pending.id, started)
        .expect("the desktop should be able to deny");

    // A phone that never comes back to collect its answer must not pin the
    // record in memory for the life of the process.
    let much_later = started + Duration::from_secs(86_400);
    store.prune_at(much_later);
    assert_eq!(store.pending_state_len(), 0);

    // A live enrollment survives pruning.
    let fresh = store
        .begin_enrollment_at(much_later)
        .expect("enrollment should start");
    store.prune_at(much_later + Duration::from_secs(1));
    store
        .submit_pairing_at(fresh.secret(), "phone", &public_key_bytes(&key), much_later)
        .expect("an unexpired enrollment must survive pruning");
    std::fs::remove_dir_all(root).ok();
}
