package com.adroited.aiterm.keyboard

import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.inputmethodservice.InputMethodService
import android.os.Build
import android.os.SystemClock
import android.view.Gravity
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import com.adroited.aiterm.R

/** A separate, system-selectable IME. Console events never pass through a composing buffer. */
class ConsoleKeyboardService : InputMethodService() {
    private var keyboard: LinearLayout? = null
    private var shift = false
    private var ctrl = false
    private var alt = false
    private var symbols = false
    private var navigationBarBottom = 0

    override fun onEvaluateFullscreenMode(): Boolean = false

    override fun onCreateInputView(): View = LinearLayout(this).also {
        keyboard = it
        it.orientation = LinearLayout.VERTICAL
        it.setOnApplyWindowInsetsListener { view, insets ->
            navigationBarBottom = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                insets.getInsets(WindowInsets.Type.navigationBars()).bottom
            } else {
                @Suppress("DEPRECATION")
                insets.systemWindowInsetBottom
            }
            view.setPadding(dp(4), dp(6), dp(4), dp(6) + navigationBarBottom)
            insets
        }
        renderKeyboard()
    }

    override fun onStartInput(attribute: EditorInfo?, restarting: Boolean) {
        super.onStartInput(attribute, restarting)
        shift = false
        ctrl = false
        alt = false
        symbols = false
        renderKeyboard()
    }

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        renderKeyboard()
        keyboard?.requestApplyInsets()
    }

    private fun acceptsConsoleInput(): Boolean = currentInputEditorInfo?.let {
        it.packageName == packageName &&
            it.privateImeOptions?.split(',')?.contains(CONSOLE_INPUT_OPTION) == true
    } == true

    private fun renderKeyboard() {
        val root = keyboard ?: return
        root.removeAllViews()
        val dark = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK ==
            Configuration.UI_MODE_NIGHT_YES
        val foreground = if (dark) Color.rgb(232, 235, 238) else Color.rgb(29, 34, 40)
        val backgroundColor = if (dark) Color.rgb(28, 31, 35) else Color.rgb(225, 230, 236)
        val keyBackground = if (dark) Color.rgb(49, 54, 60) else Color.rgb(249, 251, 253)
        val selectedBackground = if (dark) Color.rgb(58, 100, 90) else Color.rgb(173, 220, 204)
        root.setBackgroundColor(backgroundColor)
        root.setPadding(dp(4), dp(6), dp(4), dp(6) + navigationBarBottom)
        updateNavigationBarAppearance(dark)

        fun row(vararg keys: ConsoleKey) {
            val row = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
            root.addView(row, LinearLayout.LayoutParams(-1, dp(44)))
            keys.forEach { key ->
                val button = Button(this).apply {
                    text = key.label
                    contentDescription = key.description ?: key.label
                    isAllCaps = false
                    textSize = if (key.label.length > 3) 12f else 16f
                    typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
                    setTextColor(foreground)
                    minWidth = 0
                    minimumWidth = 0
                    minHeight = 0
                    minimumHeight = 0
                    setPadding(0, 0, 0, 0)
                    gravity = Gravity.CENTER
                    background = GradientDrawable().apply {
                        cornerRadius = dp(9).toFloat()
                        setColor(if (key.selected) selectedBackground else keyBackground)
                    }
                    setOnClickListener { key.action() }
                    if (key.description == getString(R.string.console_switch_keyboard)) {
                        setOnLongClickListener { inputMethodManager.showInputMethodPicker(); true }
                    }
                }
                row.addView(button, LinearLayout.LayoutParams(0, -1, key.weight).apply {
                    setMargins(dp(2), dp(2), dp(2), dp(2))
                })
            }
        }

        val globe = ConsoleKey("🌐", description = getString(R.string.console_switch_keyboard)) {
            switchKeyboard()
        }
        if (!acceptsConsoleInput()) {
            root.addView(TextView(this).apply {
                text = getString(R.string.console_terminal_only)
                setTextColor(foreground)
                textSize = 16f
                gravity = Gravity.CENTER
                setPadding(dp(24), dp(28), dp(24), dp(28))
            })
            row(ConsoleKey(getString(R.string.console_switch_keyboard)) { switchKeyboard() })
            return
        }

        fun special(label: String, code: Int, description: String = label) =
            ConsoleKey(label, description = description) { sendConsoleKey(code) }
        fun character(char: Char) = ConsoleKey(
            if (shift) char.uppercaseChar().toString() else char.toString(),
        ) { sendCharacter(if (shift) char.uppercaseChar() else char) }
        fun characters(value: String) = value.map(::character).toTypedArray()

        row(
            special("Esc", KeyEvent.KEYCODE_ESCAPE),
            special("Tab", KeyEvent.KEYCODE_TAB),
            ConsoleKey("Ctrl", selected = ctrl) { ctrl = !ctrl; renderKeyboard() },
            ConsoleKey("Alt", selected = alt) { alt = !alt; renderKeyboard() },
            special("↑", KeyEvent.KEYCODE_DPAD_UP, "Up arrow"),
            special("Del", KeyEvent.KEYCODE_FORWARD_DEL, "Delete forward"),
        )
        row(
            special("Home", KeyEvent.KEYCODE_MOVE_HOME),
            special("End", KeyEvent.KEYCODE_MOVE_END),
            special("PgUp", KeyEvent.KEYCODE_PAGE_UP, "Page up"),
            special("PgDn", KeyEvent.KEYCODE_PAGE_DOWN, "Page down"),
            special("←", KeyEvent.KEYCODE_DPAD_LEFT, "Left arrow"),
            special("↓", KeyEvent.KEYCODE_DPAD_DOWN, "Down arrow"),
            special("→", KeyEvent.KEYCODE_DPAD_RIGHT, "Right arrow"),
        )
        row(*characters(if (shift && !symbols) "!@#$%^&*()" else "1234567890"))
        row(*characters(if (symbols) "!@#$%^&*()" else "qwertyuiop"))
        row(*characters(if (symbols) "[]{}<>/\\|" else "asdfghjkl"))
        row(
            ConsoleKey("⇧", selected = shift, description = "Shift") {
                shift = !shift; renderKeyboard()
            },
            *characters(if (symbols) "`~'\";:_," else "zxcvbnm"),
            special("⌫", KeyEvent.KEYCODE_DEL, "Backspace"),
        )
        row(
            ConsoleKey(if (symbols) "ABC" else "#+=") { symbols = !symbols; renderKeyboard() },
            globe,
            character(if (symbols) '+' else '/'),
            ConsoleKey("space", weight = 3f) { sendConsoleKey(KeyEvent.KEYCODE_SPACE) },
            character(if (symbols) '=' else '-'),
            character(if (symbols) '?' else '.'),
            special("↵", KeyEvent.KEYCODE_ENTER, "Enter"),
        )
    }

    private fun sendCharacter(char: Char) {
        // Android's virtual US keymap supplies the correct keycode and Shift state for ASCII.
        val events = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD)
            .getEvents(charArrayOf(char)) ?: return
        val event = events.firstOrNull {
            it.action == KeyEvent.ACTION_DOWN && !KeyEvent.isModifierKey(it.keyCode)
        } ?: return
        sendConsoleKey(event.keyCode, event.metaState)
    }

    private fun sendConsoleKey(code: Int, characterMeta: Int = 0) {
        if (!acceptsConsoleInput()) return
        val connection = currentInputConnection ?: return
        val meta = KeyEvent.normalizeMetaState(characterMeta or
            (if (shift) KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON else 0) or
            (if (ctrl) KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON else 0) or
            (if (alt) KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON else 0))
        val now = SystemClock.uptimeMillis()
        for (action in intArrayOf(KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP)) {
            connection.sendKeyEvent(KeyEvent(
                now, now, action, code, 0, meta, KeyCharacterMap.VIRTUAL_KEYBOARD, 0,
                KeyEvent.FLAG_SOFT_KEYBOARD or KeyEvent.FLAG_KEEP_TOUCH_MODE,
            ))
        }
        if (shift || ctrl || alt) {
            shift = false
            ctrl = false
            alt = false
            renderKeyboard()
        }
    }

    private val inputMethodManager: InputMethodManager
        get() = getSystemService(InputMethodManager::class.java)

    @Suppress("DEPRECATION")
    private fun updateNavigationBarAppearance(dark: Boolean) {
        val imeWindow = window?.window ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            imeWindow.insetsController?.setSystemBarsAppearance(
                if (dark) 0 else WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
                WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
            )
        } else {
            val decor = imeWindow.decorView
            decor.systemUiVisibility = if (dark) {
                decor.systemUiVisibility and View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR.inv()
            } else {
                decor.systemUiVisibility or View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun switchKeyboard() {
        val switched = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            switchToNextInputMethod(false)
        } else {
            window?.window?.attributes?.token?.let {
                inputMethodManager.switchToNextInputMethod(it, false)
            } ?: false
        }
        if (!switched) inputMethodManager.showInputMethodPicker()
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private data class ConsoleKey(
        val label: String,
        val weight: Float = 1f,
        val selected: Boolean = false,
        val description: String? = null,
        val action: () -> Unit,
    )

    companion object {
        const val CONSOLE_INPUT_OPTION = "com.adroited.aiterm.CONSOLE_INPUT"
    }
}
