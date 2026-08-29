package com.adroited.aiterm.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalRenderingTest {
    @Test
    fun indexedTerminalColorsCoverAnsiCubeAndGrayscale() {
        assertEquals(Color(0xFF07111B), terminalIndexedColor(0))
        assertEquals(Color(0xFF000000), terminalIndexedColor(16))
        assertEquals(Color(0xFF0000FF), terminalIndexedColor(21))
        assertEquals(Color(0xFFFFFFFF), terminalIndexedColor(231))
        assertEquals(Color(0xFF080808), terminalIndexedColor(232))
        assertEquals(Color(0xFFEEEEEE), terminalIndexedColor(255))
    }

    @Test
    fun hardwareTerminalKeysUseDeleteAndApplicationCursorSequences() {
        assertEquals("\u007f", terminalKeySequence(Key.Backspace, false))
        assertEquals("\r", terminalKeySequence(Key.Enter, false))
        assertEquals("\u001b[A", terminalKeySequence(Key.DirectionUp, false))
        assertEquals("\u001bOA", terminalKeySequence(Key.DirectionUp, true))
    }
}
