package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.ToolCategory
import com.adroited.aiterm.remote.ToolStatus
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test

class AgentQuestionsTest {
    private fun tool(name: String, input: String, id: String = name) =
        Item.Tool(id, name, name, ToolCategory.Other, input, ToolStatus.Completed, "{\"accepted\":true}", 0)

    @Test fun allSixAsynchronousQuestionsAndOptionsAreVisibleAfterToolAcceptance() {
        val questions = buildJsonObject {
            putJsonArray("questions") { repeat(6) { index -> addJsonObject {
                put("title", "Question $index: " + "details ".repeat(60))
                putJsonArray("options") { add("First"); add("Second") }
            } } }
        }.toString()
        val parsed = agentQuestions(tool("functions.request_user_input_async", questions))!!
        assertEquals(6, parsed.size)
        assertEquals(listOf("First", "Second"), parsed.last().options.map { it.label })
        assertTrue(parsed.last().title.startsWith("Question 5:"))
    }

    @Test fun blockingCodexAndClaudeQuestionsKeepDescriptionsAndMultipleChoice() {
        val input = """{"questions":[{"id":"a","question":"Which one?","multiSelect":true,"options":[{"label":"Blue","description":"Low contrast"}]}]}"""
        for (name in listOf("request_user_input", "AskUserQuestion")) {
            val question = agentQuestions(tool(name, input))!!.single()
            assertEquals("Which one?", question.title)
            assertEquals("Low contrast", question.options.single().description)
            assertTrue(question.multiple)
        }
    }

    @Test fun questionsAreNotFoldedIntoMachineActivityIncludingClippedLegacyPayloads() {
        val question = tool("request_user_input_async", "{\"questions\":[…")
        assertEquals(emptyList<AgentQuestion>(), agentQuestions(question))
        val timeline = spineTimeline(listOf(tool("exec", "ls", "before"), question, tool("exec", "ls", "after")))
        assertEquals(3, timeline.size)
        assertTrue(timeline[0] is SpineTimelineItem.Activity)
        assertEquals(SpineTimelineItem.Row(question), timeline[1])
        assertNull(agentQuestions(tool("exec", "{}")))
    }

    @Test fun malformedQuestionPayloadDoesNotCrash() {
        for (input in listOf("{}", "null", "[]", "{\"questions\":false}", "{\"questions\":[null,42,{}]}")) {
            assertEquals(emptyList<AgentQuestion>(), agentQuestions(tool("request_user_input_async", input)))
        }
    }

    @Test fun navigationHonorsTheTerminalCursorMode() {
        assertEquals("\u001b[A", questionNavigationKeys(false).first().second)
        assertEquals("\u001bOA", questionNavigationKeys(true).first().second)
        assertEquals("\r", questionNavigationKeys(false).single { it.first == "Enter" }.second)
    }
}
