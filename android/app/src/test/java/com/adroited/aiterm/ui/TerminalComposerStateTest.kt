package com.adroited.aiterm.ui

import androidx.compose.ui.text.input.TextFieldValue
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalComposerStateTest {
    @Test
    fun textDraftSurvivesClosingTheOverlayUntilItIsSent() {
        val initial = TerminalComposerState()
        assertFalse(initial.expanded)
        assertFalse(initial.direct)

        val opened = initial.open()
        val drafted = opened.updateValue(TextFieldValue("hello phone"))

        assertEquals(emptyList<String>(), drafted.outbound)
        assertEquals("hello phone", drafted.state.visibleValue.text)

        val closed = drafted.state.close()
        assertFalse(closed.expanded)
        assertEquals("hello phone", closed.visibleValue.text)

        val sent = closed.open().sendText()
        assertEquals(listOf("hello phone", "\r"), sent.outbound)
        assertFalse(sent.state.expanded)
        assertEquals("", sent.state.visibleValue.text)
    }

    @Test
    fun emptyTextSubmissionStillSendsTheTerminalEnterAction() {
        val sent = TerminalComposerState().open().sendText()

        assertEquals(listOf("\r"), sent.outbound)
        assertFalse(sent.state.expanded)
    }

    @Test
    fun directModeSendsCommittedTextImmediately() {
        val direct = TerminalComposerState().open().toggleDirect()

        assertTrue(direct.direct)
        val typed = direct.updateValue(TextFieldValue("x\n"))

        assertEquals(listOf("x\r"), typed.outbound)
        assertEquals("", typed.state.visibleValue.text)
        assertTrue(typed.state.expanded)
    }
}
