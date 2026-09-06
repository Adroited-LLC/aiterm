package com.adroited.aiterm.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SubagentMessageTest {
    private fun envelope(type: String = "MESSAGE", payload: String = "Ready.") =
        "/root/performance → /root\nMessage Type: $type\nTask name: /root\nSender: /root/performance\nPayload:\n$payload"

    @Test
    fun parsesUpdateAndPreservesMultilineMarkdownPayloadExactly() {
        val payload = "**Results**\n\n- First\n- Second\n\n  indented\n"
        assertEquals(SubagentMessage("performance", "Update", payload), parseSubagentMessage(envelope(payload = payload)))
    }

    @Test
    fun completedAndTaskMessagesUseMeaningfulAgentNames() {
        assertEquals("performance · Completed", parseSubagentMessage(envelope("FINAL_ANSWER"))?.headline)
        val task = "/root → /root/review/ui\nMessage Type: NEW_TASK\nTask name: /root/review/ui\nSender: /root\nPayload:\nReview this."
        assertEquals(SubagentMessage("ui", "Task", "Review this."), parseSubagentMessage(task))
    }

    @Test
    fun emptyPayloadStillRecognizesCompleteEnvelope() {
        assertEquals("", parseSubagentMessage(envelope(payload = ""))?.payload)
        assertEquals("", parseSubagentMessage(envelope(payload = "").removeSuffix("\n"))?.payload)
        assertEquals(" \n", parseSubagentMessage(envelope(payload = " \n"))?.payload)
    }

    @Test
    fun preservesHeaderLookingTextInsidePayload() {
        val payload = "Message Type: FINAL_ANSWER\nTask name: /root\nSender: /root/other\nPayload:\nKeep all of this."
        assertEquals(payload, parseSubagentMessage(envelope(payload = payload))?.payload)
    }

    @Test
    fun supportsWindowsHeaderLineEndingsWithoutRewritingPayload() {
        val payload = "One\r\nTwo\nThree"
        val text = envelope(payload = "").replace("\n", "\r\n") + payload
        assertEquals(payload, parseSubagentMessage(text)?.payload)
    }

    @Test
    fun leavesOrdinaryQuotedMalformedAndUnknownMessagesUntouched() {
        val valid = envelope()
        listOf(
            "An ordinary assistant reply.",
            "Message Type: MESSAGE\nTask name: /root\nSender: /root/performance\nPayload:\nReady.",
            "Here is an example:\n$valid",
            "```\n$valid\n```",
            envelope("UNKNOWN"),
            valid.replace("Sender: /root/performance", "Sender: /root/other"),
            valid.replace("Task name: /root", "Task name: /root/other"),
            valid.replace("Payload:\n", ""),
            valid.replace("Task name: /root\n", ""),
            valid.replace("/root/performance", "/rootish/performance"),
            valid.replace("/root/performance", "/root/../performance"),
            valid.replace("Payload:\nReady.", "Payload: Ready."),
        ).forEach { assertNull(it, parseSubagentMessage(it)) }
    }
}
