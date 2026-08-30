package com.adroited.aiterm.remote

import com.adroited.aiterm.terminal.DefaultTerminalScreenStore
import com.adroited.aiterm.terminal.CursorState
import com.adroited.aiterm.terminal.ScreenCell
import com.adroited.aiterm.terminal.ScreenRow
import com.adroited.aiterm.terminal.ScreenSnapshot
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

@OptIn(ExperimentalCoroutinesApi::class)
class RemoteClientTest {

    @Test
    fun inputNotOwnedKeepsTerminalReadOnlyAndOffersTakeFocus() = runTest {
        val transport = FakeRemoteTransport()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.acceptForTest(
            RemoteServerEvent.FocusChanged(
                tabId = "tab-1",
                attachmentId = "attachment-1",
                focus = FocusOwner.Other,
                size = TerminalSize(80, 24),
            ),
        )

        val sent = client.sendInput("whoami")
        advanceUntilIdle()

        assertFalse(sent)
        assertTrue(client.state.value.showTakeFocus)
        assertTrue(client.state.value.readOnly)
        assertEquals(emptyList<RemoteRequest>(), transport.requests)
    }

    @Test
    fun lockCancelsPendingRequestsTransfersAndConnection() = runTest {
        val transport = FakeRemoteTransport()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()
        client.acceptForTest(RemoteServerEvent.TransferStarted("transfer-1"))
        client.lock()

        assertEquals(ConnectionState.Locked, client.state.value.connection)
        assertEquals(0, client.state.value.pendingTransfers)
        assertTrue(transport.closed)
    }

    @Test
    fun lockDuringConnectCannotPublishTheLateConnection() = runTest {
        val transport = DeferredRemoteTransport(connectImmediately = false)
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )

        val connecting = async { client.connect() }
        runCurrent()
        assertEquals(ConnectionState.Connecting, client.state.value.connection)

        client.lock()
        transport.allowConnect.complete(Unit)
        advanceUntilIdle()

