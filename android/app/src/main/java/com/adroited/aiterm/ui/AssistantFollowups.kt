package com.adroited.aiterm.ui

import androidx.compose.runtime.staticCompositionLocalOf
import kotlinx.serialization.json.Json

internal sealed interface AssistantPart {
    data class Markdown(val text: String) : AssistantPart
    data class Followup(val label: String, val prompt: String) : AssistantPart
}

internal val LocalAssistantFollowup = staticCompositionLocalOf<((String) -> Unit)?> { null }
private val followupLine = Regex("""^ {0,3}(?:(?:[-+*•]|[0-9]+[.)])\s+)?:codex-followup\[((?:\\.|[^\]\\])+)\]\{prompt=("(?:\\.|[^"\\])*")\}\s*$""")
private val followupFence = Regex("^ {0,3}(`{3,}|~{3,})(.*)$")

/** Interpret complete standalone directives only, never fenced/quoted/inline examples. */
internal fun assistantParts(text: String): List<AssistantPart> {
    val result = mutableListOf<AssistantPart>()
    val lines = mutableListOf<String>()
    var fence: String? = null
    fun flush() {
        if (lines.isNotEmpty()) result += AssistantPart.Markdown(lines.joinToString("\n"))
        lines.clear()
    }
    for (line in text.split('\n')) {
        val marker = followupFence.matchEntire(line)
        if (marker != null) {
            val token = marker.groupValues[1]
            val open = fence
            if (open == null) fence = token
            else if (token.first() == open.first() && token.length >= open.length && marker.groupValues[2].isBlank()) fence = null
            lines += line
            continue
        }
        val match = if (fence == null) followupLine.matchEntire(line) else null
        val prompt = match?.groupValues?.get(2)?.let { runCatching { Json.decodeFromString<String>(it) }.getOrNull() }
        val label = match?.groupValues?.get(1)?.replace(Regex("\\\\(.)"), "$1")?.trim()
        if (prompt.isNullOrBlank() || label.isNullOrBlank() || label.length > 300 || prompt.length > 16_000) {
            lines += line
        } else {
            // Some copied responses put the list marker on its own line.
            if (lines.lastOrNull()?.trim() in listOf("•", "-", "*", "+")) lines.removeAt(lines.lastIndex)
            flush()
            result += AssistantPart.Followup(label, prompt)
        }
    }
    flush()
    return result
}

internal fun assistantFollowupPlain(text: String): String = assistantParts(text).joinToString("\n") {
    when (it) {
        is AssistantPart.Markdown -> it.text
        is AssistantPart.Followup -> "${it.label}\n${it.prompt}"
    }
}

internal fun draftWithFollowup(draft: String, prompt: String): String =
    if (draft.isBlank()) prompt else "$draft\n\n$prompt"
