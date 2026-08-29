package com.adroited.aiterm.ui

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
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
        compose.onNodeWithText("CONNECTED").assertIsDisplayed()
        compose.onNodeWithText("Esc").assertIsDisplayed()
        compose.onNodeWithText("Take Focus").performClick()

        assertTrue(focusRequested)
    }
}
