package com.adroited.aiterm.ui

import com.adroited.aiterm.terminal.*
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.*
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ConversationInputReadinessTest {
    private fun screen(text: String, paste: Boolean = true, tab: String = "a") = ScreenSnapshot(
        tab, 1, 80, 24, text.lines().map { ScreenRow(listOf(ScreenCell(it))) },
        cursor = CursorState(2, 2, true), modes = TerminalModes(bracketedPaste = paste),
    )

    @Test fun reopenedCodexWaitsThroughBlankPtyAndProvisionalComposer() = runTest {
        val screens = MutableStateFlow<ScreenSnapshot?>(screen("", paste = false))
        val ready = async { awaitConversationInputScreen(screens, "a", "codex") }
        runCurrent()
        assertFalse(ready.isCompleted)
        screens.value = screen("│ model:     loading   /model to change │\n› Ask Codex to do anything")
        runCurrent()
        assertFalse(ready.isCompleted)
        screens.value = screen("│ directory: loading │\n› Ask Codex to do anything")
        runCurrent()
        assertFalse(ready.isCompleted)
        screens.value = screen("› Ask Codex to do anything", paste = false)
        runCurrent()
        assertFalse(ready.isCompleted)
        screens.value = screen("› Ask Codex to do anything")
        runCurrent()
        assertEquals(screens.value, ready.await())
        assertTrue(formatTerminalSubmission("hello\nworld", emptyList(), ready.await()!!.modes.bracketedPaste)
            .first().startsWith("\u001b[200~"))
    }

    @Test fun anotherTabDoesNotSupplyReadinessAndStartupTimeoutDoesNotSendAnything() = runTest {
        val screens = MutableStateFlow<ScreenSnapshot?>(screen("› Ready", tab = "other"))
        val ready = async { awaitConversationInputScreen(screens, "a", "codex") }
        advanceUntilIdle()
        assertNull(ready.await())
        assertEquals(10_000L, testScheduler.currentTime)
    }

    @Test fun readyTerminalDoesNotAddADelayOrBlockOtherAgentsWithoutPasteMode() = runTest {
        val ready = screen("› existing draft")
        assertEquals(ready, awaitConversationInputScreen(MutableStateFlow(ready), "a", "codex"))
        assertEquals(0L, testScheduler.currentTime)
        assertTrue(screen("other CLI prompt", paste = false).readyForConversationInput("other"))
        assertTrue(screen("The model is loading data\n› next").readyForConversationInput("codex"))
    }
}
