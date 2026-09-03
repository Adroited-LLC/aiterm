package com.adroited.aiterm.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SpineConversationTest {
    @Test
    fun snapshotsUpsertGrowingTextAndToolResults() {
        val store = SpineConversationStore()
        store.apply(
            page(
                event(1, "user_message", id = "u1", text = "Build it"),
                event(2, "agent_text", id = "a1", text = "Work", done = false),
                event(
                    3,
                    "tool_call",
                    id = "t1",
                    tool = "exec",
                    title = "Run tests",
                    category = "execute",
                    input = "cargo test",
                    status = "running",
                ),
            ),
        )
        val result = store.apply(
            page(
                event(4, "agent_text", id = "a1", text = "Working now", done = true),
                event(5, "tool_call_update", id = "t1", status = "completed", output = "all passed"),
            ),
        )

        assertEquals(listOf("user", "assistant", "tool.execute"), result.map { it.role })
        assertEquals("Working now", result[1].text)
        assertTrue(result[2].text.contains("Run tests"))
        assertTrue(result[2].text.contains("all passed"))
        assertEquals(5, store.lastSeq)
    }

    @Test
    fun resetDropsOldHistoryAndNewEpochStartsClean() {
        val store = SpineConversationStore()
        store.apply(page(event(1, "agent_text", id = "old", text = "old")))
        val reset = page(event(2, "reset"), event(3, "agent_text", id = "new", text = "new"))
        assertEquals(listOf("new"), store.apply(reset).map { it.text })

        val restarted = page(event(1, "agent_text", id = "fresh", text = "fresh", epoch = 2), epoch = 2)
        assertEquals(listOf("fresh"), store.apply(restarted).map { it.text })
        assertEquals(1, store.lastSeq)
    }

    private fun page(vararg events: SpineEventWire, epoch: Long = 1) =
        SpineConversationPage(epoch, live = true, hasMore = false, events = events.toList())

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
        epoch: Long = 1,
    ) = SpineEventWire(
        seq = seq,
        epoch = epoch,
        sessionId = "session-1",
        agent = "codex",
        ts = seq,
        kind = kind,
        id = id,
        text = text,
        done = done,
        tool = tool,
        title = title,
        category = category,
        input = input,
        status = status,
        output = output,
    )
}
