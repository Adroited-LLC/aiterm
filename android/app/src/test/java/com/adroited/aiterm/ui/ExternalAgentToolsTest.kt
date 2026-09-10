package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.ToolCategory
import com.adroited.aiterm.remote.ToolStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ExternalAgentToolsTest {
    private val path = "/home/matt/.claude/projects/-home-matt-Projects-gsync/memory/gsync-architecture.md"
    private val result = "The file $path has been updated successfully."
    private val envelope = """
        [external_agent_tool_call: Edit]
        file: $path
        [/external_agent_tool_call]
        [external_agent_tool_result]
        $result
        [/external_agent_tool_result]
    """.trimIndent()

    @Test fun reportedEditBecomesAnExpandableToolWithItsOriginalDetails() {
        val tool = externalAgentTools(envelope, "message", 123L)!!.single()
        assertEquals("message:external-tool:0", tool.id)
        assertEquals("External agent · Edit", tool.title)
        assertEquals(ToolCategory.Edit, tool.category)
        assertEquals(ToolStatus.Completed, tool.status)
        assertEquals("file: $path", tool.input)
        assertEquals(result, tool.output)
        assertEquals(123L, tool.ts)
    }

    @Test fun multipleCallsHaveStableDistinctIdsAndExplicitErrorsStayVisible() {
        val text = envelope + "\n\n" + envelope.replace("Edit]", "Bash]").replace(result, "Error: Permission denied")
        val tools = externalAgentTools(text, "message", 0L)!!
        assertEquals(2, tools.size)
        assertEquals(2, tools.map { it.id }.toSet().size)
        assertEquals(ToolCategory.Execute, tools[1].category)
        assertEquals(ToolStatus.Failed, tools[1].status)
        assertEquals("Error: Permission denied", tools[1].output)
        assertEquals(tools, externalAgentTools(text, "message", 0L))
    }

    @Test fun incompleteEnvelopesProseAndCodeExamplesAreNotReinterpreted() {
        for (text in listOf("Example:\n$envelope", "```\n$envelope\n```", "`$envelope`", "$envelope\nCommentary", envelope.substringBefore("[/external_agent_tool_result]"))) {
            assertNull(text, externalAgentTools(text, "message", 0L))
        }
    }

    @Test fun onlyAssistantEnvelopesFoldIntoActivityAndUserExamplesStayVisible() {
        val user = Item.User("user", envelope, 0L)
        val agent = Item.AgentText("agent", envelope, true, 1L)
        val rows = spineTimeline(listOf(user, agent, Item.AgentText("reply", "Done.", true, 2L)))
        assertEquals(SpineTimelineItem.Row(user), rows[0])
        assertTrue(rows[1] is SpineTimelineItem.Activity)
        assertEquals("Done.", ((rows[2] as SpineTimelineItem.Row).item as Item.AgentText).text)
        assertEquals(envelope, timelineText(rows[0]))
        val copied = timelineText(rows[1])
        assertFalse(copied.contains("[external_agent_tool"))
        assertTrue(copied.contains(path))
        assertTrue(copied.contains(result))
        assertEquals(copied, assistantCopyText(envelope))
    }

    @Test fun unknownToolsAndWindowsLineEndingsKeepTheirData() {
        val text = envelope.replace("Edit]", "CustomTool]").replace("\n", "\r\n")
        val tool = externalAgentTools(text, "message", 0L)!!.single()
        assertEquals(ToolCategory.Other, tool.category)
        assertEquals(result, tool.output)
        assertEquals("file: $path", tool.input)
    }
    @Test fun separateMessagesJoinWithoutChangingTheCallKey() {
        val call = envelope.substringBefore("[external_agent_tool_result]").trimEnd()
        val resultText = envelope.substringAfter("[/external_agent_tool_call]").trimStart()
        val first = Item.AgentText("call", call, true, 1L)
        val before = normalizeExternalTools(listOf(first)).single() as Item.Tool
        assertEquals(ToolStatus.Recorded, before.status)
        assertTrue(before.status.settled)
        val after = normalizeExternalTools(listOf(first, Item.AgentText("result", resultText, true, 2L))).single() as Item.Tool
        assertEquals(before.id, after.id)
        assertEquals(ToolStatus.Completed, after.status)
        assertEquals(result, after.output)
    }

    @Test fun errorHeadersAndToolUseErrorsBecomeFailuresWithoutRawWrappers() {
        val output = "[external_agent_tool_result: error]\n<tool_use_error>Permission denied</tool_use_error>\n[/external_agent_tool_result]"
        val tool = externalAgentTools(output, "result", 0L)!!.single()
        assertEquals(ToolStatus.Failed, tool.status)
        assertEquals("Permission denied", tool.output)
        assertEquals("External agent · Tool result", tool.title)
    }

    @Test fun ambiguousCallsAndConversationBoundariesDoNotMisattributeResults() {
        val call = envelope.substringBefore("[external_agent_tool_result]").trimEnd()
        val output = envelope.substringAfter("[/external_agent_tool_call]").trimStart()
        val a = Item.AgentText("a", call, true, 1L)
        val b = Item.AgentText("b", call, true, 2L)
        val reply = Item.AgentText("result", output, true, 3L)
        val ambiguous = normalizeExternalTools(listOf(a, b, reply)).filterIsInstance<Item.Tool>()
        assertEquals(3, ambiguous.size)
        assertNull(ambiguous[0].output)
        assertNull(ambiguous[1].output)
        assertEquals(result, ambiguous[2].output)
        for (barrier in listOf(Item.User("user", "Next task", 2L), Item.AgentText("prose", "A reply", true, 2L), Item.TurnEnd("turn", "completed"))) {
            val rows = normalizeExternalTools(listOf(a, barrier, reply))
            assertEquals(3, rows.size)
            assertNull((rows[0] as Item.Tool).output)
            assertEquals(barrier, rows[1])
        }
    }

    @Test fun allObservedToolNamesAndFutureNamesHaveCards() {
        val names = listOf("Bash", "ToolSearch", "WebSearch", "Read", "Write", "Edit", "Agent", "WebFetch", "AskUserQuestion", "Skill", "Monitor", "SendMessage", "TaskCreate", "TaskUpdate", "TaskStop", "Artifact", "mcp__custom_server__arbitrary_tool", "FutureTool")
        names.forEach { name ->
            val tool = externalAgentTools(envelope.replace("Edit]", "$name]"), "message", 0L)!!.single()
            assertEquals(name, tool.tool)
            assertEquals("External agent · $name", tool.title)
            assertEquals(result, tool.output)
        }
        assertEquals(ToolCategory.Think, externalToolCategory("AskUserQuestion"))
        assertEquals(ToolCategory.Search, externalToolCategory("ToolSearch"))
        assertEquals(ToolCategory.Other, externalToolCategory("mcp__custom_server__arbitrary_tool"))
    }

    @Test fun emptyResultsAreRecognizedAndFinishTheCall() {
        val call = envelope.substringBefore("[external_agent_tool_result]").trimEnd()
        val empty = "[external_agent_tool_result]\n[/external_agent_tool_result]"
        val tool = externalAgentTools("$call\n$empty", "message", 0L)!!.single()
        assertEquals(ToolStatus.Completed, tool.status)
        assertEquals("", tool.output)
    }

}
