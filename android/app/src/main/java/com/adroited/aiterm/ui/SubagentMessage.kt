package com.adroited.aiterm.ui

internal data class SubagentMessage(val name: String, val status: String, val payload: String) {
    val headline: String get() = "$name · $status"
}

private const val AGENT_PATH = "/root(?:/[A-Za-z0-9_-]+)*"
private val subagentEnvelope = Regex(
    "($AGENT_PATH) → ($AGENT_PATH)\\r?\\n" +
        "Message Type: (MESSAGE|FINAL_ANSWER|NEW_TASK)\\r?\\n" +
        "Task name: ($AGENT_PATH)\\r?\\n" +
        "Sender: ($AGENT_PATH)\\r?\\n" +
        "Payload:(?:\\r?\\n([\\s\\S]*))?",
)

/** Recognize only the complete routing envelope emitted by the desktop Codex reader. */
internal fun parseSubagentMessage(text: String): SubagentMessage? {
    val match = subagentEnvelope.matchEntire(text) ?: return null
    val (from, to, type, task, sender, payload) = match.destructured
    if (from != sender || to != task) return null
    val name = (if (type == "NEW_TASK") to else from).substringAfterLast('/')
    val status = when (type) {
        "FINAL_ANSWER" -> "Completed"
        "NEW_TASK" -> "Task"
        else -> "Update"
    }
    return SubagentMessage(name, status, payload)
}
