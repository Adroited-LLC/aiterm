package com.adroited.aiterm.ui

import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.ToolCategory
import com.adroited.aiterm.remote.ToolStatus
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SpineActivityGroupTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()

    @Test
    fun mixedActivityStartsAsOneRowAndExpandsInOrderWithReachableDetails() {
        val conversation = listOf(
            Item.AgentText("before", "Checking the implementation.", true, 0),
            tool("read", "Read source", "Source contents"),
            update("scout", "The source review is complete."),
            tool("test", "Run tests", "All tests passed"),
            update("builder", "The patch is ready."),
            Item.AgentText("after", "The change is complete.", true, 0),
        )
        compose.setContent {
            MaterialTheme {
                LazyColumn {
                    items(spineTimeline(conversation), key = { it.key }) { SpineTimelineRow(it) }
                }
            }
        }

        compose.onAllNodesWithText("Activity ·", substring = true).assertCountEquals(1)
        compose.onNodeWithText("Activity · 4 steps").assertIsDisplayed().assert(collapsed)
        compose.onNodeWithText("Checking the implementation.").assertIsDisplayed()
        compose.onNodeWithText("The change is complete.").assertIsDisplayed()
        compose.onNodeWithText("Read source").assertDoesNotExist()
        compose.onNodeWithText("scout · Update").assertDoesNotExist()
        compose.onNodeWithText("Run tests").assertDoesNotExist()
        compose.onNodeWithText("builder · Update").assertDoesNotExist()

        compose.onNodeWithText("Activity · 4 steps").performClick().assert(expanded)
        val headlines = listOf("Read source", "scout · Update", "Run tests", "builder · Update")
        val tops = headlines.map { headline ->
            compose.onNodeWithText(headline).assertIsDisplayed().fetchSemanticsNode().boundsInRoot.top
        }
        assertTrue("Activity keeps chronological order", tops.zipWithNext().all { (a, b) -> a < b })

        compose.onNodeWithText("Source contents").assertDoesNotExist()
        compose.onNodeWithText("Read source").performClick()
        compose.onNodeWithText("Source contents").assertIsDisplayed()
        compose.onNodeWithText("Read source").performClick()
        compose.onNodeWithText("scout · Update").performClick()
        compose.onNodeWithText("The source review is complete.").assertIsDisplayed()
        compose.onNodeWithText("Message Type:", substring = true).assertDoesNotExist()
        compose.onNodeWithText("Sender:", substring = true).assertDoesNotExist()

        compose.onNodeWithText("Activity · 4 steps").performClick().assert(collapsed)
        headlines.forEach { compose.onNodeWithText(it).assertDoesNotExist() }
        compose.onNodeWithText("The source review is complete.").assertDoesNotExist()
        compose.onNodeWithText("The change is complete.").assertIsDisplayed()
    }

    @Test
    fun appendingActivityPreservesExpandedGroupAndChildState() {
        val conversation = mutableStateOf<List<Item>>(listOf(
            tool("read", "Read source", "Source contents"),
            update("scout", "The source review is complete."),
        ))
        compose.setContent {
            MaterialTheme {
                LazyColumn {
                    items(spineTimeline(conversation.value), key = { it.key }) { SpineTimelineRow(it) }
                }
            }
        }

        compose.onNodeWithText("Activity · 2 steps").performClick()
        compose.onNodeWithText("scout · Update").performClick()
        compose.onNodeWithText("The source review is complete.").assertIsDisplayed()
        compose.runOnIdle {
            conversation.value = conversation.value + tool("test", "Run tests", "All tests passed")
        }

        compose.onAllNodesWithText("Activity ·", substring = true).assertCountEquals(1)
        compose.onNodeWithText("Activity · 3 steps").assertIsDisplayed().assert(expanded)
        compose.onNodeWithText("Run tests").assertIsDisplayed()
        compose.onNodeWithText("The source review is complete.").assertIsDisplayed()
        compose.onNodeWithText("Activity · 3 steps").performClick().assert(collapsed)
        compose.onNodeWithText("Run tests").assertDoesNotExist()
        compose.onNodeWithText("The source review is complete.").assertDoesNotExist()
    }

    private fun tool(id: String, title: String, output: String) = Item.Tool(
        id = id, tool = "exec", title = title, category = ToolCategory.Execute,
        input = "", status = ToolStatus.Completed, output = output, ts = 0,
    )

    private fun update(name: String, payload: String) = Item.AgentText(
        id = "update-$name",
        text = "/root/$name → /root\nMessage Type: MESSAGE\nTask name: /root\nSender: /root/$name\nPayload:\n$payload",
        done = true, ts = 0,
    )

    private val collapsed = SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "Collapsed")
    private val expanded = SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "Expanded")
}
