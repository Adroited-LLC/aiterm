package com.adroited.aiterm.remote

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class RemoteWireCodecTest {

    @Test
    fun requestEnvelopeMatchesTheRustCborShape() {
        val encoded = RemoteWireCodec.encodeRequest(
            RemoteRequest(requestId = 1, kind = "tab.list", payload = byteArrayOf()),
        )

        assertArrayEquals(
            hex(
                "a4" +
                    "6776657273696f6e01" +
                    "6a726571756573745f696401" +
                    "646b696e64687461622e6c697374" +
                    "677061796c6f616440",
            ),
            encoded,
        )
    }

    @Test
    fun rustEventFixtureDecodesWithCorrelationAndByteStringPayload() {
        val event = RemoteWireCodec.decodeEvent(
            hex(
                "a4" +
                    "6776657273696f6e01" +
                    "6a726571756573745f696407" +
                    "646b696e64687461622e6c697374" +
                    "677061796c6f616443010203",
            ),
        )

        assertEquals(7L, event.requestId)
        assertEquals("tab.list", event.kind)
        assertArrayEquals(byteArrayOf(1, 2, 3), event.payload)
    }

    @Test
    fun duplicateEnvelopeFieldsAndTrailingCborAreRejected() {
        assertThrows(RemoteProtocolException::class.java) {
            RemoteWireCodec.decodeEvent(
                hex(
                    "a5" +
                        "6776657273696f6e01" +
                        "6776657273696f6e01" +
                        "6a726571756573745f696407" +
                        "646b696e64687461622e6c697374" +
                        "677061796c6f616440",
                ),
            )
        }
        assertThrows(RemoteProtocolException::class.java) {
            RemoteWireCodec.decodeEvent(
                hex(
                    "a4" +
                        "6776657273696f6e01" +
                        "6a726571756573745f696407" +
                        "646b696e64687461622e6c697374" +
                        "677061796c6f616440" +
                        "f6",
                ),
            )
        }
    }

    private fun hex(value: String): ByteArray =
        value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}
