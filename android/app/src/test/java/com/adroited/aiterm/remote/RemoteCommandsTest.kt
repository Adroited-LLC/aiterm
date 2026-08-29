package com.adroited.aiterm.remote

import org.junit.Assert.assertArrayEquals
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

    private fun hex(value: String): ByteArray =
        value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}
