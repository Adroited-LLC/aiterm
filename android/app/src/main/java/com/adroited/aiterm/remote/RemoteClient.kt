package com.adroited.aiterm.remote

import com.adroited.aiterm.terminal.TerminalScreenStore
import com.adroited.aiterm.terminal.ApplyResult
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

@Serializable
data class TerminalSize(val cols: Int, val rows: Int) {
    init {
        require(cols in 1..512 && rows in 1..512)
    }
}

enum class FocusOwner { Self, Other, Unowned }
enum class ConnectionState { Disconnected, Connecting, Connected, Reconnecting, Locked, Revoked }

data class RemoteClientState(
    val connection: ConnectionState = ConnectionState.Disconnected,
    val focus: FocusOwner = FocusOwner.Unowned,
    val readOnly: Boolean = true,
    val showTakeFocus: Boolean = false,
    val pendingTransfers: Int = 0,
    val lastError: String? = null,
)

data class RemoteRequest(val requestId: Long, val kind: String, val payload: ByteArray)

sealed interface RemoteResponse {
    val requestId: Long
    data class Success(
        override val requestId: Long,
        val kind: String,
        val payload: ByteArray,
    ) : RemoteResponse
    data class Error(
        override val requestId: Long,
        val code: String,
        val message: String,
    ) : RemoteResponse
}

sealed interface RemoteServerEvent {
    data class FocusChanged(
        val tabId: String,
        val attachmentId: String,
        val focus: FocusOwner,
        val size: TerminalSize,
    ) : RemoteServerEvent
    data class TransferStarted(val transferId: String) : RemoteServerEvent
    data class TransferFinished(val transferId: String) : RemoteServerEvent
    data class TerminalChunk(val chunk: TerminalTransferChunk) : RemoteServerEvent
    data class Failure(val code: String, val message: String) : RemoteServerEvent
    data object Revoked : RemoteServerEvent
}

interface RemoteTransport {
    val events: Flow<RemoteServerEvent>
    suspend fun connect()
    suspend fun request(request: RemoteRequest): RemoteResponse
    fun close()
}

