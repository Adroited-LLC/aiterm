package com.adroited.aiterm.remote

import com.adroited.aiterm.pairing.AuthChallengeFrame
import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.pairing.PairingFrames
import com.adroited.aiterm.pairing.PairingProtocolException
import com.adroited.aiterm.security.DeviceKeys
import java.util.concurrent.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout

interface RemoteBinarySocket {
    suspend fun receive(): ByteArray
    fun send(bytes: ByteArray): Boolean
    fun close()
}

interface RemoteSocketDialer {
    suspend fun open(desktop: PairedDesktop): RemoteBinarySocket
}

/** Authenticated, bounded, request-correlated remote protocol transport. */
class AuthenticatedRemoteTransport(
    private val desktop: PairedDesktop,
    private val deviceKeys: DeviceKeys,
    private val isUnlocked: () -> Boolean,
    private val dialer: RemoteSocketDialer,
    private val scope: CoroutineScope,
    private val dispatcher: CoroutineDispatcher = Dispatchers.IO,
) : RemoteTransport {
    private val eventChannel = Channel<RemoteServerEvent>(
        capacity = MAX_EVENTS,
        onBufferOverflow = BufferOverflow.SUSPEND,
    )
    override val events: Flow<RemoteServerEvent> = eventChannel.receiveAsFlow()

    private val stateLock = Any()
    private val pending = LinkedHashMap<Long, PendingRequest>()
    private val completed = LinkedHashSet<Long>()
    private var socket: RemoteBinarySocket? = null
    private var connectingSocket: RemoteBinarySocket? = null
    private var readerJob: Job? = null
    private var started = false
    private var closed = false

    override suspend fun connect() {
        synchronized(stateLock) {
            if (started || closed) throw RemoteProtocolException("remote transport is closed")
            started = true
        }
        val candidate = dialer.open(desktop)
        try {
            synchronized(stateLock) {
                if (closed) {
                    candidate.close()
                    throw RemoteProtocolException("remote transport is closed")
                }
                connectingSocket = candidate
            }
            val challengeBytes = withTimeout(AUTH_TIMEOUT_MILLIS) { candidate.receive() }
            val challenge = try {
                PairingFrames.decode(challengeBytes) as? AuthChallengeFrame
            } catch (_: PairingProtocolException) {
                null
            } ?: throw RemoteProtocolException("the desktop did not send an authentication challenge")
            // This check is deliberately immediately before the Keystore call.
            // A socket opening while locked must never cause a signature prompt.
            if (!isUnlocked()) throw RemoteProtocolException("unlock is required before authentication")
            val signature = deviceKeys.signChallenge(challenge.nonce)
            ensureOpenAndUnlocked(candidate)
            val proof = RemoteWireCodec.encodeAuthProof(desktop.deviceId, signature)
            try {
                if (!candidate.send(proof)) throw RemoteProtocolException("authentication proof send failed")
            } finally {
                proof.fill(0)
                signature.fill(0)
                challenge.nonce.fill(0)
            }
            RemoteWireCodec.decodeAuthOk(withTimeout(AUTH_TIMEOUT_MILLIS) { candidate.receive() })
            ensureOpenAndUnlocked(candidate)
            synchronized(stateLock) {
                if (closed || connectingSocket !== candidate) {
                    throw RemoteProtocolException("remote transport is closed")
                }
                connectingSocket = null
                socket = candidate
            }
            readerJob = scope.launch(dispatcher) { readLoop(candidate) }
        } catch (error: Exception) {
            synchronized(stateLock) {
                if (connectingSocket === candidate) connectingSocket = null
                if (socket === candidate) socket = null
            }
            candidate.close()
            throw error
        }
    }

    override suspend fun request(request: RemoteRequest): RemoteResponse {
        val deferred = CompletableDeferred<RemoteResponse>()
        val active = synchronized(stateLock) {
            if (request.requestId <= 0 || pending.containsKey(request.requestId) ||
                completed.contains(request.requestId) || pending.size >= MAX_PENDING_REQUESTS
            ) {
                throw RemoteProtocolException("invalid or over-bound remote request")
            }
            val current = socket ?: throw RemoteProtocolException("remote transport is disconnected")
            pending[request.requestId] = PendingRequest(request.kind, deferred)
            current
        }
        val encoded = RemoteWireCodec.encodeRequest(request)
        val sent = try {
            active.send(encoded)
        } finally {
            encoded.fill(0)
        }
        if (!sent) {
            synchronized(stateLock) { pending.remove(request.requestId) }
            throw RemoteProtocolException("remote request send failed")
        }
        return try {
            withTimeout(REQUEST_TIMEOUT_MILLIS) { deferred.await() }
        } finally {
            val abandoned = synchronized(stateLock) { pending.remove(request.requestId) != null }
            if (abandoned) rememberCompleted(request.requestId)
        }
    }

    override fun close() {
        val failure = RemoteProtocolException("remote transport disconnected")
        val toFail: List<CompletableDeferred<RemoteResponse>>
        synchronized(stateLock) {
            closed = true
            readerJob?.cancel()
            readerJob = null
            connectingSocket?.close()
            connectingSocket = null
            socket?.close()
            socket = null
            toFail = pending.values.map(PendingRequest::deferred)
            pending.clear()
            completed.clear()
        }
        toFail.forEach { it.completeExceptionally(failure) }
    }

    private suspend fun readLoop(active: RemoteBinarySocket) {
        try {
            while (true) accept(RemoteWireCodec.decodeEvent(active.receive()))
        } catch (_: CancellationException) {
            // Explicit close/lock owns teardown.
        } catch (error: Exception) {
            emitOrClose(RemoteServerEvent.Failure("transport.disconnected", error.message ?: "Connection ended"))
            close()
        }
    }

    private fun accept(event: RemoteEventEnvelope) {
        when (event.kind) {
            "terminal.snapshot", "terminal.diff", "terminal.scrollback" -> {
                requireKnownCorrelation(event.requestId)
                val chunk = RemoteWireCodec.decodeTerminalChunk(event.payload, event.requestId)
                emitOrClose(
                    RemoteServerEvent.TerminalChunk(chunk),
                )
                if (chunk.kind == TerminalTransferKind.Scrollback && chunk.index + 1 == chunk.total &&
                    event.requestId > 0
                ) completeTransferOnlyRequest(event)
            }
            "state.snapshot" -> {
                if (event.requestId != 0L) protocolFailure()
                emitOrClose(RemoteServerEvent.RosterChunk(RemoteWireCodec.decodeStateSnapshot(event.payload)))
            }
            "auth.revoked" -> {
                if (event.requestId != 0L) protocolFailure()
                emitOrClose(RemoteServerEvent.Revoked)
                close()
            }
            "error" -> acceptError(event)
            "session.changed", "agent.changed", "tab.changed", "terminal.exited",
            "terminal.title", "terminal.focus_changed" -> {
                requireKnownCorrelation(event.requestId)
                emitOrClose(RemoteServerEvent.Raw(event.kind, event.payload))
            }
            else -> acceptResponse(event)
        }
    }

    private fun acceptResponse(event: RemoteEventEnvelope) {
        if (event.requestId <= 0) protocolFailure()
        val request = synchronized(stateLock) { pending.remove(event.requestId) }
        if (request == null) {
            if (synchronized(stateLock) { completed.contains(event.requestId) }) return
            protocolFailure()
        }
        if (event.kind != request.kind) protocolFailure()
        rememberCompleted(event.requestId)
        request.deferred.complete(RemoteResponse.Success(event.requestId, event.kind, event.payload))
    }

    private fun completeTransferOnlyRequest(event: RemoteEventEnvelope) {
        val request = synchronized(stateLock) { pending.remove(event.requestId) }
        if (request == null) {
            if (synchronized(stateLock) { completed.contains(event.requestId) }) return
            protocolFailure()
        }
        if (request.kind != "terminal.scrollback") protocolFailure()
        rememberCompleted(event.requestId)
        request.deferred.complete(RemoteResponse.Success(event.requestId, request.kind, event.payload))
    }

    private fun acceptError(event: RemoteEventEnvelope) {
        val error = RemoteWireCodec.decodeError(event.payload)
        if (event.requestId == 0L) {
            emitOrClose(RemoteServerEvent.Failure(error.code, error.message))
            return
        }
        val request = synchronized(stateLock) { pending.remove(event.requestId) }
        if (request == null) {
            if (synchronized(stateLock) { completed.contains(event.requestId) }) return
            protocolFailure()
        }
        rememberCompleted(event.requestId)
        request.deferred.complete(RemoteResponse.Error(event.requestId, error.code, error.message))
    }

    private fun requireKnownCorrelation(requestId: Long) {
        if (requestId == 0L) return
        val known = synchronized(stateLock) { pending.containsKey(requestId) || completed.contains(requestId) }
        if (!known) protocolFailure()
    }

    private fun rememberCompleted(requestId: Long) = synchronized(stateLock) {
        completed += requestId
        while (completed.size > MAX_COMPLETED_CORRELATIONS) {
            completed.remove(completed.first())
        }
    }

    private fun emitOrClose(event: RemoteServerEvent) {
        if (eventChannel.trySend(event).isFailure) {
            throw RemoteProtocolException("remote event queue overflow")
        }
    }

    private fun ensureOpenAndUnlocked(candidate: RemoteBinarySocket) {
        if (!isUnlocked() || synchronized(stateLock) { closed || connectingSocket !== candidate }) {
            throw RemoteProtocolException("unlock is required before authentication")
        }
    }

    private fun protocolFailure(): Nothing = throw RemoteProtocolException("uncorrelated remote response")

    private data class PendingRequest(
        val kind: String,
        val deferred: CompletableDeferred<RemoteResponse>,
    )

    private companion object {
        const val AUTH_TIMEOUT_MILLIS = 10_000L
        // The desktop bounds descriptor-safe session work at 120 seconds.
        // Keep a small transport grace period so its correlated result wins
        // rather than reconnecting while a protected delete is still active.
        const val REQUEST_TIMEOUT_MILLIS = 130_000L
        const val MAX_PENDING_REQUESTS = 64
        const val MAX_COMPLETED_CORRELATIONS = 64
        const val MAX_EVENTS = 64
    }
}
