package com.adroited.aiterm.remote

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class RemoteCommandsTest {
    @Test
    fun terminalInputUsesTheExactStrictRustPayloadShape() {
        assertArrayEquals(
            hex(
                "a3" +
                    "667461625f69646174" +
                    "6d6174746163686d656e745f69646161" +
                    "64646174614178",
            ),
            RemoteCommands.input("t", "a", byteArrayOf('x'.code.toByte())),
        )
    }

    @Test
    fun sessionPreviewDecodesTheExactRustMessageShape() {
        val messages = RemoteCommands.sessionPreview(
            hex(
                "a1" +
                    "686d65737361676573" +
                    "81a3" +
                    "64726f6c656475736572" +
                    "64746578746568656c6c6f" +
                    "626174f6",
            ),
        )

        assertEquals(listOf(RemotePreviewMessage("user", "hello")), messages)
    }

    @Test
    fun sessionPreviewAcceptsTheRustTimestampField() {
        val messages = RemoteCommands.sessionPreview(
            hex(
                "a1" +
                    "686d65737361676573" +
                    "81a3" +
                    "64726f6c656475736572" +
                    "64746578746568656c6c6f" +
                    "62617474323032362d30382d32395432333a34323a30305a",
            ),
        )

        assertEquals("2026-08-29T23:42:00Z", messages.single().at)
    }

    private fun hex(value: String): ByteArray =
        value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}
