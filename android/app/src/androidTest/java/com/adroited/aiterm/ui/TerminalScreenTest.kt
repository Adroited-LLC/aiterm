package com.adroited.aiterm.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.FocusOwner
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.terminal.CursorState
import com.adroited.aiterm.terminal.ScreenCell
import com.adroited.aiterm.terminal.ScreenRow
import com.adroited.aiterm.terminal.ScreenSnapshot
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
        for (width in 380..410) {
            compose.runOnIdle { dimensions.value = width.dp to 800.dp }
            compose.waitForIdle()
            assertLatestViewportMatchesGrid()
        }
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
}
