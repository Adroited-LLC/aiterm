package com.adroited.aiterm.ui

import kotlinx.serialization.json.Json

private val fileDirective = Regex(""":codex-file-citation\{((?:\s*[a-z_]+="(?:\\.|[^"\\])*")+\s*)\}""")
private val fileAttribute = Regex("""([a-z_]+)=("(?:\\.|[^"\\])*")""")
private val fileFence = Regex("^ {0,3}(`{3,}|~{3,})(.*)$")

/** File citations are presentation metadata, not permission to read an arbitrary host file. */
internal fun assistantFileParts(text: String): List<AssistantPart> {
    val result = mutableListOf<AssistantPart>()
    val prose = StringBuilder()
    var fence: String? = null
    var inlineTicks = 0
    fun flush() {
        if (prose.isNotEmpty()) result += AssistantPart.Markdown(prose.toString())
        prose.clear()
    }
    text.split('\n').forEachIndexed { lineIndex, line ->
        if (lineIndex > 0) prose.append('\n')
        val marker = fileFence.matchEntire(line)
        if (marker != null && inlineTicks == 0) {
            val token = marker.groupValues[1]
            val open = fence
            if (open == null) fence = token
            else if (token.first() == open.first() && token.length >= open.length && marker.groupValues[2].isBlank()) fence = null
            prose.append(line)
        } else if (fence != null || line.startsWith("    ") || line.startsWith('\t') || line.trimStart().startsWith('>')) {
            prose.append(line)
        } else {
            var index = 0
            while (index < line.length) {
                if (line[index] == '\\' && inlineTicks == 0 && index + 1 < line.length) {
                    prose.append(line, index, index + 2)
                    index += 2
                    continue
                }
                if (line[index] == '`') {
                    val end = line.indexOfFirstFrom(index) { it != '`' }
                    val count = end - index
                    if (inlineTicks == 0) inlineTicks = count else if (inlineTicks == count) inlineTicks = 0
                    prose.append(line, index, end)
                    index = end
                    continue
                }
                val match = if (inlineTicks == 0 && line.startsWith(":codex-file-citation{", index)) {
                    fileDirective.matchAt(line, index)
                } else null
                val path = match?.let { directivePath(it.groupValues[1]) }
                if (path != null) {
                    flush()
                    result += AssistantPart.FileCitation(path)
                    index = match.range.last + 1
                } else {
                    prose.append(line[index++])
                }
            }
        }
    }
    flush()
    return result
}

private fun String.indexOfFirstFrom(start: Int, predicate: (Char) -> Boolean): Int {
    for (index in start until length) if (predicate(this[index])) return index
    return length
}

private fun directivePath(attributes: String): String? {
    val values = mutableMapOf<String, String>()
    for (attribute in fileAttribute.findAll(attributes)) {
        val name = attribute.groupValues[1]
        if (name !in setOf("path", "purpose") || name in values) return null
        values[name] = runCatching { Json.decodeFromString<String>(attribute.groupValues[2]) }.getOrNull() ?: return null
    }
    val path = values["path"] ?: return null
    if (path.length > 16_000) return null
    return conversationFilePath(path)?.takeIf { it.substringAfterLast('/').isNotBlank() }
}
