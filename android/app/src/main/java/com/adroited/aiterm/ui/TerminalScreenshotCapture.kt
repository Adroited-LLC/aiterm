package com.adroited.aiterm.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.view.View
import androidx.core.content.FileProvider
import java.io.File
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** Captures the visible AITerm window after the source dialog has closed. */
internal suspend fun captureTerminalScreenshot(
    context: Context,
    view: View,
): TerminalImagePickerResult {
    var bitmap: Bitmap? = null
    var captureFile: File? = null
    return try {
        bitmap = withContext(Dispatchers.Main.immediate) {
            require(view.isAttachedToWindow && view.width > 0 && view.height > 0) {
                "AITerm is not ready to capture"
            }
            Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888).also {
                view.draw(Canvas(it))
            }
        }
        captureFile = withContext(Dispatchers.IO) {
            val directory = File(context.cacheDir, "terminal-image-captures").apply {
                if (!exists() && !mkdirs()) error("capture directory is unavailable")
            }
            File(directory, "${UUID.randomUUID()}.jpg").also { file ->
                file.outputStream().buffered().use { output ->
                    check(bitmap.compress(Bitmap.CompressFormat.JPEG, 92, output)) {
                        "screen capture could not be encoded"
                    }
                }
            }
        }
        TerminalImagePickerResult.Selected(
            uris = listOf(
                FileProvider.getUriForFile(
                    context,
                    "${context.packageName}.terminal-images",
                    captureFile,
                ),
            ),
            ownedCaptureFiles = setOf(captureFile),
        )
    } catch (error: CancellationException) {
        captureFile?.delete()
        throw error
    } catch (_: Exception) {
        captureFile?.delete()
        TerminalImagePickerResult.Failed("Could not capture this screen. Try Gallery instead.")
    } finally {
        bitmap?.recycle()
    }
}
