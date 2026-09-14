package com.adroited.aiterm.ui

import org.junit.Assert.*
import org.junit.Test

class AssistantFileCitationsTest {
    private val path = "/home/matt/Downloads/khris/VextorLogix_Lee_County_Development_Review.docx"
    private val directive = """:codex-file-citation{path="$path" purpose="output"}"""

    @Test fun inlineReportCitationBecomesAFileWithoutLosingSurroundingText() {
        assertEquals(listOf(AssistantPart.Markdown("Created the Word document: "),
            AssistantPart.FileCitation(path), AssistantPart.Markdown(". Done.")),
            assistantParts("Created the Word document: $directive. Done."))
        assertEquals("Created: $path", assistantContent("Created: $directive").copyText())
    }

    @Test fun codeQuotedEscapedAndIncompleteExamplesStayUntouched() {
        for (text in listOf("`$directive`", "``$directive``", "```text\n$directive\n```",
            "~~~\n$directive\n~~~", "    $directive", "\t$directive", "> $directive",
            "\\$directive", directive.dropLast(1), "`code\n$directive\ncode`")) {
            assertEquals(text, listOf(AssistantPart.Markdown(text)), assistantParts(text))
        }
    }

    @Test fun onlyValidHostPathsAndKnownUniqueAttributesBecomeActions() {
        for (text in listOf(directive.replace(path, "https://example.com/a.docx"),
            directive.replace(path, "javascript:alert(1)"), directive.replace(path, "relative.docx"),
            directive.replace(path, "//server/report.docx"), directive.replace(path, "/bad\\nfile"),
            directive.replace("purpose=", "unknown="), directive.replace("purpose=", "path="))) {
            assertEquals(text, listOf(AssistantPart.Markdown(text)), assistantParts(text))
        }
    }

    @Test fun attributeOrderSpacesAndEscapedQuotesAreSupported() {
        val text = """:codex-file-citation{purpose="output" path="/home/matt/My \"Review\" [final].docx"}"""
        assertEquals(listOf(AssistantPart.FileCitation("/home/matt/My \"Review\" [final].docx")), assistantParts(text))
    }

    @Test fun filesAndFollowupsCoexist() {
        val text = "$directive\n:codex-followup[Review it]{prompt=\"Review the document.\"}"
        assertEquals(1, assistantParts(text).filterIsInstance<AssistantPart.FileCitation>().size)
        assertEquals(1, assistantParts(text).filterIsInstance<AssistantPart.Followup>().size)
    }
}
