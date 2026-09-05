package com.adroited.aiterm.pairing

import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

class RelayBootstrapTest {
    private val bootstrap = RelayBootstrap("https://control.example.com", "desktop-12345678", "ab".repeat(32))
    private val fingerprint = ByteArray(32) { 7 }.toBase64Url()
    private val relay = "desktop-12345678.relay.example.com"
    private fun uri() = pairingUri(version = "4", fingerprint = fingerprint,
        relayHost = relay, relayPort = 443, relayAuthorizationDigest = bootstrap.digest(fingerprint)) +
        "&c=https%3A%2F%2Fcontrol.example.com&t=${bootstrap.tokenHash}"

    @Test fun bootstrapFieldsAreBoundToTheSignedDigest() {
        assertEquals(bootstrap, parsedPayload(uri()).relayBootstrap)
        for (tampered in listOf(
            uri().replace("control.example.com", "other.example.com"),
            uri().replace("desktop-12345678", "desktop-87654321"),
            uri().replace(bootstrap.tokenHash, "cd".repeat(32)),
            uri().replace(fingerprint, ByteArray(32) { 8 }.toBase64Url()),
            uri().replace("https%3A", "http%3A"),
            uri().replace("v=4", "v=3"),
        )) assertEquals(PairingPayloadResult.Rejected(PairingFailure.MALFORMED_PAYLOAD), PairingPayload.parse(tampered, 0))
    }

    @Test fun unreachableDesktopIsProvisionedBeforeRelayPairing() = runTest {
        val transport = RecordingPairingTransport(mapOf(relay to EnrollmentOutcome.Approved("device-1")))
        var provisioned = false
        val repository = PairingRepository(transport, FakeDeviceKeys(), FakePairedDesktopStore(),
            relayProvisioner = RelayProvisioner { draft, pin, publicKey, signature ->
                assertEquals(listOf("localhost"), transport.attempted.map { it.host })
                assertEquals(bootstrap, draft)
                assertEquals(fingerprint, pin)
                assertEquals(33, publicKey.size)
                assertTrue(signature.isNotEmpty())
                provisioned = true
                true
            })
        assertTrue(repository.pair(parsedPayload(uri()), "phone", 0) is PairingResult.Paired)
        assertTrue(provisioned)
        assertEquals(listOf("localhost", relay), transport.attempted.map { it.host })
    }

    @Test fun successfulDirectPairingDoesNotProvisionFromThePhone() = runTest {
        val transport = RecordingPairingTransport(mapOf("localhost" to EnrollmentOutcome.Approved("device-1")))
        val repository = PairingRepository(transport, FakeDeviceKeys(), FakePairedDesktopStore(),
            relayProvisioner = RelayProvisioner { _, _, _, _ -> error("Direct pairing must stay unchanged") })
        assertTrue(repository.pair(parsedPayload(uri()), "phone", 0) is PairingResult.Paired)
    }

    @Test fun failedProvisioningDoesNotConsumeTheSecretOnARelay() = runTest {
        val transport = RecordingPairingTransport(emptyMap())
        val repository = PairingRepository(transport, FakeDeviceKeys(), FakePairedDesktopStore(),
            relayProvisioner = RelayProvisioner { _, _, _, _ -> false })
        val payload = parsedPayload(uri())
        assertEquals(PairingResult.Rejected(PairingFailure.UNREACHABLE), repository.pair(payload, "phone", 0))
        assertEquals(listOf("localhost"), transport.attempted.map { it.host })
        assertFalse(payload.secret.isAvailable())
    }

    @Test fun relayProvisioningDoesNotBypassDesktopApproval() = runTest {
        val transport = RecordingPairingTransport(mapOf(relay to EnrollmentOutcome.Denied))
        val store = FakePairedDesktopStore()
        val repository = PairingRepository(transport, FakeDeviceKeys(), store,
            relayProvisioner = RelayProvisioner { _, _, _, _ -> true })
        assertEquals(PairingResult.Rejected(PairingFailure.DENIED_BY_DESKTOP), repository.pair(parsedPayload(uri()), "phone", 0))
        assertTrue(store.all().isEmpty())
    }

    @Test fun provisionedRelayCannotSubstituteAnotherDesktopIdentity() = runTest {
        val transport = RecordingPairingTransport(mapOf(relay to EnrollmentOutcome.FingerprintMismatch))
        val store = FakePairedDesktopStore()
        val repository = PairingRepository(transport, FakeDeviceKeys(), store,
            relayProvisioner = RelayProvisioner { _, _, _, _ -> true })
        assertEquals(PairingResult.Rejected(PairingFailure.FINGERPRINT_MISMATCH), repository.pair(parsedPayload(uri()), "phone", 0))
        assertEquals(listOf("localhost", relay), transport.attempted.map { it.host })
        assertTrue(store.all().isEmpty())
    }
}