class RemoteClient(
    private val transportFactory: () -> RemoteTransport,
    private val screenStore: TerminalScreenStore,
    private val isUnlocked: () -> Boolean,
    private val scope: CoroutineScope,
    private val dispatcher: CoroutineDispatcher = Dispatchers.IO,
) {
    private val mutableState = MutableStateFlow(RemoteClientState())
    val state: StateFlow<RemoteClientState> = mutableState.asStateFlow()
    private val nextRequestId = AtomicLong(1)
    private val transfers = linkedSetOf<String>()
    private val terminalAssembler = TerminalTransferAssembler()
    private var transport: RemoteTransport? = null
    private var eventJob: Job? = null
    private var recoveryRequested = false

    suspend fun connect(): Boolean {
        if (!isUnlocked()) {
            lock()
            return false
        }
        closeTransport()
        val candidate = transportFactory()
        transport = candidate
        mutableState.value = mutableState.value.copy(connection = ConnectionState.Connecting)
        return try {
            candidate.connect()
            mutableState.value = mutableState.value.copy(connection = ConnectionState.Connected)
            eventJob = scope.launch(dispatcher) { candidate.events.collect(::accept) }
            true
        } catch (error: Exception) {
            candidate.close()
            transport = null
            mutableState.value = mutableState.value.copy(
                connection = ConnectionState.Disconnected,
                lastError = error.message ?: "Connection failed",
            )
            false
        }
    }

    fun sendInput(text: String): Boolean {
        if (text.isEmpty() || mutableState.value.focus != FocusOwner.Self) {
            mutableState.value = mutableState.value.copy(
                readOnly = true,
                showTakeFocus = true,
            )
            return false
        }
        val active = transport ?: return false
        val request = RemoteRequest(nextRequestId.getAndIncrement(), "terminal.input", text.encodeToByteArray())
        scope.launch(dispatcher) {
            when (val response = active.request(request)) {
                is RemoteResponse.Error -> accept(RemoteServerEvent.Failure(response.code, response.message))
                is RemoteResponse.Success -> Unit
            }
        }
        return true
    }

    fun lock() {
        closeTransport()
        transfers.clear()
        terminalAssembler.clear()
        recoveryRequested = false
        screenStore.clear()
        mutableState.value = RemoteClientState(connection = ConnectionState.Locked)
    }

    internal fun acceptForTest(event: RemoteServerEvent) = accept(event)

    @Synchronized
    private fun accept(event: RemoteServerEvent) {
        when (event) {
            is RemoteServerEvent.FocusChanged -> mutableState.value = mutableState.value.copy(
                focus = event.focus,
                readOnly = event.focus != FocusOwner.Self,
                showTakeFocus = event.focus != FocusOwner.Self,
            )
            is RemoteServerEvent.TransferStarted -> {
                if (transfers.size >= MAX_PENDING_TRANSFERS) {
                    closeTransport()
                    transfers.clear()
                    mutableState.value = mutableState.value.copy(
                        connection = ConnectionState.Disconnected,
                        pendingTransfers = 0,
                        lastError = "Too many pending terminal transfers",
                    )
                } else {
                    transfers += event.transferId
                    mutableState.value = mutableState.value.copy(pendingTransfers = transfers.size)
                }
            }
            is RemoteServerEvent.TransferFinished -> {
                transfers -= event.transferId
                mutableState.value = mutableState.value.copy(pendingTransfers = transfers.size)
            }
            is RemoteServerEvent.TerminalChunk -> acceptTerminalChunk(event.chunk)
            is RemoteServerEvent.Failure -> {
                val lostFocus = event.code == "terminal.input_not_owned"
                mutableState.value = mutableState.value.copy(
                    focus = if (lostFocus) FocusOwner.Other else mutableState.value.focus,
                    readOnly = if (lostFocus) true else mutableState.value.readOnly,
                    showTakeFocus = if (lostFocus) true else mutableState.value.showTakeFocus,
                    lastError = event.message,
                )
            }
            RemoteServerEvent.Revoked -> {
                closeTransport()
                transfers.clear()
                terminalAssembler.clear()
                recoveryRequested = false
                screenStore.clear()
                mutableState.value = RemoteClientState(connection = ConnectionState.Revoked)
            }
        }
    }

    private fun acceptTerminalChunk(chunk: TerminalTransferChunk) {
        when (val result = terminalAssembler.accept(chunk)) {
            TerminalTransferResult.Pending -> mutableState.value = mutableState.value.copy(pendingTransfers = 1)
            TerminalTransferResult.Recover -> requestRecovery(chunk.tabId, chunk.attachmentId)
            is TerminalTransferResult.Snapshot -> {
                try {
                    screenStore.replace(result.snapshot)
                    recoveryRequested = false
                    mutableState.value = mutableState.value.copy(pendingTransfers = 0)
                } catch (_: IllegalArgumentException) {
                    requestRecovery(chunk.tabId, result.attachmentId)
                }
            }
            is TerminalTransferResult.Diff -> {
                mutableState.value = mutableState.value.copy(pendingTransfers = 0)
                if (screenStore.apply(result.diff) == ApplyResult.NeedsSnapshot) {
                    requestRecovery(chunk.tabId, result.attachmentId)
                }
            }
            is TerminalTransferResult.Scrollback -> {
                mutableState.value = mutableState.value.copy(pendingTransfers = 0)
            }
        }
    }

    private fun requestRecovery(tabId: String, attachmentId: String?) {
        terminalAssembler.clear()
        mutableState.value = mutableState.value.copy(pendingTransfers = 0)
        val active = transport ?: return
        if (!isUnlocked() || attachmentId == null || recoveryRequested) return
        recoveryRequested = true
        val revision = screenStore.screen.value?.revision ?: 0
        val request = RemoteRequest(
            nextRequestId.getAndIncrement(),
            "terminal.resume",
            RemoteWireCodec.encodeTerminalResumePayload(tabId, attachmentId, revision),
        )
        scope.launch(dispatcher) {
            when (val response = active.request(request)) {
                is RemoteResponse.Error -> accept(RemoteServerEvent.Failure(response.code, response.message))
                is RemoteResponse.Success -> Unit
            }
        }
    }

    private fun closeTransport() {
        eventJob?.cancel()
        eventJob = null
        transport?.close()
        transport = null
        terminalAssembler.clear()
        recoveryRequested = false
    }

    private companion object {
        const val MAX_PENDING_TRANSFERS = 4
    }
}