        assertFalse(connecting.await())
        assertEquals(ConnectionState.Locked, client.state.value.connection)
        assertTrue(transport.closed)
    }

    @Test
    fun explicitAuthenticationRevocationStopsReconnectAndPurgesState() = runTest {
        val transport = object : RemoteTransport {
            override val events = MutableSharedFlow<RemoteServerEvent>()
            override suspend fun connect() = throw RemoteAccessRevokedException()
            override fun request(kind: String, payload: ByteArray, onAssigned: (Long) -> Unit) =
                CompletableDeferred<RemoteResponse>().also { it.completeExceptionally(IllegalStateException("not connected")) }
            override fun close() = Unit
        }
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )

        assertFalse(client.connect())
        assertEquals(ConnectionState.Revoked, client.state.value.connection)
        advanceTimeBy(32_000)
        runCurrent()
        assertEquals(ConnectionState.Revoked, client.state.value.connection)
    }

    @Test
    fun rapidTabSelectionDetachesStaleAttachmentAndRejectsItsChunks() = runTest {
        val transport = DeferredRemoteTransport()
        val store = DefaultTerminalScreenStore()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = store,
            isUnlocked = { true },
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()

        client.selectTab("tab-a")
        runCurrent()
        client.selectTab("tab-b")
        runCurrent()
        assertEquals(1, transport.pendingAttachCount())

        transport.completeNextAttach("tab-a", "attachment-a")
        runCurrent()
        assertEquals(listOf("terminal.attach", "terminal.detach", "terminal.attach"), transport.requests.map { it.kind })

        transport.completeNextAttach("tab-b", "attachment-b")
        runCurrent()
        assertEquals("tab-b", client.state.value.activeTabId)

        client.acceptForTest(snapshotChunk("old", "tab-a", "attachment-a", "WRONG"))
        assertEquals(null, store.screen.value)
        client.acceptForTest(snapshotChunk("current", "tab-b", "attachment-b", "RIGHT"))
        assertEquals("RIGHT", store.screen.value?.visible?.single()?.plainText())
        client.lock()
    }

    @Test
    fun disconnectClosesTransportOutsideTheClientLifecycleLock() = runTest {
        lateinit var client: RemoteClient
        val closeObservedUnlockedClient = AtomicBoolean(false)
        val transport = FakeRemoteTransport(
            onClose = {
                val probe = thread(start = true) { client.requestNextScrollbackPage() }
                probe.join(1_000)
                closeObservedUnlockedClient.set(!probe.isAlive)
            },
        )
        client = RemoteClient(
            transportFactory = { transport },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()

        client.acceptForTest(RemoteServerEvent.Failure("transport.disconnected", "lost"))

        assertTrue("transport close ran while lifecycleLock was held", closeObservedUnlockedClient.get())
        client.lock()
    }

    @Test
    fun selectingANewTabAtomicallyRejectsOldAttachmentDamage() = runTest {
        val transport = DeferredRemoteTransport()
        val store = DefaultTerminalScreenStore()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = store,
            isUnlocked = { true },
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()
        client.selectTab("tab-a")
        runCurrent()
        transport.completeNextAttach("tab-a", "attachment-a")
        runCurrent()

        client.selectTab("tab-b")
        client.acceptForTest(snapshotChunk("late-a", "tab-a", "attachment-a", "WRONG"))
        assertEquals(null, store.screen.value)

        runCurrent()
        transport.completeNextAttach("tab-b", "attachment-b")
        runCurrent()
        client.acceptForTest(snapshotChunk("current-b", "tab-b", "attachment-b", "RIGHT"))
        assertEquals("RIGHT", store.screen.value?.visible?.single()?.plainText())
        client.lock()
    }

    @Test
    fun supersededSelectionsStillDetachTheCapturedOldAttachment() = runTest {
        val transport = DeferredRemoteTransport()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()
        client.selectTab("tab-a")
        runCurrent()
        transport.completeNextAttach("tab-a", "attachment-a")
        runCurrent()

        client.selectTab("tab-b")
        client.selectTab("tab-c")
        runCurrent()

        assertEquals(
            listOf("terminal.attach", "terminal.detach", "terminal.attach"),
            transport.requests.map(RemoteRequest::kind),
        )
        transport.completeNextAttach("tab-c", "attachment-c")
        runCurrent()
        assertEquals("tab-c", client.state.value.activeTabId)
        client.lock()
    }

    @Test
    fun revisionMismatchKeepsTheCurrentScreenAndRequestsAuthoritativeRecovery() = runTest {
        val transport = FakeRemoteTransport()
        val store = DefaultTerminalScreenStore()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = store,
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()
        client.selectTab("tab-1")
        advanceUntilIdle()
        store.replace(
            ScreenSnapshot(
                tabId = "tab-1",
                revision = 5,
                cols = 1,
                rows = 1,
                visible = listOf(ScreenRow(listOf(ScreenCell("old")))),
                cursor = CursorState(0, 0, true),
            ),
        )
        client.acceptForTest(
            RemoteServerEvent.TerminalChunk(
                TerminalTransferChunk(
                    transferId = "transfer-1",
                    tabId = "tab-1",
                    attachmentId = "attachment-1",
                    kind = TerminalTransferKind.Diff,
                    baseRevision = 4,
                    finalRevision = 6,
                    rowStart = 0,
                    rowEnd = 1,
                    index = 0,
                    total = 1,
                    requestId = 0,
                    part = TerminalTransferPart.Diff(
                        patches = listOf(com.adroited.aiterm.terminal.RowPatch(0, ScreenRow(listOf(ScreenCell("new"))))),
                        cursor = null,
                        modes = null,
                    ),
                ),
            ),
        )
        advanceUntilIdle()

        assertEquals("old", store.screen.value?.visible?.single()?.plainText())
        assertEquals("terminal.resume", transport.requests.last().kind)
        client.lock()
    }

    @Test
    fun authenticatedDisconnectReconnectsWithoutChangingTransportSecurityPolicy() = runTest {
        val transports = mutableListOf<FakeRemoteTransport>()
        val client = RemoteClient(
            transportFactory = { FakeRemoteTransport().also(transports::add) },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()

        client.acceptForTest(RemoteServerEvent.Failure("transport.disconnected", "storm"))
        assertEquals(ConnectionState.Reconnecting, client.state.value.connection)
        advanceTimeBy(1_000)
        runCurrent()

        assertEquals(2, transports.size)
        assertEquals(ConnectionState.Connected, client.state.value.connection)
        client.lock()
    }

    @Test
    fun completeScrollbackPageIsPublishedOnlyForTheVisibleTab() = runTest {
        val store = DefaultTerminalScreenStore()
        store.replace(
            ScreenSnapshot(
                tabId = "tab-1",
                revision = 5,
                cols = 4,
                rows = 1,
                visible = listOf(ScreenRow(listOf(ScreenCell("live")))),
                cursor = CursorState(0, 0, true),
            ),
        )
        val transport = FakeRemoteTransport()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = store,
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()
        client.selectTab("tab-1")
        advanceUntilIdle()
        store.replace(
            ScreenSnapshot(
                tabId = "tab-1",
                revision = 5,
                cols = 4,
                rows = 1,
                visible = listOf(ScreenRow(listOf(ScreenCell("live")))),
                cursor = CursorState(0, 0, true),
            ),
        )

        assertTrue(client.requestNextScrollbackPage(128))
        val requestId = transport.requests.last { it.kind == "terminal.scrollback" }.requestId
        client.acceptForTest(
            RemoteServerEvent.TerminalChunk(
                TerminalTransferChunk(
                    transferId = "history-1",
                    tabId = "tab-1",
                    attachmentId = "attachment-1",
                    kind = TerminalTransferKind.Scrollback,
                    baseRevision = 5,
                    finalRevision = 5,
                    rowStart = 0,
                    rowEnd = 1,
                    index = 0,
                    total = 1,
                    requestId = requestId,
                    part = TerminalTransferPart.Scrollback(
                        listOf(ScreenRow("old".map { ScreenCell(it.toString()) })),
                    ),
                ),
            ),
        )

        assertEquals(listOf("old"), client.scrollback.value.map(ScreenRow::plainText))
        client.lock()
    }

    @Test
    fun rapidScrollbackPagingKeepsOnlyOneRequestForTheExpectedOffset() = runTest {
        val transport = FakeRemoteTransport()
        val store = DefaultTerminalScreenStore()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = store,
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()
        client.selectTab("tab-1")
        advanceUntilIdle()

        assertTrue(client.requestNextScrollbackPage(128))
        assertFalse(client.requestNextScrollbackPage(128))
        assertEquals(1, transport.requests.count { it.kind == "terminal.scrollback" })
        client.lock()
    }

    @Test
    fun selectingBDiscardsAPagingTransactionAndAllowsBPaging() = runTest {
        val transport = DeferredRemoteTransport()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()
        client.selectTab("tab-a")
        runCurrent()
        transport.completeNextAttach("tab-a", "attachment-a")
        advanceUntilIdle()
        assertTrue(client.requestNextScrollbackPage(128))
        val oldRequestId = transport.requests.last { it.kind == "terminal.scrollback" }.requestId

        client.selectTab("tab-b")
        runCurrent()
        transport.completeNextAttach("tab-b", "attachment-b")
        advanceUntilIdle()
        client.acceptForTest(scrollbackChunk(oldRequestId, "stale-a", "tab-a", "attachment-a"))

        assertEquals(emptyList<ScreenRow>(), client.scrollback.value)
        assertTrue(client.requestNextScrollbackPage(128))
        assertEquals(2, transport.requests.count { it.kind == "terminal.scrollback" })
        client.lock()
    }

    @Test
    fun unexpectedScrollbackCorrelationCannotPublishOutOfOrderRows() = runTest {
        val transport = FakeRemoteTransport()
        val store = DefaultTerminalScreenStore()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = store,
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()
        client.selectTab("tab-1")
        advanceUntilIdle()
        store.replace(
            ScreenSnapshot(
                tabId = "tab-1",
                revision = 1,
                cols = 1,
                rows = 1,
                visible = listOf(ScreenRow(listOf(ScreenCell("x")))),
                cursor = CursorState(0, 0, true),
            ),
        )
        assertTrue(client.requestNextScrollbackPage(128))
        val expectedId = transport.requests.last { it.kind == "terminal.scrollback" }.requestId

        client.acceptForTest(scrollbackChunk(expectedId + 1, "later"))
        assertEquals(emptyList<ScreenRow>(), client.scrollback.value)
        client.acceptForTest(scrollbackChunk(expectedId, "expected"))
        assertEquals(listOf("expected"), client.scrollback.value.map(ScreenRow::plainText))
        client.lock()
    }
}

private fun snapshotChunk(
    transferId: String,
    tabId: String,
    attachmentId: String,
    text: String,
) = RemoteServerEvent.TerminalChunk(
    TerminalTransferChunk(
        transferId = transferId,
        tabId = tabId,
        attachmentId = attachmentId,
        kind = TerminalTransferKind.Snapshot,
        baseRevision = 1,
        finalRevision = 1,
        rowStart = 0,
        rowEnd = 1,
        index = 0,
        total = 1,
        requestId = 0,
        part = TerminalTransferPart.Snapshot(
            cols = text.length,
            rows = 1,
            visible = listOf(ScreenRow(text.map { ScreenCell(it.toString()) })),
            cursor = CursorState(0, 0, true),
            modes = com.adroited.aiterm.terminal.TerminalModes(),
        ),
    ),
)

private fun scrollbackChunk(
    requestId: Long,
    text: String,
    tabId: String = "tab-1",
    attachmentId: String = "attachment-1",
) = RemoteServerEvent.TerminalChunk(
    TerminalTransferChunk(
        transferId = "history-$requestId",
        tabId = tabId,
        attachmentId = attachmentId,
        kind = TerminalTransferKind.Scrollback,
        baseRevision = 1,
        finalRevision = 1,
        rowStart = 0,
        rowEnd = 1,
        index = 0,
        total = 1,
        requestId = requestId,
        part = TerminalTransferPart.Scrollback(
            listOf(ScreenRow(text.map { ScreenCell(it.toString()) })),
        ),
    ),
)

private class FakeRemoteTransport(private val onClose: () -> Unit = {}) : RemoteTransport {
    override val events = MutableSharedFlow<RemoteServerEvent>(extraBufferCapacity = 8)
    val requests = mutableListOf<RemoteRequest>()
    var closed = false

    override suspend fun connect() = Unit

    private var nextRequestId = 1L

    override fun request(
        kind: String,
        payload: ByteArray,
        onAssigned: (Long) -> Unit,
    ): CompletableDeferred<RemoteResponse> {
        val request = RemoteRequest(nextRequestId++, kind, payload)
        onAssigned(request.requestId)
        requests += request
        val responsePayload = if (request.kind == "terminal.attach") {
            attachedPayload("tab-1", "attachment-1")
        } else {
            byteArrayOf()
        }
        return CompletableDeferred(RemoteResponse.Success(request.requestId, request.kind, responsePayload))
    }

    override fun close() {
        onClose()
        closed = true
    }
}

private class DeferredRemoteTransport(connectImmediately: Boolean = true) : RemoteTransport {
    override val events = MutableSharedFlow<RemoteServerEvent>(extraBufferCapacity = 8)
    val requests = mutableListOf<RemoteRequest>()
    val allowConnect = CompletableDeferred<Unit>().also { if (connectImmediately) it.complete(Unit) }
    private val attaches = ArrayDeque<Pair<RemoteRequest, CompletableDeferred<RemoteResponse>>>()
    private var nextRequestId = 1L
    var closed = false

    override suspend fun connect() {
        allowConnect.await()
    }

    override fun request(
        kind: String,
        payload: ByteArray,
        onAssigned: (Long) -> Unit,
    ): CompletableDeferred<RemoteResponse> {
        val request = RemoteRequest(nextRequestId++, kind, payload)
        onAssigned(request.requestId)
        requests += request
        if (request.kind != "terminal.attach") {
            return CompletableDeferred(RemoteResponse.Success(request.requestId, request.kind, byteArrayOf()))
        }
        val response = CompletableDeferred<RemoteResponse>()
        attaches += request to response
        return response
    }

    fun pendingAttachCount(): Int = attaches.size

    fun completeNextAttach(tabId: String, attachmentId: String) {
        val (request, response) = attaches.removeFirst()
        response.complete(RemoteResponse.Success(request.requestId, request.kind, attachedPayload(tabId, attachmentId)))
    }

    override fun close() {
        closed = true
    }

}

private fun attachedPayload(tabId: String, attachmentId: String): ByteArray {
    fun text(value: String): String {
        val bytes = value.encodeToByteArray()
        require(bytes.size < 24)
        return (0x60 + bytes.size).toString(16).padStart(2, '0') + bytes.joinToString("") {
            it.toUByte().toString(16).padStart(2, '0')
        }
    }
    val encoded = "a4" + text("tab_id") + text(tabId) +
        text("attachment_id") + text(attachmentId) +
        text("has_focus") + "f4" + text("title") + text(tabId)
    return encoded.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}
