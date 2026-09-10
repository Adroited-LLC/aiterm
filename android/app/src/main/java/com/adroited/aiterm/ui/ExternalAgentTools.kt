package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.ToolCategory
import com.adroited.aiterm.remote.ToolStatus

internal sealed interface ExternalToolEvent {
    data class Call(val tool: String, val input: String) : ExternalToolEvent
    data class Result(val output: String, val failed: Boolean) : ExternalToolEvent
}

private val externalToolEnvelope = Regex(
    "\\[external_agent_tool_(call|result)(?:: ([^\\]\\r\\n]+))?]\\r?\\n(.*?)" +
        "^\\[/external_agent_tool_\\1]",
    setOf(RegexOption.DOT_MATCHES_ALL, RegexOption.MULTILINE),
)
private val toolUseError = Regex("<tool_use_error>(.*?)</tool_use_error>", RegexOption.DOT_MATCHES_ALL)

/** Recognize whole machine messages. Never strip wrappers out of prose or code examples. */
internal fun externalToolEvents(text: String): List<ExternalToolEvent>? {
    if (!text.trimStart().startsWith("[external_agent_tool_")) return null
    val firstLine = text.lineSequence().firstOrNull { it.isNotBlank() }.orEmpty()
    if (firstLine.startsWith("    ") || firstLine.startsWith('\t')) return null
    val events = mutableListOf<ExternalToolEvent>()
    var end = 0
    for (match in externalToolEnvelope.findAll(text)) {
        if (text.substring(end, match.range.first).isNotBlank()) return null
        val (kind, label, rawContent) = match.destructured
        val content = rawContent.removeSuffix("\n").removeSuffix("\r")
        when (kind) {
            "call" -> {
                if (label.isBlank()) return null
                events += ExternalToolEvent.Call(label.trim(), content)
            }
            "result" -> {
                // These are the two result headers observed in real rollouts.
                // An unfamiliar envelope remains visible rather than losing data.
                if (label.isNotEmpty() && label != "error") return null
                val error = toolUseError.matchEntire(content.trim())
                events += ExternalToolEvent.Result(
                    error?.groupValues?.get(1) ?: content,
                    label == "error" || error != null || content.trimStart().startsWith("Error:", ignoreCase = true),
                )
            }
        }
        end = match.range.last + 1
    }
    return events.takeIf { it.isNotEmpty() && text.substring(end).isBlank() }
}

internal fun externalToolCategory(name: String): ToolCategory = when (name.substringAfterLast('.').lowercase()) {
    "edit", "write", "multiedit", "notebookedit", "apply_patch", "write_file" -> ToolCategory.Edit
    "read", "readfile", "read_file", "view_image" -> ToolCategory.Read
    "bash", "shell", "exec", "exec_command", "write_stdin", "wait", "monitor" -> ToolCategory.Execute
    "grep", "glob", "search", "websearch", "web_search", "toolsearch" -> ToolCategory.Search
    "fetch", "webfetch", "open_page" -> ToolCategory.Fetch
    "agent", "sendmessage", "task", "taskcreate", "taskupdate", "taskstop", "tasklist", "taskget",
    "todowrite", "askuserquestion", "skill", "spawn_agent", "send_message", "followup_task",
    "wait_agent", "list_agents", "interrupt_agent", "request_user_input", "request_user_input_async" -> ToolCategory.Think
    else -> ToolCategory.Other // MCP, plugins, artifacts, and future tools keep their actual names.
}

/**
 * Old external-agent transcripts omit call ids. Join only an immediately
 * adjacent, unambiguous call/result pair; never cross a user/turn/prose boundary
 * or guess among parallel calls. The original store and wire items stay intact.
 */
internal fun normalizeExternalTools(items: List<Item>): List<Item> {
    val result = mutableListOf<Item>()
    var pending: Int? = null
    var ambiguous = false
    items.forEach { item ->
        val events = (item as? Item.AgentText)?.let { externalToolEvents(it.text) }
        if (events == null) {
            pending = null
            ambiguous = false
            result += item
            return@forEach
        }
        events.forEachIndexed { index, event ->
            val key = "${item.key}:external-tool:$index"
            val ts = (item as Item.AgentText).ts
            when (event) {
                is ExternalToolEvent.Call -> {
                    if (pending != null) ambiguous = true
                    pending = if (ambiguous) null else result.size
                    result += Item.Tool(
                        key, event.tool, "External agent · ${event.tool}", externalToolCategory(event.tool),
                        event.input, ToolStatus.Recorded, null, ts,
                    )
                }
                is ExternalToolEvent.Result -> {
                    val status = if (event.failed) ToolStatus.Failed else ToolStatus.Completed
                    val target = pending
                    if (target != null && !ambiguous) {
                        val call = result[target] as Item.Tool
                        result[target] = call.copy(status = status, output = event.output)
                    } else {
                        result += Item.Tool(key, "external_agent_tool_result", "External agent · Tool result",
                            ToolCategory.Other, "", status, event.output, ts)
                    }
                    pending = null
                }
            }
        }
    }
    return result
}

internal fun externalAgentTools(text: String, messageId: String, ts: Long): List<Item.Tool>? {
    if (externalToolEvents(text) == null) return null
    return normalizeExternalTools(listOf(Item.AgentText(messageId, text, true, ts))).filterIsInstance<Item.Tool>()
}

internal fun assistantCopyText(text: String): String =
    externalAgentTools(text, "copy", 0L)?.joinToString("\n\n") { tool ->
        listOf(tool.title, tool.input, tool.output.orEmpty()).filter(String::isNotBlank).joinToString("\n\n")
    } ?: assistantContent(text).copyText()
