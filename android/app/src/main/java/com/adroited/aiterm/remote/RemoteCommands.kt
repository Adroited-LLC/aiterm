package com.adroited.aiterm.remote

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.cbor.ByteString
import kotlinx.serialization.cbor.Cbor

@Serializable
data class RemoteSession(
    val id: String,
    val agent: String,
    val title: String,
    @SerialName("project_path") val projectPath: String,
    @SerialName("group_path") val groupPath: String,
    val branch: String? = null,
    val forked: Boolean,
    val background: Boolean,
    @SerialName("fork_parent") val forkParent: String? = null,
    @SerialName("last_active") val lastActive: Long,
)

data class AttachedTerminal(
    val tabId: String,
    val attachmentId: String,
    val hasFocus: Boolean,
    val title: String,
)

data class RemoteFocusEvent(
    val tabId: String,
    val attachmentId: String,
    val focus: FocusOwner,
    val size: TerminalSize,
)

@OptIn(ExperimentalSerializationApi::class)
object RemoteCommands {
    private val cbor = Cbor {
        encodeDefaults = true
        ignoreUnknownKeys = false
        useDefiniteLengthEncoding = true
    }

    fun tab(tabId: String): ByteArray = encode(TabIdPayload.serializer(), TabIdPayload(tabId))
    fun attachment(tabId: String, attachmentId: String): ByteArray =
        encode(AttachmentPayload.serializer(), AttachmentPayload(tabId, attachmentId))
    fun input(tabId: String, attachmentId: String, data: ByteArray): ByteArray =
        encode(InputPayload.serializer(), InputPayload(tabId, attachmentId, data))
    fun sized(tabId: String, attachmentId: String, size: TerminalSize): ByteArray =
        encode(SizedPayload.serializer(), SizedPayload(tabId, attachmentId, size))
    fun scrollback(tabId: String, attachmentId: String, offset: Int, count: Int): ByteArray =
        encode(ScrollbackPayload.serializer(), ScrollbackPayload(tabId, attachmentId, offset, count))
    fun session(sessionId: String): ByteArray =
        encode(SessionIdPayload.serializer(), SessionIdPayload(sessionId))
    fun openSession(sessionId: String, size: TerminalSize): ByteArray =
        encode(SessionOpenPayload.serializer(), SessionOpenPayload(sessionId, size))
    fun closeSession(sessionId: String, tabId: String?): ByteArray =
        encode(SessionClosePayload.serializer(), SessionClosePayload(sessionId, tabId))
    fun shell(projectPath: String?, title: String?, size: TerminalSize): ByteArray =
        encode(TabOpenPayload.serializer(), TabOpenPayload(projectPath = projectPath, title = title, size = size))

    fun sessions(payload: ByteArray): List<RemoteSession> =
        decode(SessionListReply.serializer(), payload).sessions.also { sessions ->
            if (sessions.size > 4_096 || sessions.any { it.id.length !in 1..512 }) malformed()
        }

    fun attached(payload: ByteArray): AttachedTerminal = decode(AttachedReply.serializer(), payload).let {
        if (it.tabId.isBlank() || it.attachmentId.isBlank() || it.title.length > 4_096) malformed()
        AttachedTerminal(it.tabId, it.attachmentId, it.hasFocus, it.title)
    }

    fun openedTab(payload: ByteArray): String = decode(TabOpenedReply.serializer(), payload).tabId
    fun openedSessionTab(payload: ByteArray): String = decode(SessionOpenedReply.serializer(), payload).tabId

    fun focus(payload: ByteArray): RemoteFocusEvent = decode(FocusReply.serializer(), payload).let {
        RemoteFocusEvent(
            it.tabId,
            it.attachmentId,
            when (it.focus) {
                WireFocusOwner.Self -> FocusOwner.Self
                WireFocusOwner.Other -> FocusOwner.Other
                WireFocusOwner.Unowned -> FocusOwner.Unowned
            },
            it.size,
        )
    }

    private fun <T> encode(serializer: kotlinx.serialization.KSerializer<T>, value: T): ByteArray =
        cbor.encodeToByteArray(serializer, value).also {
            if (it.isEmpty() || it.size >= RemoteWireCodec.MAX_FRAME_BYTES) malformed()
        }

    private fun <T> decode(serializer: kotlinx.serialization.KSerializer<T>, payload: ByteArray): T =
        try {
            cbor.decodeFromByteArray(serializer, payload)
        } catch (error: Exception) {
            throw RemoteProtocolException("malformed remote operation payload", error)
        }

    private fun malformed(): Nothing = throw RemoteProtocolException("invalid remote operation payload")

    @Serializable private data class TabIdPayload(@SerialName("tab_id") val tabId: String)
    @Serializable private data class AttachmentPayload(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
    )
    @Serializable private data class InputPayload(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
        @ByteString val data: ByteArray,
    )
    @Serializable private data class SizedPayload(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
        val size: TerminalSize,
    )
    @Serializable private data class ScrollbackPayload(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
        val offset: Int,
        val count: Int,
    )
    @Serializable private data class SessionIdPayload(@SerialName("session_id") val sessionId: String)
    @Serializable private data class SessionOpenPayload(
        @SerialName("session_id") val sessionId: String,
        val size: TerminalSize,
    )
    @Serializable private data class SessionClosePayload(
        @SerialName("session_id") val sessionId: String,
        @SerialName("tab_id") val tabId: String?,
    )
    @Serializable private data class TabOpenPayload(
        val kind: String = "shell",
        @SerialName("project_path") val projectPath: String?,
        val title: String?,
        val size: TerminalSize,
    )
    @Serializable private data class SessionListReply(val sessions: List<RemoteSession>)
    @Serializable private data class AttachedReply(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
        @SerialName("has_focus") val hasFocus: Boolean,
        val title: String,
    )
    @Serializable private data class TabOpenedReply(@SerialName("tab_id") val tabId: String)
    @Serializable private data class SessionOpenedReply(
        @SerialName("tab_id") val tabId: String,
        @SerialName("selected_existing") val selectedExisting: Boolean,
    )
    @Serializable private data class FocusReply(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
        val focus: WireFocusOwner,
        val size: TerminalSize,
    )
}
