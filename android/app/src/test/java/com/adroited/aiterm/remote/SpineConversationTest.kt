package com.adroited.aiterm.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class SpineConversationTest {
    @Test
    fun parsesEveryKindAndToleratesNewValues() {
        val store = SpineConversationStore()
        store.apply(
            page(
                event(1, "user_message", id = "u1", text = "do it"),
                event(2, "agent_text", id = "a1", text = "hello", done = true),
                event(3, "agent_thought", id = "thought", text = "hmm", done = false),
                event(
                    4, "tool_call", id = "tool", tool = "Magic", title = "Do magic",
                    category = "telepathy", input = "now", status = "levitating",
                ),
                event(5, "turn_started", turn = "turn-1"),
                event(6, "phase", phase = "needs_you", detail = "approve Edit"),
            ),
        )

        assertEquals(listOf("u1", "a1", "thought", "tool"), store.items.map(Item::key))
        assertEquals(ToolCategory.Other, (store.items[3] as Item.Tool).category)
        assertEquals(ToolStatus.Pending, (store.items[3] as Item.Tool).status)
        assertEquals("turn-1", store.currentTurn)
        assertEquals(true, store.turnOpen)
        assertEquals(SpinePhase.NeedsYou, store.phase)
        assertEquals("approve Edit", store.phaseDetail)
        assertTrue(store.phaseSeen)
    }

    @Test
    fun growingBlocksUpsertInPlaceAndToolUpdatesKeepOneRow() {
        val store = SpineConversationStore()
        store.apply(
            page(
                event(1, "agent_text", id = "a1", text = "Work", done = false),
                event(
                    2, "tool_call", id = "t1", tool = "exec", title = "Run tests",
                    category = "execute", input = "cargo test", status = "running",
                ),
            ),
        )
        store.apply(
            page(
                event(3, "agent_text", id = "a1", text = "Working now", done = true),
                event(4, "tool_call_update", id = "t1", status = "completed", output = "all passed"),
                event(5, "tool_call_update", id = "missing", status = "failed"),
            ),
        )

        assertEquals(listOf("a1", "t1"), store.items.map(Item::key))
        assertEquals(Item.AgentText("a1", "Working now", true, 3), store.items[0])
        val tool = store.items[1] as Item.Tool
        assertEquals(ToolStatus.Completed, tool.status)
        assertEquals("all passed", tool.output)
        assertEquals(ToolCategory.Execute, tool.category)
        assertEquals(5, store.lastSeq)
    }

    @Test
    fun replayDeduplicatesAndNewEpochStartsClean() {
        val store = SpineConversationStore()
        store.apply(page(event(1, "user_message", id = "u1", text = "old")))
        store.apply(
            page(
                event(1, "user_message", id = "u1", text = "ignored"),
                event(2, "agent_text", id = "a1", text = "answer"),
            ),
        )
        assertEquals(listOf("u1", "a1"), store.items.map(Item::key))
        assertEquals("old", (store.items[0] as Item.User).text)

        store.apply(page(event(1, "user_message", id = "u2", text = "fresh", epoch = 2), epoch = 2))
        assertEquals(2, store.epoch)
        assertEquals(1, store.lastSeq)
        assertEquals(listOf("u2"), store.items.map(Item::key))
    }

    @Test
    fun offerEnforcesSequenceAndEpochRules() {
        val store = SpineConversationStore()
        assertEquals(Offer.Applied, store.offer(userEvent(1, 1, "one")))
        assertEquals(Offer.Stale, store.offer(userEvent(1, 1, "one")))
        assertEquals(Offer.Gap, store.offer(agentEvent(3, 1, "gap")))
        assertEquals(Offer.EpochChanged, store.offer(agentEvent(2, 2, "new")))
        assertEquals(1, store.lastSeq)
        assertEquals(listOf("u1"), store.items.map(Item::key))
    }

    @Test
    fun resetDropsRowsButKeepsCursorForFollowingRebuild() {
        val store = SpineConversationStore()
        store.apply(
            page(
                event(1, "user_message", id = "u1", text = "old"),
                event(2, "turn_started", turn = "turn-1"),
                event(3, "reset"),
                event(4, "agent_text", id = "a1", text = "rebuilt"),
            ),
        )
        assertEquals(listOf("a1"), store.items.map(Item::key))
        assertEquals(4, store.lastSeq)
        assertNull(store.currentTurn)
        assertNull(store.turnOpen)
    }

    @Test
    fun realUserEventRetiresOldestOptimisticEcho() {
        val store = SpineConversationStore()
        store.echoUser("do it", 1234)
        assertEquals(1, store.items.size)
        store.apply(page(event(1, "user_message", id = "u1", text = "do it")))
        assertEquals(listOf("u1"), store.items.map(Item::key))
    }

    @Test
    fun turnAndPhaseMetadataRemainTyped() {
        val store = SpineConversationStore()
        assertEquals(SpinePhase.Idle, store.phase)
        assertFalse(store.phaseSeen)
        store.apply(
            page(
                event(1, "turn_started", turn = "turn-1"),
                event(2, "phase", phase = "working", detail = "running Bash"),
                event(3, "turn_ended", turn = "turn-1", reason = "completed"),
                event(4, "phase", phase = "idle", detail = ""),
            ),
        )
        assertNull(store.currentTurn)
        assertEquals(false, store.turnOpen)
        assertEquals(SpinePhase.Idle, store.phase)
        assertEquals(Item.TurnEnd("turn-1", "completed"), store.items.last())
    }

    @Test
    fun unknownKindsAreIgnoredWithoutStoppingKnownEvents() {
        val store = SpineConversationStore()
        store.apply(
            page(
                event(1, "future_telemetry"),
                event(2, "user_message", id = "u1", text = "still here"),
            ),
        )
        assertEquals(listOf("u1"), store.items.map(Item::key))
        assertEquals(2, store.lastSeq)
    }

    @Test
    fun legacyTranscriptMapsToTypedRowsAndClassifiesTools() {
        val store = SpineConversationStore()
        store.legacy(
            listOf(
                RemotePreviewMessage("user", "hi"),
                RemotePreviewMessage("assistant", "hello"),
                RemotePreviewMessage("Bash", "ls -la\ntotal 4"),
            ),
        )
        assertFalse(store.live)
        assertEquals(listOf("legacy-0", "legacy-1", "legacy-2"), store.items.map(Item::key))
        val tool = store.items[2] as Item.Tool
        assertEquals(ToolCategory.Execute, tool.category)
        assertEquals("ls -la", tool.input)
    }

    @Test
    fun clearForgetsAllMetadata() {
        val store = SpineConversationStore()
        store.apply(page(event(1, "phase", phase = "working", detail = "busy"), live = false))
        store.clear()
        assertTrue(store.items.isEmpty())
        assertEquals(0, store.lastSeq)
        assertEquals(0, store.epoch)
        assertTrue(store.live)
        assertFalse(store.phaseSeen)
        assertSame(SpinePhase.Idle, store.phase)
        assertNull(store.turnOpen)
    }

    private fun userEvent(seq: Long, epoch: Long, text: String) = SpineEvent(
        seq, epoch, "session-1", "codex", seq,
        SpineKind.UserMessage("u$seq", text),
    )

    private fun agentEvent(seq: Long, epoch: Long, text: String) = SpineEvent(
        seq, epoch, "session-1", "codex", seq,
        SpineKind.AgentText("a$seq", text, true),
    )

    private fun page(
        vararg events: SpineEventWire,
        epoch: Long = 1,
        live: Boolean = true,
    ) = SpineConversationPage(epoch, live, hasMore = false, events = events.toList())

    private fun event(
        seq: Long,
        kind: String,
        id: String? = null,
        text: String? = null,
        done: Boolean? = null,
        tool: String? = null,
        title: String? = null,
        category: String? = null,
        input: String? = null,
        status: String? = null,
        output: String? = null,
        turn: String? = null,
        reason: String? = null,
        phase: String? = null,
        detail: String? = null,
        epoch: Long = 1,
    ) = SpineEventWire(
        seq, epoch, "session-1", "codex", seq, kind, id, text, done, tool, title,
        category, input, status, output, turn, reason, phase, detail,
    )
}
