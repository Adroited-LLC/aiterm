package com.adroited.aiterm.ui

import android.provider.Settings
import android.view.KeyCharacterMap
import android.view.KeyEvent
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.platform.LocalContext

/** Ordinary IMEs edit the draft; the console IME and physical keyboards drive the PTY. */
internal fun Modifier.terminalConsoleInput(
    enabled: Boolean,
    applicationCursor: Boolean,
    onInput: (String) -> Unit,
): Modifier = composed {
    val context = LocalContext.current
    onPreviewKeyEvent { event ->
        val native = event.nativeKeyEvent
        val consoleIme = Settings.Secure.getString(
            context.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD,
        )?.let { it == "${context.packageName}/.keyboard.ConsoleKeyboardService" ||
            it == "${context.packageName}/${context.packageName}.keyboard.ConsoleKeyboardService" } == true
        if (!consoleIme && native.deviceId == KeyCharacterMap.VIRTUAL_KEYBOARD) {
            return@onPreviewKeyEvent false
        }
        val text = encodeTerminalKey(
            keyCode = native.keyCode,
            unicode = native.getUnicodeChar(native.metaState and
                (KeyEvent.META_CTRL_MASK or KeyEvent.META_ALT_MASK).inv()),
            control = native.isCtrlPressed,
            alt = native.isAltPressed,
            shift = native.isShiftPressed,
            applicationCursor = applicationCursor,
        ) ?: return@onPreviewKeyEvent false
        if (enabled && native.action == KeyEvent.ACTION_DOWN) onInput(text)
        true
    }
}

internal fun encodeTerminalKey(
    keyCode: Int,
    unicode: Int = 0,
    control: Boolean = false,
    alt: Boolean = false,
    shift: Boolean = false,
    applicationCursor: Boolean = false,
): String? {
    val modifier = 1 + (if (shift) 1 else 0) + (if (alt) 2 else 0) + (if (control) 4 else 0)
    fun cursor(code: Char): String = when {
        modifier > 1 -> "\u001b[1;${modifier}$code"
        applicationCursor -> "\u001bO$code"
        else -> "\u001b[$code"
    }
    fun tilde(code: Int) = if (modifier > 1) "\u001b[$code;${modifier}~" else "\u001b[$code~"
    when (keyCode) {
        KeyEvent.KEYCODE_DPAD_UP -> return cursor('A')
        KeyEvent.KEYCODE_DPAD_DOWN -> return cursor('B')
        KeyEvent.KEYCODE_DPAD_RIGHT -> return cursor('C')
        KeyEvent.KEYCODE_DPAD_LEFT -> return cursor('D')
        KeyEvent.KEYCODE_MOVE_HOME -> return cursor('H')
        KeyEvent.KEYCODE_MOVE_END -> return cursor('F')
        KeyEvent.KEYCODE_PAGE_UP -> return tilde(5)
        KeyEvent.KEYCODE_PAGE_DOWN -> return tilde(6)
        KeyEvent.KEYCODE_INSERT -> return tilde(2)
        KeyEvent.KEYCODE_FORWARD_DEL -> return tilde(3)
    }
    val text = when (keyCode) {
        KeyEvent.KEYCODE_ESCAPE -> "\u001b"
        KeyEvent.KEYCODE_TAB -> if (shift) "\u001b[Z" else "\t"
        KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> if (shift) "\u001b[13;2u" else "\r"
        KeyEvent.KEYCODE_DEL -> if (control) "\b" else "\u007f"
        else -> {
            if (control) {
                val code = when {
                    keyCode in KeyEvent.KEYCODE_A..KeyEvent.KEYCODE_Z -> keyCode - KeyEvent.KEYCODE_A + 1
                    keyCode == KeyEvent.KEYCODE_SPACE || keyCode == KeyEvent.KEYCODE_2 -> 0
                    keyCode == KeyEvent.KEYCODE_6 -> 30
                    keyCode == KeyEvent.KEYCODE_MINUS -> 31
                    keyCode == KeyEvent.KEYCODE_8 -> 127
                    unicode in 64..95 -> unicode and 31
                    unicode in 97..122 -> unicode and 31
                    else -> return null
                }
                code.toChar().toString()
            } else {
                if (unicode <= 0 || !Character.isValidCodePoint(unicode) ||
                    unicode and KeyCharacterMap.COMBINING_ACCENT != 0) return null
                String(Character.toChars(unicode))
            }
        }
    }
    return if (alt) "\u001b$text" else text
}
