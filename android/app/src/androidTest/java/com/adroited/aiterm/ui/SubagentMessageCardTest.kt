package com.adroited.aiterm.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.longClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SubagentMessageCardTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()

    private fun show(payload: String, onLongPress: (SpineTimelineItem) -> Unit = {}): SpineTimelineItem.Row {
        val text = "/root/performance → /root\nMessage Type: MESSAGE\nTask name: /root\nSender: /root/performance\nPayload:\n$payload"
        val row = SpineTimelineItem.Row(Item.AgentText("subagent-update", text, true, 0))
        compose.setContent { MaterialTheme { SpineTimelineRow(row, onLongPress) } }
        return row
    }

    @Test
    fun updateStartsCollapsedAndTogglesPayloadWithoutRoutingMetadata() {
        show("The rendering issue is fixed.")
        compose.onNodeWithText("performance · Update").assertIsDisplayed()
        compose.onNodeWithText("The rendering issue is fixed.").assertDoesNotExist()
        compose.onNodeWithText("performance · Update").performClick()
        compose.onNodeWithText("The rendering issue is fixed.").assertIsDisplayed()
        compose.onNodeWithText("Message Type:", substring = true).assertDoesNotExist()
        compose.onNodeWithText("Sender:", substring = true).assertDoesNotExist()
        compose.onNodeWithText("performance · Update").performClick()
        compose.onNodeWithText("The rendering issue is fixed.").assertDoesNotExist()
    }

    @Test
    fun emptyUpdateIsCompactWithoutAnExpansionAction() {
        show(" \n")
        compose.onNodeWithText("performance · Update").assertIsDisplayed()
        compose.onNode(hasText("performance · Update") and hasClickAction()).assertDoesNotExist()
    }

    @Test
    fun deliberateHoldKeepsMessageActionsAvailable() {
        var held: SpineTimelineItem? = null
        val row = show("Review complete.") { held = it }
        compose.onNodeWithText("performance · Update").performTouchInput {
            longClick(durationMillis = MESSAGE_ACTION_HOLD_MILLIS + 100)
        }
        compose.runOnIdle { assertEquals(row, held) }
    }
}
