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
    val events: List<SpineEventWire>,
)

internal data class SpineConversationPage(
    val epoch: Long,
    val live: Boolean,
    val hasMore: Boolean,
    val events: List<SpineEventWire>,
)

/** Incrementally projects spine events onto the conversation rows already used by Compose. */
internal class SpineConversationStore {
    private val rows = ArrayList<RemotePreviewMessage>()
    private val rowIds = ArrayList<String>()
    private val rowIndex = HashMap<String, Int>()

    var epoch: Long = 0
        private set
    var lastSeq: Long = 0
        private set
    var live: Boolean = false
        private set
    var phase: String? = null
        private set

    fun apply(page: SpineConversationPage): List<RemotePreviewMessage> {
        if (epoch != 0L && page.epoch != 0L && page.epoch != epoch) clear()
        if (page.epoch != 0L) epoch = page.epoch
        live = page.live
        page.events.sortedBy { it.seq }.forEach { event ->
            if (event.seq <= lastSeq) return@forEach
            if (event.kind == "reset") clearRows()
            else applyEvent(event)
            lastSeq = event.seq
        }
        return rows.toList()
    }

    fun clear() {
        clearRows()
        epoch = 0
        lastSeq = 0
        live = false
        phase = null
    }

    private fun clearRows() {
        rows.clear()
        rowIds.clear()
        rowIndex.clear()
    }

    private fun applyEvent(event: SpineEventWire) {
        when (event.kind) {
            "user_message" -> upsert(event, "user", event.text.orEmpty())
            "agent_text" -> upsert(event, "assistant", event.text.orEmpty())
            "agent_thought" -> upsert(event, "thinking", event.text.orEmpty())
            "tool_call" -> {
                val title = event.title?.takeIf(String::isNotBlank)
                    ?: event.tool?.takeIf(String::isNotBlank)
                    ?: "Tool"
                val detail = event.input?.takeIf(String::isNotBlank)
                upsert(event, "tool.${event.category ?: "other"}", listOfNotNull(title, detail).joinToString("\n"))
            }
            "tool_call_update" -> updateTool(event)
            "phase" -> phase = event.phase
        }
    }

    private fun upsert(event: SpineEventWire, role: String, text: String) {
        val id = event.id ?: return
        val message = RemotePreviewMessage(role, text, event.ts.takeIf { it > 0 }?.toString())
        val index = rowIndex[id]
        if (index == null) {
            rowIndex[id] = rows.size
            rowIds += id
            rows += message
        } else {
            rows[index] = message
        }
    }

    private fun updateTool(event: SpineEventWire) {
        val id = event.id ?: return
        val index = rowIndex[id] ?: return
        val current = rows[index]
        val status = event.status?.replace('_', ' ')?.takeIf(String::isNotBlank)
        val output = event.output?.takeIf(String::isNotBlank)
        val suffix = listOfNotNull(status, output).joinToString("\n")
        if (suffix.isNotEmpty()) {
            val base = current.text.substringBefore("\n$status")
            rows[index] = current.copy(text = "$base\n$suffix")
        }
    }
}
