package com.adroited.aiterm.remote

import com.adroited.aiterm.pairing.AuthChallengeFrame
import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.pairing.PairingFrames
import com.adroited.aiterm.security.DeviceKeys
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.runCurrent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class AuthenticatedRemoteTransportTest {

    @Test
    fun aLockedAppNeverSignsTheOpeningChallenge() = runTest {
        val socket = FakeBinarySocket().apply {
            incoming.trySend(PairingFrames.encode(AuthChallengeFrame(ByteArray(32) { 7 })))
        }
        val keys = RecordingDeviceKeys()
        val transport = AuthenticatedRemoteTransport(
            desktop = desktop(),
            deviceKeys = keys,
            isUnlocked = { false },
            dialer = FakeDialer(socket),
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )

        var rejected = false
        try {
            transport.connect()
        } catch (_: RemoteProtocolException) {
            rejected = true
        }
        assertTrue(rejected)
        assertEquals(0, keys.signCount)
        assertTrue(socket.closed)
    }

    @Test
    fun anUnlockedClientSignsOnceAndRequiresAuthOk() = runTest {
        val socket = FakeBinarySocket().apply {
            incoming.trySend(PairingFrames.encode(AuthChallengeFrame(ByteArray(32) { 3 })))
            incoming.trySend(hex("a1646b696e6467617574682e6f6b"))
        }
        val keys = RecordingDeviceKeys()
        val transport = AuthenticatedRemoteTransport(
            desktop = desktop(),
            deviceKeys = keys,
            isUnlocked = { true },
            dialer = FakeDialer(socket),
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )

        transport.connect()

        assertEquals(1, keys.signCount)
        assertEquals(1, socket.sent.size)
        assertTrue(socket.sent.single().isNotEmpty())
        transport.close()
    }

    @Test
    fun closeWhileDialingPreventsTheLateSocketFromSigningOrPublishing() = runTest {
        val socket = FakeBinarySocket().apply {
            incoming.trySend(PairingFrames.encode(AuthChallengeFrame(ByteArray(32) { 5 })))
            incoming.trySend(hex("a1646b696e6467617574682e6f6b"))
        }
        val dialer = DeferredDialer()
        val keys = RecordingDeviceKeys()
        val transport = AuthenticatedRemoteTransport(
            desktop = desktop(),
            deviceKeys = keys,
            isUnlocked = { true },
            dialer = dialer,
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )

        val connecting = async { runCatching { transport.connect() } }
        runCurrent()
        transport.close()
        dialer.socket.complete(socket)
        runCurrent()

        assertTrue(connecting.await().isFailure)
        assertEquals(0, keys.signCount)
        assertTrue(socket.closed)
    }

    @Test
    fun lockDuringKeystoreSignatureDiscardsProofAndClosesCandidate() = runTest {
        val socket = FakeBinarySocket().apply {
            incoming.trySend(PairingFrames.encode(AuthChallengeFrame(ByteArray(32) { 6 })))
            incoming.trySend(hex("a1646b696e6467617574682e6f6b"))
        }
        var unlocked = true
        val keys = RecordingDeviceKeys { unlocked = false }
        val transport = AuthenticatedRemoteTransport(
            desktop = desktop(),
            deviceKeys = keys,
            isUnlocked = { unlocked },
            dialer = FakeDialer(socket),
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )

        val result = runCatching { transport.connect() }

        assertTrue(result.isFailure)
        assertEquals(1, keys.signCount)
        assertEquals(0, socket.sent.size)
        assertTrue(socket.closed)
    }

    @Test
    fun explicitAuthenticationDenialIsReportedAsRevocation() = runTest {
        val socket = FakeBinarySocket().apply {
            incoming.trySend(PairingFrames.encode(AuthChallengeFrame(ByteArray(32) { 8 })))
            incoming.trySend(hex("a1646b696e646b617574682e64656e696564"))
        }
        val transport = AuthenticatedRemoteTransport(
            desktop = desktop(),
            deviceKeys = RecordingDeviceKeys(),
            isUnlocked = { true },
            dialer = FakeDialer(socket),
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )

        val result = runCatching { transport.connect() }

        assertTrue(result.exceptionOrNull() is RemoteAccessRevokedException)
        assertTrue(socket.closed)
    }

    @Test
    fun responsesCompleteOnlyTheirCorrelatedPendingRequest() = runTest {
        val socket = FakeBinarySocket().apply {
            incoming.trySend(PairingFrames.encode(AuthChallengeFrame(ByteArray(32) { 3 })))
            incoming.trySend(hex("a1646b696e6467617574682e6f6b"))
        }
        val transport = AuthenticatedRemoteTransport(
            desktop = desktop(),
            deviceKeys = RecordingDeviceKeys(),
            isUnlocked = { true },
            dialer = FakeDialer(socket),
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        transport.connect()

        val response = async {
            transport.request(RemoteRequest(1, "tab.list", byteArrayOf()))
        }
        runCurrent()
        socket.incoming.send(
            hex(
                "a4" +
                    "6776657273696f6e01" +
                    "6a726571756573745f696401" +
                    "646b696e64687461622e6c697374" +
                    "677061796c6f616440",
            ),
        )
        runCurrent()

        assertEquals("tab.list", (response.await() as RemoteResponse.Success).kind)
        transport.close()
    }

    private fun desktop() = PairedDesktop(
        deviceId = "device-1",
        displayName = "Desktop",
        hosts = listOf("desktop.local"),
        port = 43871,
        serverSpkiFingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        lastSeenEpochMillis = null,
    )

    private fun hex(value: String): ByteArray =
        value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}

private class RecordingDeviceKeys(private val afterSign: () -> Unit = {}) : DeviceKeys {
    var signCount = 0
    override fun devicePublicKey(): ByteArray = ByteArray(33)
    override fun signChallenge(nonce: ByteArray): ByteArray {
        signCount++
        afterSign()
        return byteArrayOf(1, 2, 3)
    }
}

private class DeferredDialer : RemoteSocketDialer {
    val socket = CompletableDeferred<RemoteBinarySocket>()
    override suspend fun open(desktop: PairedDesktop): RemoteBinarySocket = socket.await()
}

private class FakeDialer(private val socket: RemoteBinarySocket) : RemoteSocketDialer {
    override suspend fun open(desktop: PairedDesktop): RemoteBinarySocket = socket
}

private class FakeBinarySocket : RemoteBinarySocket {
    val incoming = Channel<ByteArray>(Channel.UNLIMITED)
    val sent = mutableListOf<ByteArray>()
    var closed = false

    override suspend fun receive(): ByteArray = incoming.receive()
    override fun send(bytes: ByteArray): Boolean {
        sent += bytes.copyOf()
        return true
    }
    override fun close() {
        closed = true
        incoming.close()
    }
}
