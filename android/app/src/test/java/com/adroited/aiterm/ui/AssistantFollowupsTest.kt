package com.adroited.aiterm.ui

import org.junit.Assert.*
import org.junit.Test

class AssistantFollowupsTest {
    private val action = """:codex-followup[Draft a reply to Khris]{prompt="Draft a constructive reply to Khris explaining the verified issues and the proposed next steps."}"""
    @Test fun reportedResponseKeepsProseAndTurnsBulletsIntoActions() {
        val text = "I did not apply his destructive reset script to an existing database.\n•\n$action\n- $action\n* $action"
        val parts = assistantParts(text)
        assertEquals(3, parts.filterIsInstance<AssistantPart.Followup>().size)
        assertEquals("Draft a reply to Khris", parts.filterIsInstance<AssistantPart.Followup>()[0].label)
        assertEquals("I did not apply his destructive reset script to an existing database.", (parts[0] as AssistantPart.Markdown).text)
        assertFalse(assistantContent(text).copyText().contains(":codex-followup"))
        assertTrue(assistantContent(text).copyText().contains("verified issues"))
    }
    @Test fun examplesMalformedAndStreamingTextRemainVerbatim() {
        for (text in listOf("```text\n$action\n```", "~~~\n$action\n~~~", "> $action", "    $action", "`$action`", "Example: $action", action.dropLast(2), action.replace("prompt=", "other="))) {
            assertEquals(listOf(AssistantPart.Markdown(text)), assistantParts(text))
        }
    }
    @Test fun escapedPromptAndLabelsAreDecodedWithoutExecutingAnything() {
        val text = """:codex-followup[Review \] file]{prompt="Read \"example\"\nthen explain."}"""
        assertEquals(listOf(AssistantPart.Followup("Review ] file", "Read \"example\"\nthen explain.")), assistantParts(text))
    }
    @Test fun selectingFollowupPreservesAnExistingDraft() {
        assertEquals("suggestion", draftWithFollowup("", "suggestion"))
        assertEquals("My unfinished text\n\nsuggestion", draftWithFollowup("My unfinished text", "suggestion"))
    }
    @Test fun actionsAndMemoryReferencesCanCoexist() {
        val parsed = assistantContent("$action\n<oai-mem-citation><citation_entries>MEMORY.md:1-1|note=[source]</citation_entries></oai-mem-citation>")
        assertEquals(1, parsed.references.size)
        assertEquals(1, assistantParts(parsed.body).filterIsInstance<AssistantPart.Followup>().size)
        assertFalse(parsed.copyText().contains(":codex-followup"))
    }
}
