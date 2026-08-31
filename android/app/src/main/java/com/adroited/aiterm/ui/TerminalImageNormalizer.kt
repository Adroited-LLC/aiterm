package com.adroited.aiterm.ui

import android.content.ContentResolver
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.ImageDecoder
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Rect
import android.net.Uri
import android.os.Build
import androidx.exifinterface.media.ExifInterface
import java.io.BufferedOutputStream
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.security.MessageDigest
import java.util.UUID
import java.nio.file.Files
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlin.math.ceil
import kotlin.math.max

data class NormalizedTerminalImage(
    val id: String,
    val file: File,
    val width: Int,
    val height: Int,
    val length: Long,
    val sha256: ByteArray,
)

class TerminalImageNormalizationError(
    val code: Code,
    message: String,
    cause: Throwable? = null,
) : IOException(message, cause) {
    enum class Code {
        CONTENT_UNAVAILABLE,
        EMPTY_CONTENT,
        DECODE_FAILED,
        DIMENSIONS_OUT_OF_BOUNDS,
        OUTPUT_TOO_LARGE,
        OUTPUT_FAILED,
    }
}

/**
 * Turns untrusted, transient picker or camera URIs into bounded app-private JPEG drafts.
 *
 * Drafts deliberately live outside the FileProvider's capture directory: only a camera app
 * needs a shareable URI, while normalized copies should never leave AITerm until upload.
 */
