package com.adroited.aiterm.ui

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.io.IOException
import java.security.MessageDigest
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import kotlin.coroutines.coroutineContext

internal data class PreparedTerminalFile(
    val id: String, val file: File, val name: String, val length: Long, val sha256: ByteArray,
)

/** Copy once while the document grant is live. Retries use these exact private bytes. */
internal suspend fun prepareTerminalFile(context: Context, uri: Uri): Result<TerminalAttachment> {
    var snapshot: File? = null
    return try {
        withContext(Dispatchers.IO) {
            val resolver = context.contentResolver
            var name = "attachment"
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                val column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (column >= 0 && cursor.moveToFirst()) name = cursor.getString(column) ?: name
            }
            val directory = File(context.cacheDir, "terminal-files")
            if (!directory.isDirectory && !directory.mkdirs()) throw IOException("Could not prepare the file.")
            // Only our UUID snapshots, and only after the same 24-hour draft lifetime as photos.
            directory.listFiles()?.filter { it.name.matches(Regex("[0-9a-f-]{36}\\.upload")) &&
                !java.nio.file.Files.isSymbolicLink(it.toPath()) && it.isFile &&
                System.currentTimeMillis() - it.lastModified() > TERMINAL_CAPTURE_TTL_MILLIS
            }?.take(64)?.forEach { it.delete() }
            val id = UUID.randomUUID().toString()
            val file = File(directory, "$id.upload")
            if (!file.createNewFile()) throw IOException("Could not prepare the file.")
            snapshot = file
            val digest = MessageDigest.getInstance("SHA-256")
            var length = 0L
            resolver.openInputStream(uri)?.use { input ->
                file.outputStream().buffered().use { output ->
                    val buffer = ByteArray(64 * 1024)
                    while (true) {
                        coroutineContext.ensureActive()
                        val count = input.read(buffer)
                        if (count < 0) break
                        length += count
                        if (length > TerminalAttachmentDraft.MAX_ATTACHMENT_BYTES) {
                            throw IOException("Each attachment must be 12 MiB or smaller.")
                        }
                        output.write(buffer, 0, count)
                        digest.update(buffer, 0, count)
                    }
                }
            } ?: throw IOException("Could not read this file. Choose it again from Files.")
            if (length == 0L) throw IOException("The selected file is empty.")
            Result.success(TerminalAttachment.from(PreparedTerminalFile(id, file, safeAttachmentName(name), length, digest.digest())))
        }
    } catch (error: Exception) {
        snapshot?.delete()
        if (error is CancellationException) throw error
        Result.failure(IOException(error.message ?: "Could not read this file.", error))
    }
}

/** Preserve readable names while preventing paths, control characters and Windows-special names. */
internal fun safeAttachmentName(name: String): String {
    var safe = name.map { if (it.isISOControl() || it in "/\\:*?\"<>|") '_' else it }.joinToString("")
        .trim().trimEnd('.', ' ')
    while (safe.toByteArray(Charsets.UTF_8).size > 180) {
        safe = safe.substring(0, safe.offsetByCodePoints(safe.length, -1))
    }
    return safe.trimEnd('.', ' ').ifBlank { "attachment" }
}
