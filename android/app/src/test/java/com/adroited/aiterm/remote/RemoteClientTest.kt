package com.adroited.aiterm.remote

import com.adroited.aiterm.terminal.DefaultTerminalScreenStore
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
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
