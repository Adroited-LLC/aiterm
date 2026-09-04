package com.fivelime.aiterm

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long

/** The spine, phone side: one live event stream for every harness.
 *  Mirrors `src-tauri/src/spine/mod.rs`; the contract is `docs/spine.md`.
 *  Pure Kotlin on purpose — no Android, no Compose — so the rules that
 *  decide what the screen shows are unit-testable. */

/** What kind of thing a tool call is, so a card wears the right mark
 *  without knowing the engine's tool names. */
enum class ToolCategory(val wire: String) {
    Read("read"), Edit("edit"), Execute("execute"), Search("search"),
    Fetch("fetch"), Think("think"), Other("other");

    companion object {
        /** Tolerant: a category this build has never heard of is "other",
         *  never a crash — the desktop may ship a new one first. */
        fun from(s: String?): ToolCategory = entries.firstOrNull { it.wire == s } ?: Other
    }
}

enum class ToolStatus(val wire: String) {
    Pending("pending"), Running("running"), Completed("completed"),
    Failed("failed"), Cancelled("cancelled");

    val settled: Boolean get() = this == Completed || this == Failed || this == Cancelled

    companion object {
        fun from(s: String?): ToolStatus = entries.firstOrNull { it.wire == s } ?: Pending
    }
}

enum class SpinePhase(val wire: String) {
    Working("working"), NeedsYou("needs_you"), Idle("idle");

    companion object {
        fun from(s: String?): SpinePhase = entries.firstOrNull { it.wire == s } ?: Working
    }
}

/** One thing that happened, as the wire carries it. */
sealed interface SpineKind {
    data class UserMessage(val id: String, val text: String) : SpineKind
    data class AgentText(val id: String, val text: String, val done: Boolean) : SpineKind
    data class AgentThought(val id: String, val text: String, val done: Boolean) : SpineKind
    data class ToolCall(
        val id: String, val tool: String, val title: String,
        val category: ToolCategory, val input: String, val status: ToolStatus,
    ) : SpineKind
    data class ToolCallUpdate(val id: String, val status: ToolStatus, val output: String?) : SpineKind
    data class TurnStarted(val turn: String) : SpineKind
    data class TurnEnded(val turn: String, val reason: String) : SpineKind
    data class PhaseChanged(val phase: SpinePhase, val detail: String) : SpineKind
    data object Reset : SpineKind
}

data class SpineEvent(
    val seq: Long,
    val epoch: Long,
    val sessionId: String,
    val agent: String,
    val ts: Long,
    val kind: SpineKind,
) {
    companion object {
        /** Parses one flattened wire object. Null when the kind is unknown
         *  or a required field is missing: a phone one release behind the
         *  desktop drops what it cannot draw and keeps the rest. */
        fun parse(o: JsonObject): SpineEvent? {
            val kind = parseKind(o) ?: return null
            return SpineEvent(
                seq = o.num("seq"),
                epoch = o.num("epoch"),
                sessionId = o.str("session_id") ?: "",
                agent = o.str("agent") ?: "",
                ts = o.num("ts"),
                kind = kind,
            )
        }

        private fun parseKind(o: JsonObject): SpineKind? = when (o.str("kind")) {
            "user_message" -> SpineKind.UserMessage(o.str("id") ?: return null, o.str("text") ?: "")
            "agent_text" -> SpineKind.AgentText(o.str("id") ?: return null, o.str("text") ?: "", o.bool("done"))
            "agent_thought" -> SpineKind.AgentThought(o.str("id") ?: return null, o.str("text") ?: "", o.bool("done"))
            "tool_call" -> SpineKind.ToolCall(
                id = o.str("id") ?: return null,
                tool = o.str("tool") ?: "",
                title = o.str("title") ?: "",
                category = ToolCategory.from(o.str("category")),
                input = o.str("input") ?: "",
                status = ToolStatus.from(o.str("status")),
            )
            "tool_call_update" -> SpineKind.ToolCallUpdate(
                id = o.str("id") ?: return null,
                status = ToolStatus.from(o.str("status")),
                output = o.str("output"),
            )
            "turn_started" -> SpineKind.TurnStarted(o.str("turn") ?: "")
            "turn_ended" -> SpineKind.TurnEnded(o.str("turn") ?: "", o.str("reason") ?: "unknown")
            "phase" -> SpineKind.PhaseChanged(SpinePhase.from(o.str("phase")), o.str("detail") ?: "")
            "reset" -> SpineKind.Reset
            else -> null
        }
    }
}

/** `GET /v1/sessions/{id}/spine?after=N`. `live` is false when the desktop
 *  serves this session through its legacy adapter — nothing on the phone
 *  branches on it; it is there to be shown, not obeyed. */
