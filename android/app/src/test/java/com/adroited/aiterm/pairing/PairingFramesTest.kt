package com.adroited.aiterm.pairing

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class PairingFramesTest {

    @Test
    fun requestFrame_matchesTheDesktopCborShape() {
        val encoded = PairingFrames.encode(
            PairRequestFrame(
                enrollmentSecret = byteArrayOf(1, 2),
                deviceName = "Pixel",
                publicKey = byteArrayOf(2, 3, 4),
            ),
        )

        assertArrayEquals(
            hex(
                "a4" +
                    "646b696e646c706169722e72657175657374" +
                    "71656e726f6c6c6d656e745f736563726574420102" +
                    "6b6465766963655f6e616d6565506978656c" +
                    "6a7075626c69635f6b657943020304",
            ),
            encoded,
        )
    }

    @Test
    fun desktopResponseFixtures_decodeWithoutUsingTheEncoderUnderTest() {
        assertEquals(
            PairPendingFrame("request-1"),
            PairingFrames.decode(
                hex("a2646b696e646c706169722e70656e64696e676a726571756573745f696469726571756573742d31"),
            ),
        )
        assertEquals(
            PairApprovedFrame("device-42"),
            PairingFrames.decode(
                hex("a2646b696e646d706169722e617070726f766564696465766963655f6964696465766963652d3432"),
            ),
        )
        assertEquals(
            PairDeniedFrame(),
            PairingFrames.decode(hex("a1646b696e646b706169722e64656e696564")),
        )
    }

    @Test
    fun unknownOrMalformedFrames_areRejected() {
        assertThrows(PairingProtocolException::class.java) {
            PairingFrames.decode(hex("a1646b696e646c706169722e756e6b6e6f776e"))
        }
        assertThrows(PairingProtocolException::class.java) {
            PairingFrames.decode(ByteArray(0))
        }
        assertThrows(PairingProtocolException::class.java) {
            PairingFrames.decode(
                hex(
                    "a3" +
                        "646b696e646c706169722e70656e64696e67" +
                        "6a726571756573745f69646473616d65" +
                        "6b6465766963655f6e616d656473616d65",
                ),
            )
        }
    }

    private fun hex(value: String): ByteArray =
        value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}
