package com.adroited.aiterm.remote

import com.adroited.aiterm.terminal.TerminalColor
import java.io.ByteArrayOutputStream
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

    @Test
    fun rustShapedSnapshotChunkDecodesIntoNativeTerminalCells() {
        val attributes = linkedMapOf(
            "bold" to true,
            "faint" to false,
            "italic" to false,
            "underline" to false,
            "inverse" to false,
            "hidden" to false,
            "strikethrough" to false,
        )
        val cell = linkedMapOf(
            "text" to "λ",
            "width" to 1,
            "continuation" to false,
            "foreground" to "Default",
            "background" to linkedMapOf("Indexed" to 4),
            "attributes" to attributes,
        )
        val part = fixture(
            linkedMapOf(
                "cols" to 1,
                "rows" to 1,
                "visible" to listOf(linkedMapOf("cells" to listOf(cell), "wrapped" to false)),
                "cursor" to linkedMapOf(
                    "col" to 0,
                    "row" to 0,
                    "visible" to true,
                    "shape" to "Beam",
                ),
                "modes" to linkedMapOf(
                    "application_cursor" to false,
                    "bracketed_paste" to true,
                    "line_wrap" to true,
                    "alternate_screen" to false,
                ),
            ),
        )
        val encoded = fixture(
            linkedMapOf(
                "transfer_id" to "transfer-1",
                "tab_id" to "tab-1",
                "attachment_id" to "attachment-1",
                "kind" to "snapshot",
                "base_revision" to 9,
                "final_revision" to 9,
                "row_start" to 0,
                "row_end" to 1,
                "index" to 0,
                "total" to 1,
                "request_id" to 7,
                "payload" to part,
            ),
        )

        val chunk = RemoteWireCodec.decodeTerminalChunk(encoded, expectedRequestId = 7)

        assertEquals(TerminalTransferKind.Snapshot, chunk.kind)
        val snapshot = chunk.part as TerminalTransferPart.Snapshot
        assertEquals("λ", snapshot.visible.single().cells.single().text)
        assertEquals(TerminalColor.Indexed(4), snapshot.visible.single().cells.single().background)
        assertEquals(true, snapshot.visible.single().cells.single().attributes.bold)
    }

    @Test
    fun rosterDescriptorUsesTheRustCamelCaseFieldContract() {
        val payload = fixture(
            linkedMapOf(
                "transfer_id" to "roster-1",
                "revision" to 12,
                "index" to 0,
                "total" to 1,
                "tabs" to listOf(
                    linkedMapOf(
                        "id" to "tab-1",
                        "title" to "Shell",
                        "sessionId" to "session-1",
                        "size" to linkedMapOf("cols" to 80, "rows" to 24),
                        "focus" to "self",
                        "state" to "running",
                    ),
                ),
            ),
        )

        val chunk = RemoteWireCodec.decodeStateSnapshot(payload)

        assertEquals("session-1", chunk.tabs.single().sessionId)
        assertEquals(WireFocusOwner.Self, chunk.tabs.single().focus)
    }

    private fun fixture(value: Any?): ByteArray {
        val output = ByteArrayOutputStream()
        fun writeHeader(major: Int, size: Int) {
            if (size < 24) {
                output.write((major shl 5) or size)
            } else if (size <= 0xff) {
                output.write((major shl 5) or 24)
                output.write(size)
            } else {
                output.write((major shl 5) or 25)
                output.write(size ushr 8)
                output.write(size)
            }
        }
        fun write(item: Any?) {
            when (item) {
                null -> output.write(0xf6)
                is Boolean -> output.write(if (item) 0xf5 else 0xf4)
                is Int -> {
                    writeHeader(0, item)
                }
                is Long -> {
                    require(item <= Int.MAX_VALUE)
                    writeHeader(0, item.toInt())
                }
                is String -> {
                    val bytes = item.encodeToByteArray()
                    writeHeader(3, bytes.size)
                    output.write(bytes)
                }
                is ByteArray -> {
                    writeHeader(2, item.size)
                    output.write(item)
                }
                is List<*> -> {
                    writeHeader(4, item.size)
                    item.forEach(::write)
                }
                is Map<*, *> -> {
                    writeHeader(5, item.size)
                    item.forEach { (key, entry) ->
                        write(key as String)
                        write(entry)
                    }
                }
                else -> error("unsupported fixture type ${item::class}")
            }
        }
        write(value)
        return output.toByteArray()
    }

    private fun hex(value: String): ByteArray =
        value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}