data class SpineResponse(val epoch: Long, val live: Boolean, val events: List<SpineEvent>) {
    companion object {
        fun parse(o: JsonObject): SpineResponse = SpineResponse(
            epoch = o.num("epoch"),
            live = o["live"]?.let { runCatching { it.jsonPrimitive.boolean }.getOrDefault(true) } ?: true,
            events = o["events"]?.let { arr ->
                runCatching { arr.jsonArray }.getOrNull()
                    ?.mapNotNull { e -> runCatching { e.jsonObject }.getOrNull()?.let { SpineEvent.parse(it) } }
            } ?: emptyList(),
        )
    }
}

private fun JsonObject.str(k: String): String? =
    (this[k] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content

private fun JsonObject.num(k: String): Long =
    this[k]?.let { runCatching { it.jsonPrimitive.long }.getOrNull() } ?: 0L

private fun JsonObject.bool(k: String): Boolean =
    this[k]?.let { runCatching { it.jsonPrimitive.boolean }.getOrNull() } ?: false

/** One row of the conversation. Ids come from the source and are stable
 *  across re-reads, so a growing block re-renders in place instead of
 *  re-keying the list under the scroll position. */
sealed interface Item {
    val key: String

    data class User(val id: String, val text: String, val ts: Long) : Item {
        override val key: String get() = id
    }
    data class AgentText(val id: String, val text: String, val done: Boolean, val ts: Long) : Item {
        override val key: String get() = id
    }
    data class Thought(val id: String, val text: String, val done: Boolean, val ts: Long) : Item {
        override val key: String get() = id
    }
    data class Tool(
        val id: String, val tool: String, val title: String, val category: ToolCategory,
        val input: String, val status: ToolStatus, val output: String?, val ts: Long,
    ) : Item {
        override val key: String get() = id
    }
    data class TurnEnd(val turn: String, val reason: String) : Item {
        override val key: String get() = TURN_KEY_PREFIX + turn
    }

    companion object { const val TURN_KEY_PREFIX = "\u0000turn:" }
}

/** What `offer` decided about an event that arrived on the WebSocket. */
enum class Offer {
    /** In sequence and applied. */
    Applied,
    /** A seq was missed — refetch from `lastSeq`. */
    Gap,
    /** Already seen; dropped. */
    Stale,
    /** The desktop restarted — drop everything and refetch from 0. */
    EpochChanged,
}

/** Everything the screen shows for one session. Fed by `replay` (the GET)
 *  and `offer` (the WebSocket); never rebuilt from scratch on a normal
 *  event. Not thread-safe: the ViewModel touches it from the main
 *  dispatcher only. */
class ConversationStore {
    private val rows = ArrayList<Item>()
    private val at = HashMap<String, Int>()
    private var snapshot: List<Item> = emptyList()
    private var dirty = false

    /** The registry's start time. 0 until the first event or fetch. */
    var epoch: Long = 0L; private set
    /** The highest seq applied. The refetch cursor. */
    var lastSeq: Long = 0L; private set
    /** False while the desktop serves this session through the legacy adapter. */
    var live: Boolean = true; private set
    var phase: SpinePhase = SpinePhase.Idle; private set
    var phaseDetail: String = ""; private set
    /** True once the desktop has said anything about the phase — before
     *  that, the list's own activity is the better guess. */
    var phaseSeen: Boolean = false; private set
    var currentTurn: String? = null; private set

    /** A fresh immutable list, swapped only when something changed: rows
     *  are equal data classes, so Compose skips every row but the one that
     *  moved. */
    val items: List<Item>
        get() {
            if (dirty) { snapshot = ArrayList(rows); dirty = false }
            return snapshot
        }

    /** Everything back to empty: a different session, or a desktop that
     *  restarted under us. */
    fun clear() {
        rows.clear(); at.clear(); dirty = true
        epoch = 0L; lastSeq = 0L; live = true
        phase = SpinePhase.Idle; phaseDetail = ""; phaseSeen = false
        currentTurn = null
    }

    fun tool(id: String): Item.Tool? = at[id]?.let { rows[it] as? Item.Tool }

    /** The seq rules from docs/spine.md "Client rule (the phone)". */
    fun offer(e: SpineEvent): Offer {
        if (epoch != 0L && e.epoch != 0L && e.epoch != epoch) return Offer.EpochChanged
        if (epoch == 0L) epoch = e.epoch
        if (e.seq <= lastSeq) return Offer.Stale
        if (e.seq > lastSeq + 1) return Offer.Gap
        apply(e)
        lastSeq = e.seq
        return Offer.Applied
    }

    /** The GET's answer. Dedupes by seq: a refetch after a gap overlaps
     *  what the WebSocket already applied. */
    fun replay(r: SpineResponse) {
        if (epoch != 0L && r.epoch != 0L && r.epoch != epoch) clear()
        if (r.epoch != 0L) epoch = r.epoch
        live = r.live
        replay(r.events)
    }

    fun replay(events: List<SpineEvent>) {
        for (e in events.sortedBy { it.seq }) {
            if (e.seq <= lastSeq) continue
            if (epoch == 0L) epoch = e.epoch
            apply(e)
            lastSeq = e.seq
        }
    }

    /** Applies one event's content. Seq and epoch are the caller's problem
     *  — `replay` and `offer` own those. */
    fun apply(e: SpineEvent) {
        when (val k = e.kind) {
            is SpineKind.UserMessage ->
                // Appends, except when the id is one we hold: the contract
                // says an id seen twice replaces, never appends.
                upsert(Item.User(k.id, k.text, e.ts)) { dropOldestEcho() }
            is SpineKind.AgentText -> upsert(Item.AgentText(k.id, k.text, k.done, e.ts))
            is SpineKind.AgentThought -> upsert(Item.Thought(k.id, k.text, k.done, e.ts))
            is SpineKind.ToolCall ->
                upsert(Item.Tool(k.id, k.tool, k.title, k.category, k.input, k.status, tool(k.id)?.output, e.ts))
            is SpineKind.ToolCallUpdate -> {
                // An update for a call we never saw is dropped, not turned
                // into a blank card: the refetch that heals the gap brings
                // the tool_call with it.
                val cur = tool(k.id) ?: return
                upsert(cur.copy(status = k.status, output = k.output ?: cur.output))
            }
            is SpineKind.TurnStarted -> currentTurn = k.turn
            is SpineKind.TurnEnded -> {
                currentTurn = null
                upsert(Item.TurnEnd(k.turn, k.reason))
            }
            is SpineKind.PhaseChanged -> {
                phase = k.phase; phaseDetail = k.detail; phaseSeen = true
            }
            SpineKind.Reset -> {
                // History was rebuilt over there. Drop what we hold; the
                // events that follow carry the rebuild, so the seq cursor
                // stays where it is.
                rows.clear(); at.clear(); dirty = true
                currentTurn = null
            }
        }
    }

    /** An optimistic echo of what was just sent, so the bubble appears on
     *  tap rather than a round trip later. The desktop's own
     *  `user_message` replaces it. */
    fun echoUser(text: String, ts: Long) {
        upsert(Item.User(ECHO + ts + "-" + rows.size, text, ts))
    }

    /** An older desktop with no `/v1/spine`: its flat transcript mapped
     *  onto items by ordinal, so the last block still grows in place. */
    fun legacy(turns: List<Turn>) {
        rows.clear(); at.clear(); dirty = true
        turns.forEachIndexed { i, t ->
            val id = "legacy-$i"
            val item = when (t.role) {
                "user" -> Item.User(id, t.text, 0)
                "assistant" -> Item.AgentText(id, t.text, true, 0)
                "thinking" -> Item.Thought(id, t.text, true, 0)
                else -> Item.Tool(
                    id, t.role, t.role, categoryOf(t.role),
                    t.text.lineSequence().firstOrNull { it.isNotBlank() }?.trim() ?: "",
                    ToolStatus.Completed, t.text, 0,
                )
            }
            at[id] = rows.size
            rows.add(item)
        }
    }

    private fun upsert(item: Item, onInsert: () -> Unit = {}) {
        dirty = true
        val i = at[item.key]
        if (i != null) { rows[i] = item; return }
        onInsert()
        at[item.key] = rows.size
        rows.add(item)
    }

    /** One echo per send: the first real user_message retires the oldest
     *  one still standing. */
    private fun dropOldestEcho() {
        val i = rows.indexOfFirst { it is Item.User && it.id.startsWith(ECHO) }
        if (i < 0) return
        rows.removeAt(i)
        at.clear()
        rows.forEachIndexed { n, r -> at[r.key] = n }
    }

    companion object {
        private const val ECHO = "\u0000echo-"

        /** What a tool's name says it does, for the legacy path (the spine
         *  itself carries the category). */
        fun categoryOf(tool: String): ToolCategory {
            val t = tool.lowercase()
            return when {
                "read" in t || "notebook" in t || "cat" in t -> ToolCategory.Read
                "edit" in t || "write" in t || "patch" in t || "apply" in t -> ToolCategory.Edit
                "bash" in t || "shell" in t || "exec" in t || "terminal" in t || "command" in t -> ToolCategory.Execute
                "grep" in t || "glob" in t || "search" in t || "find" in t -> ToolCategory.Search
                "fetch" in t || "web" in t || "http" in t || "browser" in t -> ToolCategory.Fetch
                "think" in t || "reason" in t || "plan" in t -> ToolCategory.Think
                else -> ToolCategory.Other
            }
        }
    }
}
