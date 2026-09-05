package com.adroited.aiterm.ui

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TerminalConsoleInputTest {
    @Test fun consoleKeysProduceTerminalBytes() {
        assertEquals("x", encodeTerminalKey(KeyEvent.KEYCODE_X, 'x'.code))
        assertEquals("\u0003", encodeTerminalKey(KeyEvent.KEYCODE_C, control = true))
        assertEquals("\u0000", encodeTerminalKey(KeyEvent.KEYCODE_SPACE, control = true))
        assertEquals("\u001bx", encodeTerminalKey(KeyEvent.KEYCODE_X, 'x'.code, alt = true))
        assertEquals("\u001b\u0003", encodeTerminalKey(KeyEvent.KEYCODE_C, control = true, alt = true))
        assertEquals("\u001b", encodeTerminalKey(KeyEvent.KEYCODE_ESCAPE))
        assertEquals("\r", encodeTerminalKey(KeyEvent.KEYCODE_ENTER))
        assertEquals("\u007f", encodeTerminalKey(KeyEvent.KEYCODE_DEL))
        assertEquals("\t", encodeTerminalKey(KeyEvent.KEYCODE_TAB))
        assertEquals("\u001b[Z", encodeTerminalKey(KeyEvent.KEYCODE_TAB, shift = true))
    }

    @Test fun cursorKeysRespectApplicationModeAndModifiers() {
        assertEquals("\u001b[A", encodeTerminalKey(KeyEvent.KEYCODE_DPAD_UP))
        assertEquals("\u001bOA", encodeTerminalKey(KeyEvent.KEYCODE_DPAD_UP, applicationCursor = true))
        assertEquals("\u001b[1;5D", encodeTerminalKey(KeyEvent.KEYCODE_DPAD_LEFT, control = true))
        assertEquals("\u001b[1;3C", encodeTerminalKey(KeyEvent.KEYCODE_DPAD_RIGHT, alt = true))
        assertEquals("\u001b[5~", encodeTerminalKey(KeyEvent.KEYCODE_PAGE_UP))
        assertEquals("\u001b[3;2~", encodeTerminalKey(KeyEvent.KEYCODE_FORWARD_DEL, shift = true))
    }

    @Test fun systemAndModifierKeysAreNotSentAsText() {
        assertNull(encodeTerminalKey(KeyEvent.KEYCODE_BACK))
        assertNull(encodeTerminalKey(KeyEvent.KEYCODE_SHIFT_LEFT))
        assertNull(encodeTerminalKey(KeyEvent.KEYCODE_CTRL_LEFT))
        assertNull(encodeTerminalKey(KeyEvent.KEYCODE_UNKNOWN, -1))
    }
}
