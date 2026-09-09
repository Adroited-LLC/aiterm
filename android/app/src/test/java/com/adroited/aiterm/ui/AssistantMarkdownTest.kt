package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.Item
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AssistantMarkdownTest {
    private val footer = """
        <oai-mem-citation>
        <citation_entries>
        MEMORY.md:168-168|note=[Located manual bid rotation ownership and verified Friday settings live]
        </citation_entries>
        <rollout_ids>
        01a07c42-06e8-7823-89b5-5135ad0b68c1
        </rollout_ids>
        </oai-mem-citation>
    """.trimIndent()

    @Test fun reportedFooterBecomesReadableReferencesWithoutChangingTheReply() {
        val parsed = assistantContent("Settings verified.\n\n$footer")
        assertEquals("Settings verified.", parsed.body)
        assertEquals(listOf(MemoryReference("MEMORY.md:168", "Located manual bid rotation ownership and verified Friday settings live")), parsed.references)
        assertFalse(parsed.copyText().contains("oai-mem-citation"))
        assertFalse(parsed.copyText().contains("01a07c42"))
        assertTrue(parsed.copyText().contains("MEMORY.md:168"))
    }

    @Test fun supportsMultipleReferencesAndLineRangesWithoutRolloutIds() {
        val text = "Reply.<oai-mem-citation><citation_entries>MEMORY.md:1-3|note=[First]\nnotes/setup.md:9-9|note=[Second]</citation_entries></oai-mem-citation>"
        assertEquals(listOf(MemoryReference("MEMORY.md:1–3", "First"), MemoryReference("notes/setup.md:9", "Second")), assistantContent(text).references)
        assertEquals("Reply.", assistantContent(text).body)
    }

    @Test fun ordinaryMarkupAndIncompleteOrInvalidFootersRemainVerbatim() {
        for (text in listOf("<example>normal text</example>", "Use `<oai-mem-citation>` for citations.", footer.substringBefore("</oai-mem-citation>"), footer.replace("168-168", "169-168"), "$footer\nText after the example.")) {
            assertEquals(AssistantContent(text, emptyList()), assistantContent(text))
        }
    }

    @Test fun codeExamplesAreNotMistakenForMetadata() {
        for (text in listOf("```xml\n$footer\n```", "~~~xml\n$footer\n~~~", "```xml\n$footer", "`$footer`", "> $footer", "    $footer")) {
            assertEquals(AssistantContent(text, emptyList()), assistantContent(text))
        }
        assertEquals("```\nexample\n```", assistantContent("```\nexample\n```\n$footer").body)
    }

    @Test fun copyAndShareNormalizeOnlyAssistantMessages() {
        val user = SpineTimelineItem.Row(Item.User("user", footer, 0L))
        val agent = SpineTimelineItem.Row(Item.AgentText("agent", footer, true, 0L))
        assertEquals(footer, timelineText(user))
        assertEquals(assistantContent(footer).copyText(), timelineText(agent))
        assertTrue(timelineText(agent).startsWith("Memory references:"))
    }
}
