package com.adroited.aiterm.pairing

/**
 * A desktop this phone has already enrolled with.
 *
 * Only non-secret metadata lives here. The device private key and the desktop
 * credential stay in Android Keystore (Task 8); [serverSpkiFingerprint] is the
 * pinned SHA-256 SPKI fingerprint from the pairing QR and is safe to display.
 */
data class PairedDesktop(
    val deviceId: String,
    val displayName: String,
    val serverSpkiFingerprint: String,
    val lastSeenEpochMillis: Long?,
)
