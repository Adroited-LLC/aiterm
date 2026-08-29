package com.adroited.aiterm.remote

import com.adroited.aiterm.terminal.DefaultTerminalScreenStore
import com.adroited.aiterm.terminal.CursorState
import com.adroited.aiterm.terminal.ScreenCell
import com.adroited.aiterm.terminal.ScreenRow
import com.adroited.aiterm.terminal.ScreenSnapshot
import kotlinx.coroutines.ExperimentalCoroutinesApi
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

@OptIn(ExperimentalCoroutinesApi::class)
class RemoteClientTest {

    @Test
    fun inputNotOwnedKeepsTerminalReadOnlyAndOffersTakeFocus() = runTest {
        val transport = FakeRemoteTransport()
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = DefaultTerminalScreenStore(),
            isUnlocked = { true },
            scope = backgroundScope,
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
    fun revisionMismatchKeepsTheCurrentScreenAndRequestsAuthoritativeRecovery() = runTest {
        val transport = FakeRemoteTransport()
        val store = DefaultTerminalScreenStore()
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
        val client = RemoteClient(
            transportFactory = { transport },
            screenStore = store,
            isUnlocked = { true },
            scope = this,
            dispatcher = StandardTestDispatcher(testScheduler),
        )
        client.connect()

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
        assertEquals(listOf("terminal.resume"), transport.requests.map(RemoteRequest::kind))
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
        val client = RemoteClient(
            transportFactory = { FakeRemoteTransport() },
            screenStore = store,
            isUnlocked = { true },
            scope = backgroundScope,
            dispatcher = StandardTestDispatcher(testScheduler),
        )

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
                    requestId = 3,
                    part = TerminalTransferPart.Scrollback(
                        listOf(ScreenRow("old".map { ScreenCell(it.toString()) })),
                    ),
                ),
            ),
        )

        assertEquals(listOf("old"), client.scrollback.value.map(ScreenRow::plainText))
    }
}

private class FakeRemoteTransport : RemoteTransport {
    override val events = MutableSharedFlow<RemoteServerEvent>(extraBufferCapacity = 8)
    val requests = mutableListOf<RemoteRequest>()
    var closed = false

    override suspend fun connect() = Unit

    override suspend fun request(request: RemoteRequest): RemoteResponse {
        requests += request
        return RemoteResponse.Success(request.requestId, request.kind, byteArrayOf())
    }

    override fun close() {
        closed = true
    }
}
