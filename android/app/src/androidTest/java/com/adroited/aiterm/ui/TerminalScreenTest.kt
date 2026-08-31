package com.adroited.aiterm.ui

import android.view.WindowInsets
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performImeAction
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.FocusOwner
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.remote.RemoteAgentChoice
import com.adroited.aiterm.remote.RemoteModelOption
import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.terminal.CursorState
import com.adroited.aiterm.terminal.ScreenCell
import com.adroited.aiterm.terminal.ScreenRow
import com.adroited.aiterm.terminal.ScreenSnapshot
import com.adroited.aiterm.terminal.TerminalModes
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.assertTrue
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class TerminalScreenTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()

    @Test
    fun terminalKeyBarCollapsesToARestoreStrip() {
        val expanded = mutableStateOf(true)
        compose.setContent {
            TerminalScreenContent(
                state = connectedState(),
                screen = oneCellScreen("tab-keys"),
                keyBarExpanded = expanded.value,
                onKeyBarExpandedChange = { expanded.value = it },
            )
        }

        compose.onNodeWithTag("collapse-extra-keys").performClick()
        assertTrue(compose.onAllNodesWithText("Esc").fetchSemanticsNodes().isEmpty())
        compose.onNodeWithTag("expand-extra-keys").assertIsDisplayed().performClick()
        compose.onNodeWithText("Esc").assertIsDisplayed()
    }

    @Test
    fun textComposerKeepsTheDraftVisibleUntilSend() {
        val sent = mutableListOf<String>()
        compose.setContent {
            TerminalScreenContent(
                state = RemoteClientState(
                    connection = ConnectionState.Connected,
                    focus = FocusOwner.Self,
                    activeTabId = "tab-compose",
                    activeTitle = "Prompt",
                ),
                screen = ScreenSnapshot(
                    tabId = "tab-compose",
                    revision = 1,
                    cols = 5,
                    rows = 1,
                    visible = listOf(ScreenRow("ready".map { ScreenCell(it.toString()) })),
                    cursor = CursorState(0, 0, true),
                ),
                onInput = sent::add,
            )
        }

        assertTrue(compose.onAllNodesWithTag("terminal-composer", useUnmergedTree = true).fetchSemanticsNodes().isEmpty())
        compose.onNodeWithText("Type").performClick()
        val composer = compose.onNodeWithTag("terminal-composer", useUnmergedTree = true)
        composer.assertIsDisplayed().performTextInput("hello phone")
        composer.assertTextEquals("hello phone")
        compose.runOnIdle { assertTrue(sent.isEmpty()) }
        assertTrue(compose.onAllNodesWithText("Send").fetchSemanticsNodes().isEmpty())

        composer.performImeAction()

        compose.runOnIdle { assertEquals(listOf("hello phone", "\r"), sent) }
        assertTrue(compose.onAllNodesWithTag("terminal-composer", useUnmergedTree = true).fetchSemanticsNodes().isEmpty())
        compose.onNodeWithText("Type").assertIsDisplayed()
    }

    @Test
    fun imeSubmitUsesBracketedPasteBeforeTheToolbarEnterAction() {
        val sent = mutableListOf<String>()
        compose.setContent {
            TerminalScreenContent(
                state = RemoteClientState(
                    connection = ConnectionState.Connected,
                    focus = FocusOwner.Self,
                    activeTabId = "tab-ime-submit",
                ),
                screen = ScreenSnapshot(
                    tabId = "tab-ime-submit",
                    revision = 1,
                    cols = 1,
                    rows = 1,
                    visible = listOf(ScreenRow(listOf(ScreenCell("$")))),
                    cursor = CursorState(0, 0, true),
                    modes = TerminalModes(bracketedPaste = true),
                ),
                onInput = sent::add,
            )
        }

        compose.onNodeWithText("Type").performClick()
        val composer = compose.onNodeWithTag("terminal-composer", useUnmergedTree = true)
        composer.performTextInput("hello phone")
        composer.performImeAction()

        compose.runOnIdle {
            assertEquals(listOf("\u001b[200~hello phone\u001b[201~", "\r"), sent)
        }
    }

    @Test
    fun composerFloatsOverTheTerminalWithoutCoveringItsRenderArea() {
        compose.setContent {
            TerminalScreenContent(
                state = RemoteClientState(
                    connection = ConnectionState.Connected,
                    focus = FocusOwner.Self,
                    activeTabId = "tab-overlay",
                ),
                screen = ScreenSnapshot(
                    tabId = "tab-overlay",
                    revision = 1,
                    cols = 1,
                    rows = 1,
                    visible = listOf(ScreenRow(listOf(ScreenCell("$")))),
                    cursor = CursorState(0, 0, true),
                ),
            )
        }

        compose.onNodeWithText("Type").performClick()
        compose.waitForIdle()

        val render = compose.onNodeWithTag("terminal-render-content", useUnmergedTree = true)
            .fetchSemanticsNode().boundsInRoot
        val overlay = compose.onNodeWithTag("terminal-composer-overlay", useUnmergedTree = true)
            .fetchSemanticsNode().boundsInRoot
        val field = compose.onNodeWithTag("terminal-composer", useUnmergedTree = true)
            .fetchSemanticsNode().boundsInRoot
        val placeholder = compose.onNodeWithText("Type a command or prompt…", useUnmergedTree = true)
            .fetchSemanticsNode().boundsInRoot
        val maxSingleRowHeight = 60f * compose.activity.resources.displayMetrics.density

        assertTrue(
            "composer input must remain a compact single row",
            field.height <= maxSingleRowHeight,
        )
        assertTrue("terminal content must end above the composer", render.bottom <= overlay.top + 1f)
        assertTrue(
            "placeholder must be vertically centered in the input",
            kotlin.math.abs(field.center.y - placeholder.center.y) < 2f,
        )

        assertTrue(
            compose.onAllNodesWithTag("input-mode-direct", useUnmergedTree = true)
                .fetchSemanticsNodes().isEmpty(),
        )
        assertTrue(kotlin.math.abs(field.center.y - placeholder.center.y) < 2f)
    }

    @Test
    fun textComposerStaysAboveTheSoftwareKeyboard() {
        compose.setContent {
            TerminalScreenContent(
                state = RemoteClientState(
                    connection = ConnectionState.Connected,
                    focus = FocusOwner.Self,
                    activeTabId = "tab-ime",
                ),
                screen = ScreenSnapshot(
                    tabId = "tab-ime",
                    revision = 1,
                    cols = 1,
                    rows = 1,
                    visible = listOf(ScreenRow(listOf(ScreenCell("$")))),
                    cursor = CursorState(0, 0, true),
                ),
            )
        }

        compose.onNodeWithText("Type").performClick()
        val composer = compose.onNodeWithTag("terminal-composer", useUnmergedTree = true)
        composer.performClick().performTextInput("visible")
        compose.waitUntil(5_000) {
            compose.activity.window.decorView.rootWindowInsets
                ?.isVisible(WindowInsets.Type.ime()) == true
        }

        val composerBottom = composer.fetchSemanticsNode().boundsInRoot.bottom
        val decor = compose.activity.window.decorView
        val keyboardTop = decor.height - decor.rootWindowInsets
            .getInsets(WindowInsets.Type.ime()).bottom
        assertTrue(
            "composer bottom $composerBottom must be above keyboard top $keyboardTop",
            composerBottom <= keyboardTop + 1f,
        )
    }

    @Test
    fun sessionsDrawerOmitsTheAgentLauncherForTheRemoteClient() {
        compose.setContent {
            TerminalScreenContent(
                state = RemoteClientState(
                    connection = ConnectionState.Connected,
                    sessions = listOf(
                        RemoteSession(
                            id = "session-1",
                            agent = "codex",
                            title = "AITerm",
                            projectPath = "/projects/aiterm",
                            groupPath = "/projects/aiterm",
                            forked = false,
                            background = false,
                            lastActive = 1,
                        ),
                    ),
                    agents = listOf(
                        RemoteAgentChoice(
                            id = "codex",
                            displayName = "Codex",
                            models = listOf(
                                RemoteModelOption(
                                    id = "gpt-5",
                                    displayName = "GPT-5",
                                    efforts = listOf("high"),
                                ),
                            ),
                            mintsSessionId = true,
                        ),
                    ),
                ),
                screen = null,
            )
        }

        compose.onNodeWithText("Sessions").performClick()
        compose.waitForIdle()

        compose.onNodeWithText("LIVE TABS").assertIsDisplayed()
        compose.onNodeWithText("SESSIONS").assertIsDisplayed()
        assertTrue(compose.onAllNodesWithText("NEW AGENT").fetchSemanticsNodes().isEmpty())
        assertTrue(compose.onAllNodesWithText("Start Codex · GPT-5 · high").fetchSemanticsNodes().isEmpty())
    }

    @Test
    fun nativeGridRemainsVisibleWhileReadOnlyAndOffersFocusAndExtraKeys() {
        var focusRequested = false
        compose.setContent {
            TerminalScreenContent(
                state = RemoteClientState(
                    connection = ConnectionState.Connected,
                    focus = FocusOwner.Other,
                    readOnly = true,
                    showTakeFocus = true,
                    activeTabId = "tab-1",
                    activeTitle = "Storm shell",
                ),
                screen = ScreenSnapshot(
                    tabId = "tab-1",
                    revision = 1,
                    cols = 5,
                    rows = 1,
                    visible = listOf(ScreenRow("hello".map { ScreenCell(it.toString()) })),
                    cursor = CursorState(0, 0, true),
                ),
                onTakeFocus = { _, _ -> focusRequested = true },
            )
        }

        compose.onNodeWithTag("terminal-grid").assertIsDisplayed()
        compose.onNodeWithText("hello").assertIsDisplayed()
        compose.onAllNodesWithText("CONNECTED").onFirst().assertIsDisplayed()
        compose.onNodeWithText("Esc").assertIsDisplayed()
        compose.onNodeWithText("Take Focus").performClick()

        assertTrue(focusRequested)
    }

    @Test
    fun portraitToLandscapeConstraintsKeepTheScreenAndReportANewCanonicalViewport() {
        val sizes = mutableListOf<Pair<Int, Int>>()
        val landscape = mutableStateOf(false)
        compose.setContent {
            Box(Modifier.size(if (landscape.value) 800.dp else 400.dp, if (landscape.value) 400.dp else 800.dp)) {
                TerminalScreenContent(
                    state = RemoteClientState(connection = ConnectionState.Connected),
                    screen = ScreenSnapshot(
                        tabId = "tab-rotation",
                        revision = 8,
                        cols = 6,
                        rows = 1,
                        visible = listOf(ScreenRow("rotate".map { ScreenCell(it.toString()) })),
                        cursor = CursorState(0, 0, true),
                    ),
                    onResize = { cols, rows -> sizes += cols to rows },
                )
            }
        }
        compose.waitUntil(5_000) { sizes.isNotEmpty() }
        val initial = sizes.last()

        compose.runOnIdle { landscape.value = true }
        compose.waitUntil(8_000) { sizes.any { it != initial } }

        compose.onNodeWithText("rotate").assertIsDisplayed()
        assertTrue(sizes.any { it != initial })
    }

    @Test
    fun resizeStormPublishesOnlyTheFinalStableViewport() {
        val sizes = mutableListOf<Pair<Int, Int>>()
        val height = mutableStateOf(480.dp)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            Box(Modifier.size(400.dp, height.value)) {
                TerminalScreenContent(
                    state = RemoteClientState(connection = ConnectionState.Connected),
                    screen = ScreenSnapshot(
                        tabId = "tab-resize-storm",
                        revision = 1,
                        cols = 1,
                        rows = 1,
                        visible = listOf(ScreenRow(listOf(ScreenCell("M")))),
                        cursor = CursorState(0, 0, true),
                    ),
                    onResize = { cols, rows -> sizes += cols to rows },
                )
            }
        }
        compose.mainClock.advanceTimeByFrame()
        compose.runOnIdle { sizes.clear() }

        repeat(10) { index ->
            compose.runOnIdle { height.value = (480 + (index + 1) * 24).dp }
            compose.mainClock.advanceTimeBy(10)
        }

        compose.runOnIdle { assertTrue(sizes.isEmpty()) }
        compose.mainClock.advanceTimeBy(TERMINAL_RESIZE_SETTLE_MILLIS)
        compose.runOnIdle { assertEquals(1, sizes.size) }
    }

    @Test
    fun measuredGridKeepsWideCombiningAndCursorOnTheSameFontScaledGeometry() {
        compose.setContent {
            val density = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(density.density, 1.6f)) {
                TerminalScreenContent(
                    state = RemoteClientState(connection = ConnectionState.Connected),
                    screen = ScreenSnapshot(
                        tabId = "tab-geometry",
                        revision = 1,
                        cols = 3,
                        rows = 1,
                        visible = listOf(
                            ScreenRow(
                                listOf(
                                    ScreenCell("界", width = 2),
                                    ScreenCell("", continuation = true),
                                    ScreenCell("e\u0301"),
                                ),
                            ),
                        ),
                        cursor = CursorState(2, 0, true),
                    ),
                )
            }
        }

        val wide = compose.onNodeWithTag("terminal-cell-0-0", useUnmergedTree = true)
            .fetchSemanticsNode().boundsInRoot
        val combining = compose.onNodeWithTag("terminal-cell-0-2", useUnmergedTree = true)
            .fetchSemanticsNode().boundsInRoot
        val cursor = compose.onNodeWithTag("terminal-cursor", useUnmergedTree = true)
            .fetchSemanticsNode().boundsInRoot
        assertTrue(kotlin.math.abs(wide.width - combining.width * 2f) < 2f)
        assertTrue(kotlin.math.abs(cursor.left - combining.left) < 2f)
        assertTrue(kotlin.math.abs(cursor.height - combining.height) < 2f)
        compose.onNodeWithText("界é").assertIsDisplayed()
    }

    @Test
    fun advertisedViewportUsesTheFontScaledPaddedRenderBoundsAcrossRotation() {
        val sizes = mutableListOf<Pair<Int, Int>>()
        val dimensions = mutableStateOf(400.dp to 800.dp)
        compose.setContent {
            val density = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(density.density, 1.6f)) {
                Box(
                    Modifier.size(dimensions.value.first, dimensions.value.second),
                ) {
                    TerminalScreenContent(
                        state = RemoteClientState(connection = ConnectionState.Connected),
                        screen = ScreenSnapshot(
                            tabId = "tab-render-bounds",
                            revision = 1,
                            cols = 1,
                            rows = 1,
                            visible = listOf(ScreenRow(listOf(ScreenCell("M")))),
                            cursor = CursorState(0, 0, true),
                        ),
                        onResize = { cols, rows -> sizes += cols to rows },
                    )
                }
            }
        }

        fun assertLatestViewportMatchesGrid() {
            val advertised = sizes.last()
            val grid = compose.onNodeWithTag("terminal-render-content", useUnmergedTree = true)
                .fetchSemanticsNode().boundsInRoot
            val cell = compose.onNodeWithTag("terminal-cell-0-0", useUnmergedTree = true)
                .fetchSemanticsNode().boundsInRoot
            val row = compose.onNodeWithTag("terminal-row", useUnmergedTree = true)
                .fetchSemanticsNode().boundsInRoot
            assertEquals(
                "advertised columns must come from the padded grid width",
                (grid.width / cell.width).toInt().coerceIn(1, 512),
                advertised.first,
            )
            assertEquals(
                "advertised rows must come from the padded grid height",
                (grid.height / row.height).toInt().coerceIn(1, 512),
                advertised.second,
            )
            assertTrue(advertised.first * cell.width <= grid.width + 1f)
            assertTrue(advertised.second * row.height <= grid.height + 1f)
        }

        compose.waitUntil(5_000) { sizes.isNotEmpty() }
        val callbacksBeforeResizeStorm = sizes.size
        for (width in 380..410) {
            compose.runOnIdle { dimensions.value = width.dp to 800.dp }
            compose.waitForIdle()
        }
        compose.waitUntil(8_000) { sizes.size > callbacksBeforeResizeStorm }
        assertLatestViewportMatchesGrid()
        val portrait = sizes.last()
        compose.runOnIdle { dimensions.value = 800.dp to 400.dp }
        compose.waitUntil(8_000) { sizes.lastOrNull() != portrait }
        assertLatestViewportMatchesGrid()
    }

    @Test
    fun largeScrollbackComposesOnlyTheBoundedVisibleRowWindow() {
        val history = List(5_000) { index ->
            ScreenRow("history-$index".map { ScreenCell(it.toString()) })
        }
        compose.setContent {
            Box(Modifier.size(400.dp, 800.dp)) {
                TerminalScreenContent(
                    state = RemoteClientState(connection = ConnectionState.Connected),
                    screen = ScreenSnapshot(
                        tabId = "tab-history",
                        revision = 1,
                        cols = 4,
                        rows = 1,
                        visible = listOf(ScreenRow("live".map { ScreenCell(it.toString()) })),
                        cursor = CursorState(0, 0, true),
                    ),
                    scrollback = history,
                )
            }
        }

        compose.onNodeWithTag("terminal-grid").assertIsDisplayed()
        val composedRows = compose.onAllNodesWithTag("terminal-row", useUnmergedTree = true)
            .fetchSemanticsNodes().size
        assertTrue(composedRows > 0)
        assertTrue("composed $composedRows rows", composedRows < 100)
        assertEquals(5_000, history.size)
    }

    private fun connectedState() = RemoteClientState(
        connection = ConnectionState.Connected,
        focus = FocusOwner.Self,
    )

    private fun oneCellScreen(tabId: String) = ScreenSnapshot(
        tabId = tabId,
        revision = 1,
        cols = 1,
        rows = 1,
        visible = listOf(ScreenRow(listOf(ScreenCell("$")))),
        cursor = CursorState(0, 0, true),
    )
}
