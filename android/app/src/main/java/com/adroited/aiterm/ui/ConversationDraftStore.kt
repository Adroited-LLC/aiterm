package com.adroited.aiterm.ui

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** Session switching preserves both unsent text and private attachment ownership. */
internal class ConversationDraftStore {
    class Draft {
        var text by mutableStateOf("")
        var attachments by mutableStateOf(TerminalAttachmentDraft())
    }
    private val drafts = mutableMapOf<String, Draft>()
    fun forSession(id: String): Draft = drafts.getOrPut(id) { Draft() }
    fun clear() {
        drafts.values.forEach { draft -> draft.attachments.items.forEach { it.image.file.delete() } }
        drafts.clear()
    }
}
