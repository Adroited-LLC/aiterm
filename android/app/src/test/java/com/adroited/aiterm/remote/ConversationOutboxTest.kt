package com.adroited.aiterm.remote

import org.junit.Assert.*
import org.junit.Test

class ConversationOutboxTest {
    private fun page(vararg texts: Pair<Long, String>, epoch: Long = 1, session: String = "a") =
        SpineConversationPage(epoch, true, false, events = texts.map { (seq, text) ->
            SpineEventWire(seq, epoch, session, "codex", 0, "user_message", text = text)
        })

    @Test fun desktopAcceptanceKeepsPromptUntilANewerUserMessageArrives() {
        val outbox = ConversationOutbox()
        val id = outbox.begin("a", "continue", 1, 10)
        outbox.accepted(id)
        outbox.reconcile("a", page(10L to "continue"))
        assertTrue(outbox.prompts.single().accepted)
        outbox.reconcile("a", page(11L to "continue"))
        assertTrue(outbox.prompts.isEmpty())
    }

    @Test fun repeatedPromptsConsumeOneReceiptEachEvenWhenPagesReplay() {
        val outbox = ConversationOutbox()
        outbox.begin("a", "hello", 1, 10)
        val second = outbox.begin("a", "hello", 1, 10)
        repeat(2) { outbox.reconcile("a", page(11L to "hello")) }
        assertEquals(second, outbox.prompts.single().id)
        outbox.reconcile("a", page(12L to "hello"))
        assertTrue(outbox.prompts.isEmpty())
    }

    @Test fun unrelatedSessionOrEpochCannotConfirmAPrompt() {
        val outbox = ConversationOutbox()
        val id = outbox.begin("a", "hello", 1, 10)
        outbox.reconcile("b", page(11L to "hello", session = "b"))
        outbox.reconcile("a", page(20L to "hello", epoch = 2))
        assertEquals(id, outbox.prompts.single().id)
    }

    @Test fun coalescedUserTurnConfirmsAllIncludedQueuedMessages() {
        val outbox = ConversationOutbox()
        outbox.begin("a", "first", 1, 10)
        outbox.begin("a", "second\r\nline", 1, 10)
        outbox.reconcile("a", page(11L to "first\n\nsecond\nline\n"))
        assertTrue(outbox.prompts.isEmpty())
    }

    @Test fun failedSubmissionCanBeRemovedWithoutAffectingOtherSessions() {
        val outbox = ConversationOutbox()
        val first = outbox.begin("a", "first", 1, 10)
        val second = outbox.begin("b", "second", 1, 10)
        outbox.remove(first)
        assertEquals(second, outbox.prompts.single().id)
    }

    @Test fun attachmentPromptAndFollowUpDotConfirmFromOneUnseparatedUserMessage() {
        val outbox = ConversationOutbox()
        val prompt = "Please inspect this\n\nAttached images:\n- /project/first.jpg"
        val followUp = ".\n\nAttached images:\n- /project/second.jpg"
        outbox.accepted(outbox.begin("a", prompt, 1, 10))
        outbox.accepted(outbox.begin("a", followUp, 1, 12))
        outbox.reconcile("a", page(13L to prompt + followUp))
        assertTrue(outbox.prompts.isEmpty())
    }

    @Test fun coalescedReceiptDoesNotConsumeAnOlderUnmatchedPromptOrALaterRepeat() {
        val outbox = ConversationOutbox()
        val unmatched = outbox.begin("a", "unconfirmed", 1, 10)
        outbox.begin("a", "prompt", 1, 10)
        outbox.begin("a", ".", 1, 10)
        val repeated = outbox.begin("a", ".", 1, 10)
        repeat(2) { outbox.reconcile("a", page(11L to "prompt.")) }
        assertEquals(listOf(unmatched, repeated), outbox.prompts.map { it.id })
    }

    @Test fun coalescedMessagesStillRequireCausalityAndCompleteExactText() {
        val outbox = ConversationOutbox()
        outbox.begin("a", "prompt", 1, 10)
        outbox.begin("a", ".", 1, 12)
        outbox.reconcile("a", page(11L to "prompt."))
        outbox.reconcile("a", page(13L to "prompt.", epoch = 2))
        outbox.reconcile("b", page(13L to "prompt.", session = "b"))
        outbox.reconcile("a", page(13L to "echo: prompt."))
        assertEquals(2, outbox.prompts.size)
        outbox.reconcile("a", page(14L to "prompt\n."))
        assertTrue(outbox.prompts.isEmpty())
    }

    @Test fun fastTranscriptReceiptIsNotResurrectedByLateInputAcknowledgement() {
        val outbox = ConversationOutbox()
        val id = outbox.begin("a", "hello", 1, 10)
        outbox.reconcile("a", page(11L to "hello"))
        outbox.accepted(id)
        assertTrue(outbox.prompts.isEmpty())
    }
}
