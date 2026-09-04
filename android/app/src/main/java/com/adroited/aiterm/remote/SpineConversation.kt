package com.adroited.aiterm.remote

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/** The transport-neutral conversation vocabulary produced by the desktop spine. */
@Serializable
internal data class SpineEventWire(
    val seq: Long,
    val epoch: Long,
    @SerialName("session_id") val sessionId: String,
    val agent: String,
    val ts: Long,
    val kind: String,
    val id: String? = null,
    val text: String? = null,
    val done: Boolean? = null,
    val tool: String? = null,
    val title: String? = null,
    val category: String? = null,
    val input: String? = null,
    val status: String? = null,
    val output: String? = null,
    val turn: String? = null,
    val reason: String? = null,
    val phase: String? = null,
    val detail: String? = null,
)

@Serializable
internal data class SpineSnapshotWire(
    val epoch: Long,
    val live: Boolean,
    @SerialName("has_more") val hasMore: Boolean,
    @SerialName("oldest_seq") val oldestSeq: Long = 0L,
    @SerialName("latest_seq") val latestSeq: Long = 0L,
    @SerialName("turn_open") val turnOpen: Boolean? = null,
    val events: List<SpineEventWire>,
)

internal data class SpineConversationPage(
    val epoch: Long,
    val live: Boolean,
    val hasMore: Boolean,
    val oldestSeq: Long = 0L,
    val latestSeq: Long = 0L,
    val turnOpen: Boolean? = null,
    val events: List<SpineEventWire>,
)

enum class ToolCategory(val wire: String) {
    Read("read"), Edit("edit"), Execute("execute"), Search("search"), Fetch("fetch"), Think("think"), Other("other");

    companion object {
        /** A newer desktop category must degrade to a drawable generic tool. */
        fun from(value: String?): ToolCategory = entries.firstOrNull { it.wire == value } ?: Other
    }
}

enum class ToolStatus(val wire: String) {
    Pending("pending"), Running("running"), Completed("completed"), Failed("failed"), Cancelled("cancelled");

    val settled: Boolean get() = this == Completed || this == Failed || this == Cancelled

    companion object {
        fun from(value: String?): ToolStatus = entries.firstOrNull { it.wire == value } ?: Pending
    }
}

enum class SpinePhase(val wire: String) {
    Working("working"), NeedsYou("needs_you"), Idle("idle");

    companion object {
        fun from(value: String?): SpinePhase = entries.firstOrNull { it.wire == value } ?: Working
    }
}

sealed interface SpineKind {
    data class UserMessage(val id: String, val text: String) : SpineKind
    data class AgentText(val id: String, val text: String, val done: Boolean) : SpineKind
    data class AgentThought(val id: String, val text: String, val done: Boolean) : SpineKind
    data class ToolCall(
        val id: String, val tool: String, val title: String, val category: ToolCategory,
        val input: String, val status: ToolStatus,
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
)

/** One typed, stable-key row in the API conversation. */
sealed interface Item {
    val key: String

    data class User(val id: String, val text: String, val ts: Long) : Item { override val key: String get() = id }
    data class AgentText(val id: String, val text: String, val done: Boolean, val ts: Long) : Item { override val key: String get() = id }
    data class Thought(val id: String, val text: String, val done: Boolean, val ts: Long) : Item { override val key: String get() = id }
    data class Tool(
        val id: String, val tool: String, val title: String, val category: ToolCategory,
        val input: String, val status: ToolStatus, val output: String?, val ts: Long,
    ) : Item { override val key: String get() = id }
    data class TurnEnd(val turn: String, val reason: String) : Item { override val key: String get() = TURN_KEY_PREFIX + turn }

    companion object { const val TURN_KEY_PREFIX = "\u0000turn:" }
}

enum class Offer { Applied, Gap, Stale, EpochChanged }

/** Incrementally applies spine pages without rebuilding unchanged rows. */
internal class SpineConversationStore {
    private val rows = ArrayList<Item>()
    private val rowIndex = HashMap<String, Int>()
    private var snapshot: List<Item> = emptyList()
    private var dirty = false

    var epoch: Long = 0L; private set
    var lastSeq: Long = 0L; private set
    var live: Boolean = true; private set
    var phase: SpinePhase = SpinePhase.Idle; private set
    var phaseDetail: String = ""; private set
    var phaseSeen: Boolean = false; private set
    var currentTurn: String? = null; private set
    var turnOpen: Boolean? = null; private set

    val items: List<Item>
        get() {
            if (dirty) { snapshot = ArrayList(rows); dirty = false }
            return snapshot
        }

