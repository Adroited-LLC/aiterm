package com.adroited.aiterm.ui

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalAttachmentDraftTest {
    @Test
    fun addAndRemovePreserveSelectionOrder() {
        val first = image("first", 12)
        val second = image("second", 34)
        val draft = TerminalAttachmentDraft()
            .add(first).draft
            .add(second).draft

        val removed = draft.remove("first")

        assertTrue(removed.accepted)
        assertEquals(listOf("second"), removed.draft.items.map { it.image.id })
        assertEquals(listOf("first"), removed.removed.map { it.image.id })
    }

    @Test
    fun duplicateImageIdIsRejectedWithoutChangingTheDraft() {
        val initial = TerminalAttachmentDraft().add(image("duplicate", 12)).draft

        val duplicate = initial.add(image("duplicate", 30))

        assertFalse(duplicate.accepted)
        assertEquals("This image is already attached.", duplicate.draft.message)
        assertEquals(listOf("duplicate"), duplicate.draft.items.map { it.image.id })
    }

    @Test
    fun fifthImageIsRejectedWithAnExplicitMessage() {
        val four = (1..4).fold(TerminalAttachmentDraft()) { draft, index ->
            draft.add(image("image-$index", 1)).draft
        }

        val fifth = four.add(image("image-5", 1))

        assertFalse(fifth.accepted)
        assertEquals("You can attach up to 4 images.", fifth.draft.message)
        assertEquals(4, fifth.draft.items.size)
    }

    @Test
    fun oversizedImageIsRejectedWithAnExplicitMessage() {
        val rejected = TerminalAttachmentDraft().add(
            image("oversized", TerminalAttachmentDraft.MAX_IMAGE_BYTES + 1),
        )

        assertFalse(rejected.accepted)
        assertEquals("Each image must be 12 MiB or smaller.", rejected.draft.message)
        assertTrue(rejected.draft.items.isEmpty())
    }

    @Test
    fun progressNeverMovesBackwardAndFailureKeepsTheFullDraftForRetry() {
        val draft = TerminalAttachmentDraft()
            .add(image("first", 100)).draft
            .add(image("second", 200)).draft
            .beginSubmission().draft
            .recordProgress("first", sentBytes = 70, totalBytes = 100).draft
            .recordProgress("first", sentBytes = 20, totalBytes = 100).draft

        val failed = draft.failSubmission("second", "Desktop disconnected.").draft
        val retry = failed.retrySubmission().draft

        assertFalse(failed.submitting)
        assertEquals(listOf("first", "second"), failed.items.map { it.image.id })
        assertEquals(70, failed.items[0].sentBytes)
        assertEquals(TerminalAttachmentUploadState.Failed, failed.items[1].state)
        assertEquals("Desktop disconnected.", failed.items[1].message)
        assertEquals(listOf("first", "second"), retry.items.map { it.image.id })
        assertFalse(retry.submitting)
        assertTrue(retry.items.all { it.state == TerminalAttachmentUploadState.Pending })
        assertTrue(retry.items.all { it.sentBytes == 0L && it.message == null })
    }

    @Test
    fun successfulSubmissionReturnsLocalFilesForCallerOwnedDeletion() {
        val completed = TerminalAttachmentDraft()
            .add(image("first", 12)).draft
            .add(image("second", 34)).draft
            .beginSubmission().draft
            .completeSubmission()

        assertTrue(completed.accepted)
        assertTrue(completed.draft.items.isEmpty())
        assertEquals(listOf("first", "second"), completed.removed.map { it.image.id })
    }

    @Test
    fun generalUploadFailurePreservesTheWholeDraftAndExplainsWhatHappened() {
        val failed = TerminalAttachmentDraft()
            .add(image("first", 12)).draft
            .add(image("second", 34)).draft
            .beginSubmission().draft
            .failSubmission("Desktop requires an update for image attachments.").draft

        assertFalse(failed.submitting)
        assertEquals(listOf("first", "second"), failed.items.map { it.image.id })
        assertTrue(failed.items.all { it.state == TerminalAttachmentUploadState.Pending })
        assertEquals("Desktop requires an update for image attachments.", failed.message)
    }

    @Test
    fun atomicAttachmentTransitionReturnsFilesForCallerOwnedDeletion() {
        val store = TerminalDraftStore()
        store.updateAttachments("tab-a") { it.add(image("first", 12)).draft }

        val removed = store.transitionAttachments("tab-a") { it.remove("first") }

        assertEquals(listOf("first"), removed.removed.map { it.image.id })
        assertTrue(store.draftFor("tab-a").attachments.items.isEmpty())
    }

    @Test
    fun attachmentDigestIsDefensivelyCopied() {
        val original = image("first", 12, digest = ByteArray(32) { 1 })
        val item = TerminalAttachmentDraft().add(original).draft.items.single()

        val firstRead = item.image.sha256
        firstRead[0] = 99
        val secondRead = item.image.sha256

        assertNotSame(firstRead, secondRead)
        assertEquals(1, secondRead[0].toInt())
    }

    @Test
    fun tabDraftStoreKeepsIndependentDraftsForEachAuthoritativeTab() {
        val store = TerminalDraftStore()
        store.updateComposer("tab-a") { it.open().updateValue(androidx.compose.ui.text.input.TextFieldValue("for A")).state }
        store.updateAttachments("tab-a") { it.add(image("a", 12)).draft }
        store.updateComposer("tab-b") { it.open().updateValue(androidx.compose.ui.text.input.TextFieldValue("for B")).state }
        store.updateAttachments("tab-b") { it.add(image("b", 34)).draft }

        assertEquals("for A", store.draftFor("tab-a").composer.value.text)
        assertEquals(listOf("a"), store.draftFor("tab-a").attachments.items.map { it.image.id })
        assertEquals("for B", store.draftFor("tab-b").composer.value.text)
        assertEquals(listOf("b"), store.draftFor("tab-b").attachments.items.map { it.image.id })
        assertTrue(store.hasDrafts())
    }

    private fun image(id: String, length: Long, digest: ByteArray = ByteArray(32) { 7 }) =
        NormalizedTerminalImage(
            id = id,
            file = File("/private/$id.jpg"),
            width = 20,
            height = 10,
            length = length,
            sha256 = digest,
        )
}
