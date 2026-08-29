package com.adroited.aiterm.remote

import com.adroited.aiterm.terminal.TerminalScreenStore
import com.adroited.aiterm.terminal.ApplyResult
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
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
    val tabs: List<RemoteTab> = emptyList(),
    val sessions: List<RemoteSession> = emptyList(),
    val activeTabId: String? = null,
    val activeTitle: String? = null,
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
    data class RosterChunk(val chunk: StateSnapshotChunk) : RemoteServerEvent
    data class Raw(val kind: String, val payload: ByteArray) : RemoteServerEvent
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
    val screen = screenStore.screen
    private val nextRequestId = AtomicLong(1)
    private val transfers = linkedSetOf<String>()
    private val terminalAssembler = TerminalTransferAssembler()
    private val rosterAssembler = RosterTransferAssembler()
    private var transport: RemoteTransport? = null
    private var eventJob: Job? = null
    private var reconnectJob: Job? = null
    private var recoveryRequested = false
    private var activeAttachmentId: String? = null

    suspend fun connect(): Boolean {
        reconnectJob?.cancel()
        reconnectJob = null
        return connectOnce(ConnectionState.Connecting)
    }

    private suspend fun connectOnce(connectingState: ConnectionState): Boolean {
        if (!isUnlocked()) {
            lock()
            return false
        }
        closeTransport()
        val candidate = transportFactory()
        transport = candidate
        mutableState.value = mutableState.value.copy(connection = connectingState)
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
        val tabId = mutableState.value.activeTabId ?: return false
        val attachmentId = activeAttachmentId ?: return false
        val data = text.encodeToByteArray()
        if (data.size > MAX_INPUT_BYTES) return false
        launchRequest("terminal.input", RemoteCommands.input(tabId, attachmentId, data))
        return true
    }

    fun selectTab(tabId: String) {
        if (tabId == mutableState.value.activeTabId && activeAttachmentId != null) return
        launchRequest("terminal.attach", RemoteCommands.tab(tabId)) { payload ->
            val attached = RemoteCommands.attached(payload)
            activeAttachmentId = attached.attachmentId
            mutableState.value = mutableState.value.copy(
                activeTabId = attached.tabId,
                activeTitle = attached.title,
                focus = if (attached.hasFocus) FocusOwner.Self else FocusOwner.Other,
                readOnly = !attached.hasFocus,
                showTakeFocus = !attached.hasFocus,
            )
        }
    }

    fun takeFocus(size: TerminalSize): Boolean {
        val tabId = mutableState.value.activeTabId ?: return false
        val attachmentId = activeAttachmentId ?: return false
        launchRequest("terminal.focus", RemoteCommands.sized(tabId, attachmentId, size))
        return true
    }

    fun resize(size: TerminalSize): Boolean {
        if (mutableState.value.focus != FocusOwner.Self) return false
        val tabId = mutableState.value.activeTabId ?: return false
        val attachmentId = activeAttachmentId ?: return false
        launchRequest("terminal.resize", RemoteCommands.sized(tabId, attachmentId, size))
        return true
    }

    fun requestScrollback(offset: Int, count: Int): Boolean {
        if (offset < 0 || count !in 1..512) return false
        val tabId = mutableState.value.activeTabId ?: return false
        val attachmentId = activeAttachmentId ?: return false
        launchRequest("terminal.scrollback", RemoteCommands.scrollback(tabId, attachmentId, offset, count))
        return true
    }

    fun refreshSessions() {
        launchRequest("session.list", byteArrayOf()) { payload ->
            mutableState.value = mutableState.value.copy(sessions = RemoteCommands.sessions(payload))
        }
    }

    fun openSession(sessionId: String, size: TerminalSize) {
        launchRequest("session.open", RemoteCommands.openSession(sessionId, size)) { payload ->
            selectTab(RemoteCommands.openedSessionTab(payload))
        }
    }

    fun deleteSession(sessionId: String) = sessionMutation("session.delete", sessionId)
    fun forkSession(sessionId: String) = sessionMutation("session.fork", sessionId)
    fun stopSession(sessionId: String) = sessionMutation("session.stop", sessionId)

    fun closeTab(tabId: String) {
        launchRequest("tab.close", RemoteCommands.tab(tabId))
    }

    fun openShell(projectPath: String?, size: TerminalSize) {
        launchRequest("tab.open", RemoteCommands.shell(projectPath, null, size)) { payload ->
            selectTab(RemoteCommands.openedTab(payload))
        }
    }

    private fun sessionMutation(kind: String, sessionId: String) {
        launchRequest(kind, RemoteCommands.session(sessionId)) { refreshSessions() }
    }

    fun lock() {
        reconnectJob?.cancel()
        reconnectJob = null
        closeTransport()
        transfers.clear()
        terminalAssembler.clear()
        rosterAssembler.clear()
        recoveryRequested = false
        activeAttachmentId = null
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
            is RemoteServerEvent.RosterChunk -> {
                val roster = try {
                    rosterAssembler.accept(event.chunk)
                } catch (error: RemoteProtocolException) {
                    mutableState.value = mutableState.value.copy(lastError = error.message)
                    null
                }
                if (roster != null) mutableState.value = mutableState.value.copy(tabs = roster.tabs)
            }
            is RemoteServerEvent.Raw -> acceptRaw(event)
            is RemoteServerEvent.Failure -> {
                val lostFocus = event.code == "terminal.input_not_owned"
                val disconnected = event.code == "transport.disconnected"
                mutableState.value = mutableState.value.copy(
                    connection = if (disconnected) ConnectionState.Reconnecting else mutableState.value.connection,
                    focus = if (lostFocus) FocusOwner.Other else mutableState.value.focus,
                    readOnly = if (lostFocus) true else mutableState.value.readOnly,
                    showTakeFocus = if (lostFocus) true else mutableState.value.showTakeFocus,
                    lastError = event.message,
                )
                if (disconnected) {
                    closeTransport()
                    scheduleReconnect()
                }
            }
            RemoteServerEvent.Revoked -> {
                reconnectJob?.cancel()
                reconnectJob = null
                closeTransport()
                transfers.clear()
                terminalAssembler.clear()
                rosterAssembler.clear()
                recoveryRequested = false
                activeAttachmentId = null
                screenStore.clear()
                mutableState.value = RemoteClientState(connection = ConnectionState.Revoked)
            }
        }
    }

    private fun acceptRaw(event: RemoteServerEvent.Raw) {
        when (event.kind) {
            "terminal.focus_changed" -> {
                val focus = RemoteCommands.focus(event.payload)
                if (focus.attachmentId == activeAttachmentId && focus.tabId == mutableState.value.activeTabId) {
                    accept(RemoteServerEvent.FocusChanged(focus.tabId, focus.attachmentId, focus.focus, focus.size))
                }
            }
            "session.changed" -> refreshSessions()
            else -> Unit
        }
    }

    private fun launchRequest(kind: String, payload: ByteArray, onSuccess: (ByteArray) -> Unit = {}) {
        val active = transport ?: return
        val request = RemoteRequest(nextRequestId.getAndIncrement(), kind, payload)
        scope.launch(dispatcher) {
            try {
                when (val response = active.request(request)) {
                    is RemoteResponse.Error -> accept(RemoteServerEvent.Failure(response.code, response.message))
                    is RemoteResponse.Success -> onSuccess(response.payload)
                }
            } catch (error: Exception) {
                accept(RemoteServerEvent.Failure("transport.disconnected", error.message ?: "Connection ended"))
            }
        }
    }

    private fun scheduleReconnect() {
        if (reconnectJob?.isActive == true || !isUnlocked()) return
        reconnectJob = scope.launch(dispatcher) {
            for (delayMillis in RECONNECT_DELAYS_MILLIS) {
                delay(delayMillis)
                if (!isUnlocked() || mutableState.value.connection == ConnectionState.Revoked ||
                    mutableState.value.connection == ConnectionState.Locked
                ) return@launch
                if (connectOnce(ConnectionState.Reconnecting)) return@launch
            }
            mutableState.value = mutableState.value.copy(connection = ConnectionState.Disconnected)
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
        rosterAssembler.clear()
        recoveryRequested = false
        activeAttachmentId = null
    }

    private companion object {
        const val MAX_PENDING_TRANSFERS = 4
        const val MAX_INPUT_BYTES = 64 * 1_024
        val RECONNECT_DELAYS_MILLIS = longArrayOf(1_000, 2_000, 4_000, 8_000, 16_000)
    }
}
