package com.adroited.aiterm.ui

import androidx.compose.ui.text.input.TextFieldValue

internal data class TerminalComposerUpdate(
    val state: TerminalComposerState,
    val outbound: String? = null,
)

internal data class TerminalComposerState(
    val expanded: Boolean = false,
    val direct: Boolean = false,
    private val textValue: TextFieldValue = TextFieldValue(),
    private val directValue: TextFieldValue = TextFieldValue(),
) {
    val visibleValue: TextFieldValue
        get() = if (direct) directValue else textValue

    fun open(): TerminalComposerState = copy(expanded = true)

    fun close(): TerminalComposerState = copy(expanded = false)

    fun toggleDirect(): TerminalComposerState = copy(direct = !direct)

    fun updateValue(next: TextFieldValue): TerminalComposerUpdate {
        if (!direct) {
            return TerminalComposerUpdate(copy(textValue = next))
        }
        if (next.composition != null || next.text.isEmpty()) {
            return TerminalComposerUpdate(copy(directValue = next))
        }
        return TerminalComposerUpdate(
            state = copy(directValue = TextFieldValue()),
            outbound = next.text.replace("\n", "\r"),
        )
    }

    fun sendText(): TerminalComposerUpdate {
        if (direct || textValue.text.isEmpty()) return TerminalComposerUpdate(this)
        return TerminalComposerUpdate(
            state = copy(expanded = false, textValue = TextFieldValue()),
            outbound = textValue.text + "\r",
        )
    }
}
