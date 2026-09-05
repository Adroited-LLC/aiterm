package com.adroited.aiterm.ui

import android.accessibilityservice.AccessibilityServiceInfo
import android.graphics.Bitmap
import android.graphics.Rect
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.provider.Settings
import android.view.WindowInsets
import android.view.accessibility.AccessibilityNodeInfo
import android.view.inputmethod.InputMethodManager
import androidx.compose.material3.OutlinedTextField
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.FocusOwner
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.terminal.CursorState
import com.adroited.aiterm.terminal.ScreenCell
import com.adroited.aiterm.terminal.ScreenRow
import com.adroited.aiterm.terminal.ScreenSnapshot
import com.adroited.aiterm.testing.ComposeTestActivity
import com.adroited.aiterm.ui.theme.AitermTheme
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Exercises the real system IME and InputConnection, rather than injecting Compose key events. */
@RunWith(AndroidJUnit4::class)
class ConsoleKeyboardTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val automation get() = instrumentation.uiAutomation
    private val consoleIme = "com.adroited.aiterm/.keyboard.ConsoleKeyboardService"

    @Test
    fun consoleKeysReachTerminalWithoutEditingDraftAndGlobeRestoresNormalTyping() = withKeyboard {
        val sent = mutableListOf<String>()
        compose.setContent {
            AitermTheme {
                TerminalScreenContent(
                    state = RemoteClientState(
                        connection = ConnectionState.Connected,
                        focus = FocusOwner.Self,
                    ),
                    screen = ScreenSnapshot(
                        tabId = "console-ime-test", revision = 1, cols = 1, rows = 1,
                        visible = listOf(ScreenRow(listOf(ScreenCell("$")))),
                        cursor = CursorState(0, 0, true),
                    ),
                    onInput = { sent.add(it) },
                )
            }
        }
        val composer = compose.onNodeWithTag("terminal-composer", useUnmergedTree = true)
        composer.performClick().performTextInput("unsent draft")
        shell("ime set $consoleIme")
        compose.waitUntil(10_000) { defaultIme() == consoleIme && findKey("Ctrl") != null }

        fun send(label: String, expected: String) {
            val oldCount = sent.size
            clickKey(label)
            compose.waitUntil(5_000) { sent.size > oldCount }
            compose.runOnIdle {
                assertEquals(oldCount + 1, sent.size)
                assertEquals(expected, sent.last())
            }
        }
        send("a", "a")
        clickKey("Ctrl")
        send("c", "\u0003")
        send("Tab", "\t")
        send("Up arrow", "\u001b[A")
        send("Enter", "\r")
        // Accessibility nodes can arrive before the IME's entrance animation has finished.
        SystemClock.sleep(500)
        automation.takeScreenshot()?.let { screenshot ->
            File(instrumentation.targetContext.cacheDir, "console-keyboard-test.png")
                .outputStream().use { screenshot.compress(Bitmap.CompressFormat.PNG, 100, it) }
            screenshot.recycle()
        }
        val decor = compose.activity.window.decorView
        val decorLocation = IntArray(2)
        decor.getLocationOnScreen(decorLocation)
        val insets = requireNotNull(decor.rootWindowInsets)
        val screenBottom = decor.height + decorLocation[1]
        val keyboardTop = screenBottom - insets.getInsets(WindowInsets.Type.ime()).bottom
        val composerBottom = composer.fetchSemanticsNode().boundsInWindow.bottom + decorLocation[1]
        val gap = keyboardTop - composerBottom
        val maxGap = 20 * compose.activity.resources.displayMetrics.density
        assertTrue("Terminal composer overlaps keyboard or floats above it: gap=$gap", gap >= -1 && gap <= maxGap)
        val spaceBounds = Rect()
        requireNotNull(findKey("space")).getBoundsInScreen(spaceBounds)
        val navigationTop = screenBottom - insets.getInsets(WindowInsets.Type.navigationBars()).bottom
        assertTrue("Bottom console keys overlap system navigation", spaceBounds.bottom <= navigationTop)
        clickKey("#+=")
        send("|", "|")
        send("~", "~")
        send("'", "'")
        composer.assertTextContains("unsent draft")

        clickKey("Switch keyboard")
        compose.waitUntil(10_000) { defaultIme() != consoleIme }
        // The ordinary keyboard's text-editing path must remain separate from console events.
        composer.performTextInput(" more")
        composer.assertTextContains("unsent draft more")
        compose.runOnIdle { assertEquals(8, sent.size) }
    }

    @Test
    fun consoleKeyboardRefusesUnmarkedEditorsAndOffersSwitching() = withKeyboard {
        val value = mutableStateOf("")
        compose.setContent {
            OutlinedTextField(
                value = value.value,
                onValueChange = { value.value = it },
                modifier = Modifier.testTag("ordinary-editor"),
            )
        }
        compose.onNodeWithTag("ordinary-editor").performClick()
        shell("ime set $consoleIme")
        compose.waitUntil(10_000) {
            findKey("Open an aiterm terminal to use this keyboard.") != null
        }
        assertTrue(findKey("Ctrl") == null)
        clickKey("Switch keyboard")
        compose.waitUntil(10_000) { defaultIme() != consoleIme }
        compose.runOnIdle { assertEquals("", value.value) }
    }

    private fun withKeyboard(block: () -> Unit) {
        val previous = defaultIme()
        val originalFlags = automation.serviceInfo.flags
        try {
            automation.serviceInfo = automation.serviceInfo.apply {
                flags = flags or AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS
            }
            val normal = previous.takeIf {
                it.isNotBlank() && it != consoleIme && it != "null"
            } ?: instrumentation.targetContext.getSystemService(InputMethodManager::class.java)
                .enabledInputMethodList.firstOrNull { info ->
                    info.id != consoleIme && (0 until info.subtypeCount).any { index ->
                        val subtype = info.getSubtypeAt(index)
                        !subtype.isAuxiliary && subtype.mode == "keyboard"
                    }
                }?.id
            requireNotNull(normal) { "A normal keyboard must be enabled for the globe round-trip test" }
            shell("ime enable $consoleIme")
            shell("ime set $normal")
            compose.waitUntil(10_000) { defaultIme() == normal }
            block()
        } finally {
            if (previous.isNotBlank() && previous != "null") shell("ime set $previous")
            automation.serviceInfo = automation.serviceInfo.apply { flags = originalFlags }
        }
    }

    private fun defaultIme(): String = Settings.Secure.getString(
        instrumentation.targetContext.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD,
    ).orEmpty()

    private fun shell(command: String): String = ParcelFileDescriptor.AutoCloseInputStream(
        automation.executeShellCommand(command),
    ).bufferedReader().use { it.readText() }

    private fun clickKey(label: String) {
        // One-shot modifiers redraw the native key views. A cached accessibility node can
        // disappear between discovery and ACTION_CLICK; only retry rejected actions.
        compose.waitUntil(10_000) {
            findKey(label)?.let { node ->
                node.refresh() && node.performAction(AccessibilityNodeInfo.ACTION_CLICK)
            } == true
        }
        instrumentation.waitForIdleSync()
    }

    private fun findKey(label: String): AccessibilityNodeInfo? {
        fun find(node: AccessibilityNodeInfo): AccessibilityNodeInfo? {
            if (node.contentDescription?.toString() == label || node.text?.toString() == label) {
                return node
            }
            for (index in 0 until node.childCount) {
                val child = node.getChild(index) ?: continue
                find(child)?.let { return it }
            }
            return null
        }
        return automation.windows.asSequence()
            .filter { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_INPUT_METHOD }
            .mapNotNull { it.root?.let(::find) }
            .firstOrNull()
    }
}
