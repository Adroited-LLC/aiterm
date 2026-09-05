package com.adroited.aiterm.remote

import com.adroited.aiterm.terminal.DefaultTerminalScreenStore
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.cbor.Cbor
import org.junit.Assert.*
import org.junit.Test

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class, kotlinx.serialization.ExperimentalSerializationApi::class)
class RemoteTerminalOpeningTest {
    @Test
    fun mainTerminalCreatesShellAndConversationTerminalOpensThatSession() = runTest {
        for (sessionId in listOf(null, "session-work")) {
            val transport = OpeningTransport()
            val client = RemoteClient({ transport }, DefaultTerminalScreenStore(), { true }, backgroundScope,
                StandardTestDispatcher(testScheduler))
            client.connect()
            val opening = async { client.openTerminalTarget(sessionId, TerminalSize(80, 24)) }
            runCurrent()
            assertNull(client.state.value.activeTabId)
            val request = transport.requests.single()
            assertEquals(if (sessionId == null) "tab.open" else "session.open", request.first)
            assertArrayEquals(
                if (sessionId == null) RemoteCommands.shell(null, null, TerminalSize(80, 24))
                else RemoteCommands.openSession(sessionId, TerminalSize(80, 24)), request.second,
            )
            transport.opening.complete(RemoteResponse.Success(1, request.first,
                if (sessionId == null) Cbor.encodeToByteArray(Opened.serializer(), Opened("requested-tab"))
                else Cbor.encodeToByteArray(SessionOpened.serializer(), SessionOpened("requested-tab", true))))
            assertEquals("requested-tab", opening.await())
            assertEquals("requested-tab", client.state.value.activeTabId)
            client.lock()
        }
    }

    @Test
    fun openingRequiresConnectionAndReportsServerFailureWithoutSelectingAnotherTab() = runTest {
        val transport = OpeningTransport()
        val client = RemoteClient({ transport }, DefaultTerminalScreenStore(), { true }, backgroundScope,
            StandardTestDispatcher(testScheduler))
        assertTrue(runCatching { client.openTerminalTarget(null, TerminalSize(80, 24)) }.isFailure)
        assertTrue(transport.requests.isEmpty())
        client.connect()
        val opening = async { runCatching { client.openTerminalTarget("missing", TerminalSize(80, 24)) } }
        runCurrent()
        transport.opening.complete(RemoteResponse.Error(1, "session.not_found", "Session no longer exists"))
        assertEquals("Session no longer exists", opening.await().exceptionOrNull()?.message)
        assertNull(client.state.value.activeTabId)
        assertEquals(ConnectionState.Connected, client.state.value.connection)
        client.lock()
    }

    @Test
    fun cancellingAnOpenAbandonsItsRequest() = runTest {
        val transport = OpeningTransport()
        val client = RemoteClient({ transport }, DefaultTerminalScreenStore(), { true }, backgroundScope,
            StandardTestDispatcher(testScheduler))
        client.connect()
        val opening = async { client.openTerminalTarget(null, TerminalSize(80, 24)) }
        runCurrent()
        opening.cancelAndJoin()
        assertTrue(transport.abandoned)
        assertNull(client.state.value.activeTabId)
        client.lock()
    }

    @Test
    fun lateOpenAfterLockCannotSelectATerminal() = runTest {
        val transport = OpeningTransport()
        val client = RemoteClient({ transport }, DefaultTerminalScreenStore(), { true }, backgroundScope,
            StandardTestDispatcher(testScheduler))
        client.connect()
        val opening = async { runCatching { client.openTerminalTarget(null, TerminalSize(80, 24)) } }
        runCurrent()
        client.lock()
        transport.opening.complete(RemoteResponse.Success(1, "tab.open",
            Cbor.encodeToByteArray(Opened.serializer(), Opened("late-tab"))))
        assertTrue(opening.await().isFailure)
        assertNull(client.state.value.activeTabId)
        assertEquals(ConnectionState.Locked, client.state.value.connection)
    }
}

@Serializable
private data class Opened(@SerialName("tab_id") val tabId: String)

@Serializable
private data class SessionOpened(@SerialName("tab_id") val tabId: String,
    @SerialName("selected_existing") val selectedExisting: Boolean)

private class OpeningTransport : RemoteTransport {
    override val events = MutableSharedFlow<RemoteServerEvent>()
    val requests = mutableListOf<Pair<String, ByteArray>>()
    val opening = CompletableDeferred<RemoteResponse>()
    var abandoned = false
    override suspend fun connect() = Unit
    override fun close() = Unit
    override fun request(kind: String, payload: ByteArray, onAssigned: (Long) -> Unit): Deferred<RemoteResponse> {
        requests += kind to payload
        onAssigned(requests.size.toLong())
        return if (kind == "tab.open" || kind == "session.open") opening else CompletableDeferred()
    }
    override fun requestBatch(requests: List<RemoteRequestInput>): List<Deferred<RemoteResponse>>? = null
    override fun abandonRequest(request: Deferred<RemoteResponse>) { abandoned = true }
}