    fun apply(page: SpineConversationPage): List<Item> {
        if (epoch != 0L && page.epoch != 0L && page.epoch != epoch) clear()
        // A bounded desktop ring may have advanced past a phone that slept.
        // Keeping the phone's older rows while applying a discontinuous tail
        // looks plausible but is not truthful, so rebuild from the oldest
        // page the desktop can still supply.
        if (lastSeq > 0L && page.oldestSeq > 0L && lastSeq + 1L < page.oldestSeq) clear()
        if (page.epoch != 0L) epoch = page.epoch
        live = page.live

        // Advance across every wire event, including a future event kind this
        // version cannot draw. Otherwise one unknown event at the head of the
        // next page pins the cursor and every poll downloads it forever.
        page.events.sortedBy { it.seq }.forEach { wire ->
            if (wire.seq <= lastSeq) return@forEach
            if (lastSeq > 0L && wire.seq > lastSeq + 1L) {
                rows.clear(); rowIndex.clear(); dirty = true
                phase = SpinePhase.Idle; phaseDetail = ""; phaseSeen = false
                currentTurn = null; turnOpen = null
            }
            parse(wire)?.let(::applyEvent)
            lastSeq = wire.seq
        }
        // The current gate is returned atomically with the ring bounds. A
        // long turn may evict a boundary event, but cannot evict this state.
        page.turnOpen?.let { authoritative ->
            turnOpen = authoritative
            if (!authoritative) currentTurn = null
        }
        return items
    }

    fun clear() {
        rows.clear(); rowIndex.clear(); dirty = true
        epoch = 0L; lastSeq = 0L; live = true
        phase = SpinePhase.Idle; phaseDetail = ""; phaseSeen = false
        currentTurn = null; turnOpen = null
    }

    fun tool(id: String): Item.Tool? = rowIndex[id]?.let { rows[it] as? Item.Tool }

    fun offer(event: SpineEvent): Offer {
        if (epoch != 0L && event.epoch != 0L && event.epoch != epoch) return Offer.EpochChanged
        if (epoch == 0L) epoch = event.epoch
        if (event.seq <= lastSeq) return Offer.Stale
        if (event.seq > lastSeq + 1) return Offer.Gap
        applyEvent(event); lastSeq = event.seq
        return Offer.Applied
    }

    fun replay(epoch: Long, live: Boolean, events: List<SpineEvent>) {
        if (this.epoch != 0L && epoch != 0L && epoch != this.epoch) clear()
        if (epoch != 0L) this.epoch = epoch
        this.live = live
        replay(events)
    }

    fun replay(events: List<SpineEvent>) {
        events.sortedBy { it.seq }.forEach { event ->
            if (event.seq <= lastSeq) return@forEach
            if (epoch == 0L) epoch = event.epoch
            applyEvent(event); lastSeq = event.seq
        }
    }

    /** Adds an immediate local bubble; the desktop's real event retires the oldest echo. */
    fun echoUser(text: String, ts: Long) {
        upsert(Item.User(ECHO_PREFIX + ts + "-" + rows.size, text, ts))
    }

    /** Maps an older desktop transcript onto the same typed rows. */
    fun legacy(messages: List<RemotePreviewMessage>) {
        rows.clear(); rowIndex.clear(); dirty = true; live = false
        phase = SpinePhase.Idle; phaseDetail = ""; phaseSeen = false
        currentTurn = null; turnOpen = null
        messages.forEachIndexed { index, message ->
            val id = "legacy-$index"
            val timestamp = message.at?.toLongOrNull() ?: 0L
            val item = when (message.role) {
                "user" -> Item.User(id, message.text, timestamp)
                "assistant" -> Item.AgentText(id, message.text, true, timestamp)
                "thinking" -> Item.Thought(id, message.text, true, timestamp)
                else -> Item.Tool(
                    id, message.role, message.role, categoryOf(message.role),
                    message.text.lineSequence().firstOrNull { it.isNotBlank() }?.trim().orEmpty(),
                    ToolStatus.Completed, message.text, timestamp,
                )
            }
            rowIndex[id] = rows.size; rows += item
        }
    }

    fun asPreviewMessages(): List<RemotePreviewMessage> = items.mapNotNull { item ->
        when (item) {
            is Item.User -> RemotePreviewMessage("user", item.text, item.ts.takeIf { it > 0 }?.toString())
            is Item.AgentText -> RemotePreviewMessage("assistant", item.text, item.ts.takeIf { it > 0 }?.toString())
            is Item.Thought -> RemotePreviewMessage("thinking", item.text, item.ts.takeIf { it > 0 }?.toString())
            is Item.Tool -> RemotePreviewMessage(
                "tool.${item.category.wire}",
                listOfNotNull(
                    item.title.takeIf(String::isNotBlank), item.input.takeIf(String::isNotBlank),
                    item.status.wire.replace('_', ' '), item.output?.takeIf(String::isNotBlank),
                ).joinToString("\n"),
                item.ts.takeIf { it > 0 }?.toString(),
            )
            is Item.TurnEnd -> null
        }
    }

