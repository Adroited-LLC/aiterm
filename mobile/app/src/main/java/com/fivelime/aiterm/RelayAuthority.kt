package com.fivelime.aiterm

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec

/** This phone's say over a desktop's relay route. One P-256 key, born in
 *  the hardware keystore on first use and never leaving it: the desktop
 *  hands the phone an enrollment digest (`ta` in the QR), the phone signs
 *  it, and the relay learns that this key vouched for that route. No user
 *  authentication on the key — signing happens inside the pairing flow,
 *  where the person has just scanned the QR and nothing should ask twice. */
object RelayAuthority {
    const val ALIAS = "aiterm-relay-authority-p256-v1"
    private const val STORE = "AndroidKeyStore"

    private fun keyStore(): KeyStore = KeyStore.getInstance(STORE).apply { load(null) }

    private fun ensureKey(): KeyStore.PrivateKeyEntry {
        val ks = keyStore()
        (ks.getEntry(ALIAS, null) as? KeyStore.PrivateKeyEntry)?.let { return it }
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .build()
        KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, STORE).apply { initialize(spec) }.generateKeyPair()
        return ks.getEntry(ALIAS, null) as KeyStore.PrivateKeyEntry
    }

    private fun privateKey(): PrivateKey = ensureKey().privateKey
    private fun publicKey(): ECPublicKey = ensureKey().certificate.publicKey as ECPublicKey

    /** SEC1 compressed point: 0x02/0x03 (Y's parity) then X, 33 bytes. The
     *  relay insists on this canonical form. */
    fun publicKeyCompressed(): ByteArray {
        val w = publicKey().w
        val x = fixed32(w.affineX)
        val prefix: Byte = if (w.affineY.testBit(0)) 0x03 else 0x02
        return byteArrayOf(prefix) + x
    }

    /** ECDSA P-256 over SHA-256 of `digest32`, DER-encoded — what
     *  `p256::ecdsa::VerifyingKey::verify(&digest, &sig)` checks. */
    fun sign(digest32: ByteArray): ByteArray {
        require(digest32.size == 32) { "enrollment digest must be 32 bytes, got ${digest32.size}" }
        return Signature.getInstance("SHA256withECDSA").run {
            initSign(privateKey())
            update(digest32)
            sign()
        }
    }

    /** A coordinate as exactly 32 big-endian bytes: BigInteger drops leading
     *  zeros and may add a sign byte. */
    private fun fixed32(n: BigInteger): ByteArray {
        val raw = n.toByteArray()
        val out = ByteArray(32)
        val src = if (raw.size > 32) raw.copyOfRange(raw.size - 32, raw.size) else raw
        System.arraycopy(src, 0, out, 32 - src.size, src.size)
        return out
    }
}
