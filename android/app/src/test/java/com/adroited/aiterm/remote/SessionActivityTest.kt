package com.adroited.aiterm.remote

import org.junit.Assert.assertEquals
import org.junit.Test

class SessionActivityTest {
    private fun page(seq: Long, open: Boolean?, vararg events: SpineEventWire, epoch: Long = 1, more: Boolean = false) =
        SpineConversationPage(epoch, true, more, latestSeq = seq, turnOpen = open, events = events.toList())
    private fun phase(seq: Long, value: String, detail: String = "") =
        SpineEventWire(seq, 1, "s", "codex", seq, "phase", phase = value, detail = detail)

    @Test fun idleTurnWinsEvenWhenTerminalRepaintsAndPhaseIsStale() {
        val status = SessionActivity()
        status.apply(page(10, false, phase(10, "working")))
        assertEquals("idle", status.activity)
        status.apply(page(11, false, phase(11, "needs_you", "approval")))
        assertEquals("idle", status.activity)
    }

    @Test fun quietLongRunningTurnDoesNotBecomeAnApprovalRequest() {
        val status = SessionActivity()
        for (detail in listOf("approval", "a tool call is waiting")) {
            status.apply(page(10, true, phase(10, "needs_you", detail)))
            assertEquals("output", status.activity)
        }
        status.apply(page(11, true, phase(11, "idle")))
        assertEquals("output", status.activity)
    }

    @Test fun explicitPermissionIsPreservedThroughMetadataOnlyPolls() {
        val status = SessionActivity()
        status.apply(page(10, true, phase(10, "needs_you", "permission: Bash")))
        assertEquals("attention", status.activity)
        status.apply(page(10, true))
        assertEquals("attention", status.activity)
        status.apply(page(11, true, phase(11, "working")))
        assertEquals("output", status.activity)
    }

    @Test fun slowStatusReplyCannotUndoACompletedTurnFromThePreview() {
        val status = SessionActivity()
        status.apply(page(20, false, phase(20, "idle")))
        status.apply(page(15, true, phase(15, "working")))
        assertEquals("idle", status.activity)
    }

    @Test fun restartedDesktopDiscardsPriorPermissionAndSequence() {
        val status = SessionActivity()
        status.apply(page(100, true, phase(100, "needs_you", "permission")))
        status.apply(page(1, true, epoch = 2))
        assertEquals("output", status.activity)
        assertEquals(1L, status.latestSeq)
    }

    @Test fun incompleteHistoryNeverOverrulesAtomicTurnState() {
        val status = SessionActivity()
        status.apply(page(100, true, phase(1, "needs_you", "permission"), more = true))
        assertEquals("output", status.activity)
        status.apply(page(101, false))
        assertEquals("idle", status.activity)
    }

    @Test fun nextTurnDoesNotInheritAPreviousPermissionRequest() {
        val status = SessionActivity()
        status.apply(page(10, true, phase(10, "needs_you", "permission")))
        status.apply(page(11, false))
        status.apply(page(12, true))
        assertEquals("output", status.activity)
    }

    @Test fun unknownOrLegacyStatusDoesNotInventWork() {
        val status = SessionActivity()
        status.apply(page(0, null))
        assertEquals(null, status.activity)
    }
}
