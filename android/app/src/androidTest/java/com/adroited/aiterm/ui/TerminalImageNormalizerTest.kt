package com.adroited.aiterm.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.net.Uri
import androidx.core.content.FileProvider
import androidx.exifinterface.media.ExifInterface
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.io.DataOutputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import java.util.zip.CRC32
import java.util.zip.DeflaterOutputStream
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.assertThrows
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class TerminalImageNormalizerTest {
    private val context = ApplicationProvider.getApplicationContext<Context>()
    private val capturesRoot = File(
        context.cacheDir,
        "terminal-image-captures/normalizer-test-${UUID.randomUUID()}",
    ).apply { mkdirs() }
    private val normalizedOutputs = mutableListOf<File>()

    @After
    fun removeOnlyThisTestsFixtures() {
        capturesRoot.deleteRecursively()
        normalizedOutputs.forEach(File::delete)
    }

    @Test
    fun landscapeImage_isBoundedAndStoredAsMetadataFreePrivateJpeg() = runBlocking {
        val source = writeBitmap("landscape.jpg", 6_000, 3_000, Bitmap.CompressFormat.JPEG) { canvas ->
            canvas.drawColor(Color.rgb(90, 140, 210))
        }

        val image = normalizer().normalize(uriFor(source)).getOrThrow().also { normalizedOutputs += it.file }

        assertEquals(4_096, image.width)
        assertEquals(2_048, image.height)
        assertTrue(image.file.parentFile == File(context.cacheDir, "terminal-image-drafts"))
        assertThrows(IllegalArgumentException::class.java) {
            FileProvider.getUriForFile(context, "${context.packageName}.terminal-images", image.file)
        }
        assertTrue(image.file.name.matches(Regex("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\\.jpg")))
        assertEquals(image.file.length(), image.length)
        assertArrayEquals(sha256(image.file), image.sha256)
        val bytes = image.file.readBytes()
        assertTrue(bytes.size >= 4)
        assertEquals(0xff.toByte(), bytes.first())
        assertEquals(0xd8.toByte(), bytes[1])
        assertEquals(0xff.toByte(), bytes[bytes.lastIndex - 1])
        assertEquals(0xd9.toByte(), bytes.last())
        assertTrue(image.length in 1..TerminalImageNormalizer.MAX_OUTPUT_BYTES)

        val outputExif = ExifInterface(image.file.absolutePath)
        assertNull(outputExif.getAttribute(ExifInterface.TAG_GPS_LATITUDE))
        assertNull(outputExif.getAttribute(ExifInterface.TAG_GPS_LONGITUDE))
        assertNull(outputExif.getAttribute(ExifInterface.TAG_GPS_PROCESSING_METHOD))
        assertNull(outputExif.getAttribute(ExifInterface.TAG_MAKE))
        assertNull(outputExif.getAttribute(ExifInterface.TAG_MODEL))
    }

    @Test
    fun encodedExifRotation_isAppliedBeforeTheNormalizedDimensionsAreReported() = runBlocking {
        val source = writeBitmap("rotated.jpg", 30, 10, Bitmap.CompressFormat.JPEG) { canvas ->
            canvas.drawColor(Color.RED)
        }
        ExifInterface(source.absolutePath).apply {
            setAttribute(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_ROTATE_90.toString())
            saveAttributes()
        }

        val image = normalizer().normalize(uriFor(source)).getOrThrow().also { normalizedOutputs += it.file }

        assertEquals(10, image.width)
        assertEquals(30, image.height)
    }

    @Test
    fun transparentPng_isCompositedOntoTheTerminalBackgroundBeforeJpegEncoding() = runBlocking {
        val source = writeBitmap("transparent.png", 32, 32, Bitmap.CompressFormat.PNG) { canvas ->
            canvas.drawColor(Color.TRANSPARENT)
        }

        val image = normalizer().normalize(uriFor(source)).getOrThrow().also { normalizedOutputs += it.file }
        val color = BitmapFactory.decodeFile(image.file.absolutePath).getPixel(16, 16)

        assertTrue("red was ${Color.red(color)}", Color.red(color) in 0..25)
        assertTrue("green was ${Color.green(color)}", Color.green(color) in 5..35)
        assertTrue("blue was ${Color.blue(color)}", Color.blue(color) in 12..42)
    }

    @Test
    fun emptyContent_isRejectedWithoutCreatingADraft() = runBlocking {
        val source = File(capturesRoot, "empty.jpg").apply { writeBytes(ByteArray(0)) }

        val failure = normalizer().normalize(uriFor(source)).exceptionOrNull() as? TerminalImageNormalizationError

        assertNotNull(failure)
        assertEquals(TerminalImageNormalizationError.Code.EMPTY_CONTENT, failure?.code)
        assertFalse(File(context.cacheDir, "terminal-image-drafts").listFiles().orEmpty()
            .any { it.name.startsWith("empty") })
    }

    @Test
    fun corruptContent_isRejectedWithoutCreatingADraft() = runBlocking {
        val source = File(capturesRoot, "corrupt.jpg").apply { writeBytes("not an image".encodeToByteArray()) }

        val failure = normalizer().normalize(uriFor(source)).exceptionOrNull() as? TerminalImageNormalizationError

        assertNotNull(failure)
        assertEquals(TerminalImageNormalizationError.Code.DECODE_FAILED, failure?.code)
    }

    @Test
    fun claimedSourcePastTheDecodeBound_isRejectedBeforeBitmapAllocation() = runBlocking {
        val source = File(capturesRoot, "over-bound.png")
        writePngHeader(source, width = TerminalImageNormalizer.MAX_SOURCE_EDGE + 1, height = 1)

        val failure = normalizer().normalize(uriFor(source)).exceptionOrNull() as? TerminalImageNormalizationError

        assertNotNull(failure)
        assertEquals(TerminalImageNormalizationError.Code.DIMENSIONS_OUT_OF_BOUNDS, failure?.code)
    }

    @Test
    fun jpegWhoseNormalizedOutputExceedsTwelveMiB_isRemovedAndReported() = runBlocking {
        val source = writeNoisyBitmap("output-too-large.jpg", 4_096, 4_096)
        val drafts = File(context.cacheDir, "terminal-image-drafts")
        val before = drafts.listFiles().orEmpty().map(File::getName).toSet()

        val failure = normalizer().normalize(uriFor(source))
            .exceptionOrNull() as? TerminalImageNormalizationError

        assertNotNull(failure)
        assertEquals(TerminalImageNormalizationError.Code.OUTPUT_TOO_LARGE, failure?.code)
        assertEquals(before, drafts.listFiles().orEmpty().map(File::getName).toSet())
    }

    @Test
    fun lazyCleanup_deletesOnlyExpiredUuidDrafts() = runBlocking {
        val drafts = File(context.cacheDir, "terminal-image-drafts").apply { mkdirs() }
        val now = System.currentTimeMillis()
        val oldGenerated = File(drafts, "${UUID.randomUUID()}.jpg").apply {
            writeBytes(byteArrayOf(1))
            setLastModified(now - TerminalImageNormalizer.DRAFT_TTL_MILLIS - 1)
        }
        val freshGenerated = File(drafts, "${UUID.randomUUID()}.jpg").apply {
            writeBytes(byteArrayOf(2))
            setLastModified(now)
        }
        val unrelated = File(drafts, "user-photo.jpg").apply {
            writeBytes(byteArrayOf(3))
            setLastModified(now - TerminalImageNormalizer.DRAFT_TTL_MILLIS - 1)
        }

        TerminalImageNormalizer(context, clockMillis = { now }).cleanupExpiredDrafts()

        assertFalse(oldGenerated.exists())
        assertTrue(freshGenerated.exists())
        assertTrue(unrelated.exists())
        freshGenerated.delete()
        unrelated.delete()
        Unit
    }

    private fun normalizer() = TerminalImageNormalizer(context)

    private fun writeBitmap(
        name: String,
        width: Int,
        height: Int,
        format: Bitmap.CompressFormat,
        draw: (Canvas) -> Unit,
    ): File {
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        try {
            draw(Canvas(bitmap))
            return File(capturesRoot, name).also { file ->
                file.outputStream().use { output ->
                    assertTrue(bitmap.compress(format, 100, output))
                }
            }
        } finally {
            bitmap.recycle()
        }
    }

    private fun writeNoisyBitmap(name: String, width: Int, height: Int): File {
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        try {
            val pixels = IntArray(width * height)
            var state = 0x13579BDF
            for (index in pixels.indices) {
                state = state * 1_103_515_245 + 12_345
                pixels[index] = 0xff000000.toInt() or (state and 0x00ffffff)
            }
            bitmap.setPixels(pixels, 0, width, 0, 0, width, height)
            return File(capturesRoot, name).also { file ->
                file.outputStream().use { output ->
                    assertTrue(bitmap.compress(Bitmap.CompressFormat.JPEG, 100, output))
                }
            }
        } finally {
            bitmap.recycle()
        }
    }

    private fun uriFor(file: File): Uri = FileProvider.getUriForFile(
        context,
        "${context.packageName}.terminal-images",
        file,
    )

    private fun sha256(file: File): ByteArray = MessageDigest.getInstance("SHA-256").digest(file.readBytes())

    private fun writePngHeader(file: File, width: Int, height: Int) {
        val rawPixels = ByteArray(1 + width * 4)
        val compressed = ByteArrayOutputStream().use { bytes ->
            DeflaterOutputStream(bytes).use { it.write(rawPixels) }
            bytes.toByteArray()
        }
        DataOutputStream(file.outputStream()).use { output ->
            output.write(byteArrayOf(137.toByte(), 80, 78, 71, 13, 10, 26, 10))
            ByteArrayOutputStream().use { header ->
                DataOutputStream(header).use {
                    it.writeInt(width)
                    it.writeInt(height)
                    it.writeByte(8)
                    it.writeByte(6)
                    it.writeByte(0)
                    it.writeByte(0)
                    it.writeByte(0)
                }
                writePngChunk(output, "IHDR", header.toByteArray())
            }
            writePngChunk(output, "IDAT", compressed)
            writePngChunk(output, "IEND", ByteArray(0))
        }
    }

    private fun writePngChunk(output: DataOutputStream, type: String, data: ByteArray) {
        val typeBytes = type.encodeToByteArray()
        val crc = CRC32().apply {
            update(typeBytes)
            update(data)
        }
        output.writeInt(data.size)
        output.write(typeBytes)
        output.write(data)
        output.writeInt(crc.value.toInt())
    }
}