    private fun applyEvent(event: SpineEvent) {
        when (val kind = event.kind) {
            is SpineKind.UserMessage -> upsert(Item.User(kind.id, kind.text, event.ts)) { dropOldestEcho() }
            is SpineKind.AgentText -> upsert(Item.AgentText(kind.id, kind.text, kind.done, event.ts))
            is SpineKind.AgentThought -> upsert(Item.Thought(kind.id, kind.text, kind.done, event.ts))
            is SpineKind.ToolCall -> upsert(Item.Tool(kind.id, kind.tool, kind.title, kind.category, kind.input, kind.status, tool(kind.id)?.output, event.ts))
            is SpineKind.ToolCallUpdate -> {
                val current = tool(kind.id) ?: return
                upsert(current.copy(status = kind.status, output = kind.output ?: current.output))
            }
            is SpineKind.TurnStarted -> { currentTurn = kind.turn; turnOpen = true }
            is SpineKind.TurnEnded -> {
                currentTurn = null
                turnOpen = false
                upsert(Item.TurnEnd(kind.turn, kind.reason))
            }
            is SpineKind.PhaseChanged -> { phase = kind.phase; phaseDetail = kind.detail; phaseSeen = true }
            SpineKind.Reset -> {
                rows.clear(); rowIndex.clear(); dirty = true
                phase = SpinePhase.Idle; phaseDetail = ""; phaseSeen = false
                currentTurn = null; turnOpen = null
            }
        }
    }

    private fun upsert(item: Item, onInsert: () -> Unit = {}) {
        dirty = true
        val index = rowIndex[item.key]
        if (index == null) {
            onInsert()
            rowIndex[item.key] = rows.size
            rows += item
        } else {
            rows[index] = item
        }
    }

    private fun dropOldestEcho() {
        val index = rows.indexOfFirst { it is Item.User && it.id.startsWith(ECHO_PREFIX) }
        if (index < 0) return
        rows.removeAt(index)
        rowIndex.clear()
        rows.forEachIndexed { row, item -> rowIndex[item.key] = row }
    }

    private fun parse(event: SpineEventWire): SpineEvent? {
        val kind = when (event.kind) {
            "user_message" -> SpineKind.UserMessage(event.id ?: return null, event.text.orEmpty())
            "agent_text" -> SpineKind.AgentText(event.id ?: return null, event.text.orEmpty(), event.done ?: false)
            "agent_thought" -> SpineKind.AgentThought(event.id ?: return null, event.text.orEmpty(), event.done ?: false)
            "tool_call" -> SpineKind.ToolCall(event.id ?: return null, event.tool.orEmpty(), event.title.orEmpty(), ToolCategory.from(event.category), event.input.orEmpty(), ToolStatus.from(event.status))
            "tool_call_update" -> SpineKind.ToolCallUpdate(event.id ?: return null, ToolStatus.from(event.status), event.output)
            "turn_started" -> SpineKind.TurnStarted(event.turn.orEmpty())
            "turn_ended" -> SpineKind.TurnEnded(event.turn.orEmpty(), event.reason ?: "unknown")
            "phase" -> SpineKind.PhaseChanged(SpinePhase.from(event.phase), event.detail.orEmpty())
            "reset" -> SpineKind.Reset
            else -> return null
        }
        return SpineEvent(event.seq, event.epoch, event.sessionId, event.agent, event.ts, kind)
    }

    companion object {
        private const val ECHO_PREFIX = "\u0000echo-"

        fun categoryOf(tool: String): ToolCategory {
            val value = tool.lowercase()
            return when {
                "read" in value || "notebook" in value || "cat" in value -> ToolCategory.Read
                "edit" in value || "write" in value || "patch" in value || "apply" in value -> ToolCategory.Edit
                "bash" in value || "shell" in value || "exec" in value || "terminal" in value || "command" in value -> ToolCategory.Execute
                "grep" in value || "glob" in value || "search" in value || "find" in value -> ToolCategory.Search
                "fetch" in value || "web" in value || "http" in value || "browser" in value -> ToolCategory.Fetch
                "think" in value || "reason" in value || "plan" in value -> ToolCategory.Think
                else -> ToolCategory.Other
            }
        }
    }
}
