package com.adroited.aiterm.ui

import java.io.File
import org.junit.Assert.*
import org.junit.Test

class TerminalFileAttachmentTest {
    @Test fun fileNamesKeepReadableNamesWithoutPathsOrPromptControls() {
        assertEquals("Wayland log résumé.pdf", safeAttachmentName("Wayland log résumé.pdf"))
        assertEquals(".._logs_bad_name_.txt", safeAttachmentName("../logs\nbad:name?.txt"))
        assertEquals("attachment", safeAttachmentName(".."))
        assertTrue(safeAttachmentName("é".repeat(200)).toByteArray().size <= 180)
    }

    @Test fun documentDraftSharesLimitsAndProgressWithPhotos() {
        val file = TerminalAttachment.from(PreparedTerminalFile("pdf", File("/private/pdf"), "log.pdf", 20, ByteArray(32) { 1 }))
        val photo = NormalizedTerminalImage("photo", File("/private/photo.jpg"), 2, 2, 30, ByteArray(32) { 2 })
        val draft = TerminalAttachmentDraft().add(photo).draft.add(file).draft
        assertEquals(50, draft.totalBytes)
        assertEquals("log.pdf", draft.items.last().image.asRemoteUploadSource().fileName)
        assertNull(draft.items.first().image.asRemoteUploadSource().fileName)
        assertFalse(draft.add(file).accepted)
        val uploading = draft.beginSubmission().draft.recordProgress("pdf", 10, 20).draft
        assertEquals(10, uploading.items.last().sentBytes)
        assertEquals(2, uploading.completeSubmission().removed.size)
    }

    @Test fun documentsProduceAUsableFilePromptAndOneEnter() {
        assertEquals(listOf("\u001b[200~Please inspect the attached file(s):\n\nAttached files:\n- /project/log.pdf\u001b[201~", "\r"),
            formatTerminalSubmission("", listOf("/project/log.pdf"), bracketedPaste = true, hasFiles = true))
        val split = splitConversationAttachments("Look at this\n\nAttached files:\n- /project/log.pdf")
        assertEquals("Look at this", split.text)
        assertEquals(listOf("/project/log.pdf"), split.imagePaths)
    }
}
