package com.adroited.aiterm.ui

import androidx.compose.ui.text.input.TextFieldValue

internal data class TerminalComposerUpdate(
    val state: TerminalComposerState,
    val outbound: List<String> = emptyList(),
)

internal data class TerminalComposerState(
    val expanded: Boolean = false,
    val value: TextFieldValue = TextFieldValue(),
) {
    fun open(): TerminalComposerState = copy(expanded = true)

    fun close(): TerminalComposerState = copy(expanded = false)

    fun updateValue(next: TextFieldValue) = TerminalComposerUpdate(copy(value = next))

    fun sendText(bracketedPaste: Boolean = false): TerminalComposerUpdate {
        val outbound = buildList {
            if (value.text.isNotEmpty()) {
                add(
                    if (bracketedPaste) "\u001b[200~${value.text}\u001b[201~"
                    else value.text,
                )
            }
            add("\r")
        }
        return TerminalComposerUpdate(
            state = copy(expanded = false, value = TextFieldValue()),
            outbound = outbound,
        )
    }
}
