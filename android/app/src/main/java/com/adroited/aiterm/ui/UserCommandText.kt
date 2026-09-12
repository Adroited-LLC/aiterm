package com.adroited.aiterm.ui

private val commandField = Regex("""<(command-name|command-message|command-args)>([\s\S]*?)</\1>""")
private val slashCommandName = Regex("""/[A-Za-z][A-Za-z0-9_:/.-]*""")

/** Presentation only: collapse a complete CLI command envelope, not XML in prose or code. */
internal fun userCommandText(text: String): String {
    val envelope = text.trim().removePrefix("› ").trimStart()
    if (!envelope.startsWith("<command-")) return text
    val fields = mutableMapOf<String, String>()
    var position = 0
    for (match in commandField.findAll(envelope)) {
        if (envelope.substring(position, match.range.first).isNotBlank()) return text
        val name = match.groupValues[1]
        if (fields.put(name, match.groupValues[2]) != null) return text
        position = match.range.last + 1
    }
    if (envelope.substring(position).isNotBlank()) return text
    val command = fields["command-name"]?.trim() ?: return text
    if (!slashCommandName.matches(command)) return text
    val arguments = fields["command-args"]?.trim().orEmpty()
    return if (arguments.isEmpty()) command else "$command $arguments"
}