class TerminalImageNormalizer(
    context: Context,
    private val clockMillis: () -> Long = System::currentTimeMillis,
    private val maxOutputBytes: Long = MAX_OUTPUT_BYTES,
) {
    private val resolver: ContentResolver = context.contentResolver
    private val cacheDirectory = context.cacheDir
    private val draftsDirectory = File(cacheDirectory, DRAFT_DIRECTORY_NAME)

    init {
        require(maxOutputBytes in 1..MAX_OUTPUT_BYTES) { "maxOutputBytes must stay within the JPEG upload limit" }
    }

    suspend fun normalize(uri: Uri): Result<NormalizedTerminalImage> = withContext(Dispatchers.IO) {
        runCatching { normalizeBlocking(uri) }
    }

    /** Deletes a bounded batch of only AITerm-generated stale drafts. */
    suspend fun cleanupExpiredDrafts() = withContext(Dispatchers.IO) {
        cleanupExpiredDraftsBlocking(clockMillis())
    }

    private fun normalizeBlocking(uri: Uri): NormalizedTerminalImage {
        cleanupExpiredDraftsBlocking(clockMillis())
        ensureContentIsNotEmpty(uri)

        var decoded: Bitmap? = null
        var encoded: Bitmap? = null
        var output: File? = null
        try {
            decoded = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                decodeModern(uri)
            } else {
                decodeLegacy(uri)
            }
            val outputSize = constrainedSize(decoded.width, decoded.height)
            encoded = Bitmap.createBitmap(outputSize.width, outputSize.height, Bitmap.Config.ARGB_8888)
            Canvas(encoded).apply {
                drawColor(TERMINAL_BACKGROUND)
                drawBitmap(
                    decoded,
                    null,
                    Rect(0, 0, outputSize.width, outputSize.height),
                    SCALE_PAINT,
                )
            }

            val directory = ensureDraftsDirectory()
            val id = UUID.randomUUID().toString()
            output = File(directory, "$id.jpg")
            BufferedOutputStream(FileOutputStream(output)).use { stream ->
                if (!encoded.compress(Bitmap.CompressFormat.JPEG, JPEG_QUALITY, stream)) {
                    throw TerminalImageNormalizationError(
                        TerminalImageNormalizationError.Code.OUTPUT_FAILED,
                        "could not encode terminal image as JPEG",
                    )
                }
            }

            val length = output.length()
            if (length !in 1..maxOutputBytes) {
                throw TerminalImageNormalizationError(
                    TerminalImageNormalizationError.Code.OUTPUT_TOO_LARGE,
                    "normalized image exceeds the ${MAX_OUTPUT_BYTES}-byte limit",
                )
            }
            return NormalizedTerminalImage(
                id = id,
                file = output,
                width = outputSize.width,
                height = outputSize.height,
                length = length,
                sha256 = sha256(output),
            )
        } catch (error: TerminalImageNormalizationError) {
            output?.delete()
            throw error
        } catch (error: Exception) {
            output?.delete()
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.DECODE_FAILED,
                "could not decode the selected image",
                error,
            )
        } finally {
            encoded?.recycle()
            decoded?.recycle()
        }
    }

    private fun ensureContentIsNotEmpty(uri: Uri) {
        val firstByte = try {
            resolver.openInputStream(uri)?.use { it.read() }
        } catch (error: Exception) {
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.CONTENT_UNAVAILABLE,
                "could not open the selected image",
                error,
            )
        }
        when (firstByte) {
            null -> throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.CONTENT_UNAVAILABLE,
                "could not open the selected image",
            )
            -1 -> throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.EMPTY_CONTENT,
                "the selected image is empty",
            )
        }
    }

    @Suppress("NewApi")
    private fun decodeModern(uri: Uri): Bitmap {
        var validationError: TerminalImageNormalizationError? = null
        return try {
            ImageDecoder.decodeBitmap(ImageDecoder.createSource(resolver, uri)) { decoder, info, _ ->
                try {
                    // Sampling is decided from headers before ImageDecoder allocates any bitmap.
                    decoder.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE)
                    decoder.setTargetSampleSize(validatedSampleSize(info.size.width, info.size.height))
                } catch (error: TerminalImageNormalizationError) {
                    validationError = error
                    throw error
                }
            }
        } catch (error: TerminalImageNormalizationError) {
            throw error
        } catch (error: Exception) {
            throw validationError ?: TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.DECODE_FAILED,
                "could not decode the selected image",
                error,
            )
        }
    }

    private fun decodeLegacy(uri: Uri): Bitmap {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        try {
            resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
        } catch (error: Exception) {
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.CONTENT_UNAVAILABLE,
                "could not open the selected image",
                error,
            )
        }
        val sampleSize = validatedSampleSize(bounds.outWidth, bounds.outHeight)
        val options = BitmapFactory.Options().apply {
            inSampleSize = sampleSize
            inPreferredConfig = Bitmap.Config.ARGB_8888
        }
        val decoded = try {
            resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, options) }
        } catch (error: Exception) {
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.DECODE_FAILED,
                "could not decode the selected image",
                error,
            )
        } ?: throw TerminalImageNormalizationError(
            TerminalImageNormalizationError.Code.DECODE_FAILED,
            "could not decode the selected image",
        )
        return applyExifOrientation(uri, decoded)
    }

    private fun applyExifOrientation(uri: Uri, bitmap: Bitmap): Bitmap {
        val orientation = try {
            resolver.openInputStream(uri)?.use { ExifInterface(it).getAttributeInt(
                ExifInterface.TAG_ORIENTATION,
                ExifInterface.ORIENTATION_NORMAL,
            ) }
        } catch (_: Exception) {
            ExifInterface.ORIENTATION_NORMAL
        } ?: ExifInterface.ORIENTATION_NORMAL
        val matrix = Matrix().apply {
            when (orientation) {
                ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> setScale(-1f, 1f)
                ExifInterface.ORIENTATION_ROTATE_180 -> setRotate(180f)
                ExifInterface.ORIENTATION_FLIP_VERTICAL -> setScale(1f, -1f)
                ExifInterface.ORIENTATION_TRANSPOSE -> {
                    setRotate(90f)
                    postScale(-1f, 1f)
                }
                ExifInterface.ORIENTATION_ROTATE_90 -> setRotate(90f)
                ExifInterface.ORIENTATION_TRANSVERSE -> {
                    setRotate(-90f)
                    postScale(-1f, 1f)
                }
                ExifInterface.ORIENTATION_ROTATE_270 -> setRotate(-90f)
            }
        }
        if (matrix.isIdentity) return bitmap
        return try {
            Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, matrix, true)
                .also { bitmap.recycle() }
        } catch (error: Exception) {
            bitmap.recycle()
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.DECODE_FAILED,
                "could not apply the image orientation",
                error,
            )
        }
    }

    private fun validatedSampleSize(width: Int, height: Int): Int {
        if (width !in 1..MAX_SOURCE_EDGE || height !in 1..MAX_SOURCE_EDGE) {
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.DIMENSIONS_OUT_OF_BOUNDS,
                "selected image dimensions exceed safe decode bounds",
            )
        }
        var sample = 1
        while (
            ceil(width.toDouble() / (sample * 2)) > MAX_IMAGE_EDGE ||
            ceil(height.toDouble() / (sample * 2)) > MAX_IMAGE_EDGE
        ) {
            sample *= 2
        }
        val sampledPixels = ceil(width.toDouble() / sample).toLong() *
            ceil(height.toDouble() / sample).toLong()
        if (sampledPixels > MAX_DECODED_PIXELS) {
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.DIMENSIONS_OUT_OF_BOUNDS,
                "selected image would require too much decode memory",
            )
        }
        return sample
    }

    private fun constrainedSize(width: Int, height: Int): ImageSize {
        if (width <= 0 || height <= 0) {
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.DECODE_FAILED,
                "decoded image has invalid dimensions",
            )
        }
        val longest = max(width, height)
        if (longest <= MAX_IMAGE_EDGE) return ImageSize(width, height)
        val scale = MAX_IMAGE_EDGE.toDouble() / longest
        return ImageSize(
            width = max(1, (width * scale).toInt()),
            height = max(1, (height * scale).toInt()),
        )
    }

    private fun ensureDraftsDirectory(): File {
        if (draftsDirectory.exists()) {
            if (!draftsDirectory.isDirectory || isSymlink(draftsDirectory)) {
                throw TerminalImageNormalizationError(
                    TerminalImageNormalizationError.Code.OUTPUT_FAILED,
                    "terminal image draft path is not a directory",
                )
            }
        } else if (!draftsDirectory.mkdirs() && !draftsDirectory.isDirectory) {
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.OUTPUT_FAILED,
                "could not create terminal image draft directory",
            )
        }
        if (draftsDirectory.canonicalFile.parentFile != contextCacheDirectory()) {
            throw TerminalImageNormalizationError(
                TerminalImageNormalizationError.Code.OUTPUT_FAILED,
                "terminal image draft path is outside the private cache",
            )
        }
        return draftsDirectory
    }

    private fun cleanupExpiredDraftsBlocking(nowMillis: Long) {
        if (
            !draftsDirectory.isDirectory ||
            isSymlink(draftsDirectory) ||
            runCatching { draftsDirectory.canonicalFile.parentFile != contextCacheDirectory() }.getOrDefault(true)
        ) return
        draftsDirectory.listFiles()
            .orEmpty()
            .asSequence()
            .filter(::isGeneratedDraft)
            .filter { nowMillis - it.lastModified() > DRAFT_TTL_MILLIS }
            .sortedBy(File::lastModified)
            .take(MAX_CLEANUP_FILES_PER_PASS)
            .forEach(File::delete)
    }

    private fun isGeneratedDraft(file: File): Boolean =
        file.name.matches(GENERATED_DRAFT_NAME) &&
            file.isFile &&
            !isSymlink(file)

    private fun isSymlink(file: File): Boolean =
        runCatching { Files.isSymbolicLink(file.toPath()) }.getOrDefault(true)

    private fun contextCacheDirectory(): File = cacheDirectory.canonicalFile

    private fun sha256(file: File): ByteArray {
        val digest = MessageDigest.getInstance("SHA-256")
        FileInputStream(file).use { stream ->
            val buffer = ByteArray(DEFAULT_BUFFER_BYTES)
            while (true) {
                val count = stream.read(buffer)
                if (count < 0) break
                digest.update(buffer, 0, count)
            }
        }
        return digest.digest()
    }

    private data class ImageSize(val width: Int, val height: Int)

    companion object {
        const val MAX_IMAGE_EDGE = 4_096
        const val MAX_SOURCE_EDGE = 32_768
        const val MAX_OUTPUT_BYTES = 12L * 1_024 * 1_024
        const val DRAFT_TTL_MILLIS = 24L * 60 * 60 * 1_000
        private const val MAX_DECODED_PIXELS = 64L * 1_024 * 1_024
        private const val MAX_CLEANUP_FILES_PER_PASS = 64
        private const val JPEG_QUALITY = 90
        private const val DEFAULT_BUFFER_BYTES = 32 * 1_024
        private const val DRAFT_DIRECTORY_NAME = "terminal-image-drafts"
        private val GENERATED_DRAFT_NAME = Regex(
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\\.jpg",
        )
        private val TERMINAL_BACKGROUND = Color.rgb(0x07, 0x11, 0x1B)
        private val SCALE_PAINT = Paint(Paint.ANTI_ALIAS_FLAG or Paint.FILTER_BITMAP_FLAG)
    }
}
