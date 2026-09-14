package com.adroited.aiterm.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.ToolCategory
import com.adroited.aiterm.remote.ToolStatus
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AgentQuestionsDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()

    @Test fun questionIsVisibleWithoutOpeningActivityAndAnswerRequiresAnExplicitTap() {
        val question = Item.Tool("q", "request_user_input_async", "Tool", ToolCategory.Other,
            """{"questions":[{"title":"Which configuration should we use?","options":["Keep current settings","Use the new configuration"]}]}""",
            ToolStatus.Completed, "{\"accepted\":true}", 0)
        var opened = 0
        compose.setContent {
            MaterialTheme {
                CompositionLocalProvider(LocalQuestionAction provides { opened++ }) {
                    Column(Modifier.verticalScroll(rememberScrollState())) {
                        spineTimeline(listOf(question)).forEach { SpineTimelineRow(it) }
                    }
                }
            }
        }
        compose.onNodeWithText("Which configuration should we use?").assertIsDisplayed()
        compose.onNodeWithText("• Keep current settings").assertIsDisplayed()
        compose.runOnIdle { assertEquals(0, opened) }
        compose.onNodeWithText("Answer in terminal").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(1, opened) }
    }

    @Test fun questionControlsOnlySendTheKeysExplicitlyChosen() {
        val keys = mutableListOf<String>()
        var closed = false
        compose.setContent {
            MaterialTheme {
                QuestionTerminalControls(true, true, false, keys::add, { closed = true })
            }
        }
        compose.runOnIdle { assertTrue(keys.isEmpty()) }
        compose.onNodeWithText("Open queued questions").performClick()
        compose.onNodeWithText("↓").performClick()
        compose.onNodeWithText("Enter").performClick()
        compose.onNodeWithText("Hide").performClick()
        compose.runOnIdle {
            assertEquals(listOf("\u001b[1;3A", "\u001b[B", "\r"), keys)
            assertTrue(closed)
        }
    }

    @Test fun readOnlyControlsCannotSubmitAnything() {
        val keys = mutableListOf<String>()
        compose.setContent {
            MaterialTheme { QuestionTerminalControls(false, true, false, keys::add, {}) }
        }
        compose.onNodeWithText("Enter").assertIsNotEnabled().performClick()
        compose.onNodeWithText("Open queued questions").assertIsNotEnabled().performClick()
        compose.runOnIdle { assertTrue(keys.isEmpty()) }
    }
}
