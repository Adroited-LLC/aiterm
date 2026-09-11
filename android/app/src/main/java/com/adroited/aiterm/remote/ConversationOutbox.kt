package com.adroited.aiterm.remote

import java.util.UUID

/** Desktop acceptance is distinct from the agent recording a user message. */
data class PendingConversationPrompt(
    val id: String,
    val sessionId: String,
    val text: String,
    val accepted: Boolean = false,
)

/** Called under RemoteClient's lifecycle lock; never expires an unconfirmed prompt silently. */
internal class ConversationOutbox {
    private data class Entry(val prompt: PendingConversationPrompt, val epoch: Long, val after: Long)
    private val consumed = linkedSetOf<Triple<String, Long, Long>>()
    private val entries = mutableListOf<Entry>()
    val prompts: List<PendingConversationPrompt> get() = entries.map { it.prompt }

    fun begin(sessionId: String, text: String, epoch: Long, after: Long): String {
        val prompt = PendingConversationPrompt(UUID.randomUUID().toString(), sessionId, text)
        entries += Entry(prompt, epoch, after)
        return prompt.id
    }

    fun accepted(id: String) {
        val index = entries.indexOfFirst { it.prompt.id == id }
        if (index >= 0) entries[index] = entries[index].let { it.copy(prompt = it.prompt.copy(accepted = true)) }
    }

    fun remove(id: String) { entries.removeAll { it.prompt.id == id } }

    fun reconcile(sessionId: String, page: SpineConversationPage) {
        page.events.sortedBy { it.seq }.filter { it.kind == "user_message" }.forEach { event ->
            val receipt = Triple(sessionId, page.epoch, event.seq)
            if (receipt in consumed) return@forEach
            // Sequence numbers only establish causality inside the same desktop epoch.
            // A restart must not make an old, identical prompt look newly accepted.
            val eligible = entries.filter { it.prompt.sessionId == sessionId &&
                it.epoch == page.epoch && event.seq > it.after }
            val text = normalize(event.text.orEmpty())
            val match = eligible.firstOrNull { normalize(it.prompt.text) == text }
            if (match != null) { entries.remove(match); consumed += receipt }
            else {
                // A CLI may queue messages with newlines, or append a later paste directly
                // to an unsubmitted draft (including an attachment path followed by ".").
                // Match complete, consecutive prompts only: a substring of another user
                // message or an assistant echo is not proof that we delivered this prompt.
                val group = eligible.indices.firstNotNullOfOrNull { start ->
                    (2..eligible.size - start).firstNotNullOfOrNull { count ->
                        val candidates = eligible.subList(start, start + count)
                        candidates.takeIf {
                            listOf("", "\n", "\n\n").any { separator ->
                                normalize(candidates.joinToString(separator) { it.prompt.text }) == text
                            }
                        }
                    }
                }
                if (group != null) {
                    entries.removeAll(group.toSet())
                    consumed += receipt
                }
            }
        }
        consumed.removeAll { receipt -> entries.none { it.prompt.sessionId == receipt.first } }
    }

    private fun normalize(text: String) = text.replace("\r\n", "\n").trim()
}
