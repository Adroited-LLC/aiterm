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

        val opened = initial.open()
        val drafted = opened.updateValue(TextFieldValue("hello phone"))

        assertEquals(emptyList<String>(), drafted.outbound)
        assertEquals("hello phone", drafted.state.value.text)

        val closed = drafted.state.close()
        assertFalse(closed.expanded)
        assertEquals("hello phone", closed.value.text)

        val sent = closed.open().sendText()
        assertEquals(listOf("hello phone", "\r"), sent.outbound)
        assertFalse(sent.state.expanded)
        assertEquals("", sent.state.value.text)
    }

    @Test
    fun emptyTextSubmissionStillSendsTheTerminalEnterAction() {
        val sent = TerminalComposerState().open().sendText()

        assertEquals(listOf("\r"), sent.outbound)
        assertFalse(sent.state.expanded)
    }

    @Test
    fun composerHasOneAutocorrectableTextDraft() {
        val typed = TerminalComposerState().open()
            .updateValue(TextFieldValue("correct this"))

        assertEquals("correct this", typed.state.value.text)
        assertEquals(emptyList<String>(), typed.outbound)
        assertTrue(typed.state.expanded)
    }
}
