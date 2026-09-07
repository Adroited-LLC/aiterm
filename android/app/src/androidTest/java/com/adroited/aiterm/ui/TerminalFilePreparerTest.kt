package com.adroited.aiterm.ui

import android.content.Context
import android.net.Uri
import android.os.Bundle
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.io.File
import java.security.MessageDigest
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull
import org.junit.After
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class TerminalFilePreparerTest {
    private val context = ApplicationProvider.getApplicationContext<Context>()
    private val uri = Uri.parse("content://com.adroited.aiterm.test.mutable-image/image")
    private val outputs = mutableListOf<File>()

    @After fun cleanup() {
        outputs.forEach { it.delete() }
        context.contentResolver.call(uri, "reset", null, null)
    }

    @Test fun arbitraryBytesAreCopiedOnceEvenWhenProviderReportsImageMime() = runBlocking {
        val bytes = "%PDF-1.7\nWayland protocol log\n%%EOF".toByteArray() + byteArrayOf(0, -1)
        context.contentResolver.call(uri, "configure", null, Bundle().apply {
            putByteArray("first", bytes); putByteArray("later", "changed".toByteArray())
        })
        val prepared = prepareTerminalFile(context, uri).getOrThrow().also { outputs += it.file }
        assertArrayEquals(bytes, prepared.file.readBytes())
        assertArrayEquals(MessageDigest.getInstance("SHA-256").digest(bytes), prepared.sha256)
        assertEquals("attachment", prepared.fileName)
        assertEquals(1, context.contentResolver.call(uri, "stats", null, null)!!.getInt("opens"))
    }

    @Test fun declaredSizeIsNotTrustedAndOversizedCopyIsRemoved() = runBlocking {
        val before = snapshots()
        context.contentResolver.call(uri, "configure-generated", null, Bundle().apply {
            putInt("length", TerminalAttachmentDraft.MAX_ATTACHMENT_BYTES.toInt() + 1)
        })
        assertTrue(prepareTerminalFile(context, uri).isFailure)
        assertEquals(before, snapshots())
    }

    @Test fun emptyDocumentsAreRejectedWithoutLeavingSnapshots() = runBlocking {
        val before = snapshots()
        context.contentResolver.call(uri, "configure", null, Bundle().apply {
            putByteArray("first", byteArrayOf()); putByteArray("later", byteArrayOf())
        })
        assertTrue(prepareTerminalFile(context, uri).isFailure)
        assertEquals(before, snapshots())
    }

    @Test fun cancellingACloudDocumentCopyRemovesItsPartialSnapshot() = runBlocking {
        val before = snapshots()
        context.contentResolver.call(uri, "configure-slow", null, Bundle().apply {
            putInt("length", 2 * 1024 * 1024); putInt("chunk", 4096); putLong("delay", 30)
        })
        assertNull(withTimeoutOrNull(150) { prepareTerminalFile(context, uri) })
        assertEquals(before, snapshots())
    }

    private fun snapshots() = File(context.cacheDir, "terminal-files").listFiles()?.map { it.name }?.toSet().orEmpty()
}
