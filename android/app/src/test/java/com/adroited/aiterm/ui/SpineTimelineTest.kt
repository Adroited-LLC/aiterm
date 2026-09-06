package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.ToolCategory
import com.adroited.aiterm.remote.ToolStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SpineTimelineTest {
    private fun tool(id: String, status: ToolStatus = ToolStatus.Completed) = Item.Tool(
        id, "exec", "Run $id", ToolCategory.Execute, "command $id", status, "output $id", 1,
    )

    private fun update(id: String, type: String = "MESSAGE") = Item.AgentText(
        id,
        "/root/checks → /root\nMessage Type: $type\nTask name: /root\nSender: /root/checks\nPayload:\nResult $id",
        true,
        1,
    )

    @Test
    fun mixedToolsUpdatesAndCompletionBecomeOneChronologicalActivity() {
        val items = listOf(tool("read"), update("progress"), tool("test"), update("done", "FINAL_ANSWER"))

        assertEquals(listOf(SpineTimelineItem.Activity(items)), spineTimeline(items))
    }

    @Test
    fun ordinaryMessagesThoughtsAndTurnEndsKeepActivityBoundaries() {
        val boundaries = listOf(
            Item.User("user", "Please continue", 1),
            Item.AgentText("assistant", "Here is my update", true, 1),
            Item.Thought("thought", "Check the result", true, 1),
            Item.TurnEnd("turn", "completed"),
        )
        boundaries.forEach { boundary ->
            val before = tool("before")
            val after = update("after")
            assertEquals(
                listOf(
                    SpineTimelineItem.Activity(listOf(before)),
                    SpineTimelineItem.Row(boundary),
                    SpineTimelineItem.Activity(listOf(after)),
                ),
                spineTimeline(listOf(before, boundary, after)),
            )
        }
    }

    @Test
    fun standaloneSubagentAndToolEachGetAnActivityRow() {
        listOf(update("solo"), tool("solo")).forEach { item ->
            assertEquals(listOf(SpineTimelineItem.Activity(listOf(item))), spineTimeline(listOf(item)))
        }
        assertTrue(spineTimeline(emptyList()).isEmpty())
    }

    @Test
    fun appendedActivityAndToolStatusChangesKeepTheSameKey() {
        val running = tool("first", ToolStatus.Running)
        val initial = spineTimeline(listOf(running)).single()
        val appended = spineTimeline(listOf(running.copy(status = ToolStatus.Completed), update("next"))).single()
        assertEquals("activity:first", initial.key)
        assertEquals(initial.key, appended.key)

        val subagentFirst = update("agent-first")
        assertEquals(
            spineTimeline(listOf(subagentFirst)).single().key,
            spineTimeline(listOf(subagentFirst, tool("later"))).single().key,
        )
    }

    @Test
    fun incompleteOrUnrelatedEnvelopesRemainOrdinaryMessages() {
        val message = Item.AgentText("ordinary", "Message Type: MESSAGE\nI am explaining the protocol.", false, 1)
        assertEquals(listOf(SpineTimelineItem.Row(message)), spineTimeline(listOf(message)))
    }

    @Test
    fun activityCopyTextPreservesToolDetailsAndSubagentMessagesInOrder() {
        val first = tool("first")
        val message = update("agent")
        val last = tool("last")
        val activity = SpineTimelineItem.Activity(listOf(first, message, last))
        assertEquals(
            "Run first\n\ncommand first\n\noutput first\n\n${message.text}\n\nRun last\n\ncommand last\n\noutput last",
            timelineText(activity),
        )
    }
}
