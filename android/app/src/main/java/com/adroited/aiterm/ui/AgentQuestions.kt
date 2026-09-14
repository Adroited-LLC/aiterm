package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.Item
import kotlinx.serialization.json.*

internal data class AgentQuestionOption(val label: String, val description: String)
internal data class AgentQuestion(val title: String, val options: List<AgentQuestionOption>, val multiple: Boolean)

internal fun isQuestionTool(name: String): Boolean = name.substringAfterLast('.').substringAfterLast(':')
    .substringAfterLast("__") in setOf("request_user_input", "request_user_input_async", "AskUserQuestion")

/** Old desktops may send a clipped JSON summary. Keep the request visible even then. */
internal fun agentQuestions(item: Item.Tool): List<AgentQuestion>? {
    if (!isQuestionTool(item.tool)) return null
    if (item.input.length > 64 * 1024) return emptyList()
    val root = runCatching { Json.parseToJsonElement(item.input) as? JsonObject }.getOrNull() ?: return emptyList()
    val rows = root["questions"] as? JsonArray ?: return emptyList()
    if (rows.size > 100) return emptyList()
    return rows.mapNotNull { row ->
        val value = row as? JsonObject ?: return@mapNotNull null
        val title = (value["question"] as? JsonPrimitive)?.contentOrNull
            ?: (value["title"] as? JsonPrimitive)?.contentOrNull ?: return@mapNotNull null
        if (title.isBlank()) return@mapNotNull null
        val options = (value["options"] as? JsonArray)?.mapNotNull { option ->
            when (option) {
                is JsonPrimitive -> option.contentOrNull?.let { AgentQuestionOption(it, "") }
                is JsonObject -> (option["label"] as? JsonPrimitive)?.contentOrNull?.let {
                    AgentQuestionOption(it, (option["description"] as? JsonPrimitive)?.contentOrNull.orEmpty())
                }
                else -> null
            }
        }.orEmpty()
        AgentQuestion(title, options, (value["multiSelect"] as? JsonPrimitive)?.booleanOrNull == true)
    }
}
