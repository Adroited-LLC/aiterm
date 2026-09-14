package com.adroited.aiterm.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.OutlinedButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp

internal data class MemoryReference(val source: String, val note: String)
internal data class AssistantContent(val body: String, val references: List<MemoryReference>) {
    fun copyText(): String = if (references.isEmpty()) assistantFollowupPlain(body) else buildString {
        append(assistantFollowupPlain(body).trimEnd())
        if (isNotEmpty()) append("\n\n")
        append("Memory references:\n")
        append(references.joinToString("\n") { "${it.source} — ${it.note}" })
    }
}

private val memoryFooter = Regex(
    "<oai-mem-citation>\\s*<citation_entries>\\s*(.*?)\\s*</citation_entries>" +
        "\\s*(?:<rollout_ids>.*?</rollout_ids>\\s*)?</oai-mem-citation>\\s*$",
    RegexOption.DOT_MATCHES_ALL,
)
private val memoryEntry = Regex("^(.+?):([0-9]+)-([0-9]+)\\|note=\\[(.*)]$")
private val codeFence = Regex("^ {0,3}(`{3,}|~{3,})(.*)$")

/** Interpret only a complete assistant footer. User messages and source files never use this path. */
internal fun assistantContent(text: String): AssistantContent {
    val original = AssistantContent(text, emptyList())
    val footer = memoryFooter.find(text) ?: return original
    val prefix = text.substring(0, footer.range.first)
    // An example inside Markdown code is content, not a metadata footer.
    var fence: String? = null
    for (line in prefix.lineSequence()) {
        val match = codeFence.matchEntire(line) ?: continue
        val marker = match.groupValues[1]
        val open = fence
        if (open == null) fence = marker
        else if (marker.first() == open.first() && marker.length >= open.length && match.groupValues[2].isBlank()) fence = null
    }
    if (fence != null) return original
    val currentLine = prefix.substringAfterLast('\n')
    if (currentLine.startsWith("    ") || currentLine.startsWith('\t') || currentLine.contains('`') || currentLine.trimStart().startsWith('>')) return original
    val references = mutableListOf<MemoryReference>()
    for (line in footer.groupValues[1].lineSequence().filter { it.isNotBlank() }) {
        val entry = memoryEntry.matchEntire(line.trim()) ?: return original
        val (file, start, end, note) = entry.destructured
        val first = start.toLongOrNull() ?: return original
        val last = end.toLongOrNull() ?: return original
        if (first < 1 || last < first) return original
        references += MemoryReference(if (first == last) "$file:$start" else "$file:$start–$end", note)
    }
    if (references.isEmpty()) return original
    return AssistantContent(prefix.trimEnd(), references)
}

@Composable
internal fun AssistantMarkdown(text: String) {
    val tools = remember(text) { externalAgentTools(text, "assistant", 0L) }
    if (tools != null) {
        Column { tools.forEach { SpineToolCard(it) } }
        return
    }
    val content = remember(text) { assistantContent(text) }
    var expanded by rememberSaveable { mutableStateOf(false) }
    val parts = remember(content.body) { assistantParts(content.body) }
    val onFollowup = LocalAssistantFollowup.current
    val uriHandler = LocalUriHandler.current
    Column {
        parts.forEach { part ->
            when (part) {
                is AssistantPart.Markdown -> if (part.text.isNotBlank()) ConversationMarkdown(part.text)
                is AssistantPart.FileCitation -> OutlinedButton(
                    onClick = { uriHandler.openUri(part.path) },
                    modifier = Modifier.padding(vertical = 4.dp),
                ) { Text(part.label) }
                is AssistantPart.Followup -> if (onFollowup != null) {
                    OutlinedButton(onClick = { onFollowup(part.prompt) }, modifier = Modifier.padding(vertical = 4.dp)) {
                        Text(part.label)
                    }
                } else {
                    Text(part.label, style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(top = 8.dp))
                    Text(part.prompt, style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
        if (content.references.isNotEmpty()) {
            Text(
                "Memory references · ${content.references.size} ${if (expanded) "⌃" else "⌄"}",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.clickable { expanded = !expanded }
                    .semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" }
                    .padding(vertical = 10.dp),
            )
            if (expanded) {
                content.references.forEach { reference ->
                    Column(Modifier.padding(bottom = 8.dp)) {
                        Text(reference.source, style = MaterialTheme.typography.labelMedium)
                        Text(reference.note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}
