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

data class RemoteTitleEvent(val tabId: String, val attachmentId: String, val title: String)
data class RemoteTerminalExitEvent(val tabId: String, val attachmentId: String, val exit: RemoteTabExit)
@Serializable data class RemotePreviewMessage(val role: String, val text: String, val at: String? = null)

@Serializable
data class RemoteModelOption(
    val id: String,
    @SerialName("display_name") val displayName: String,
    val efforts: List<String>,
    @SerialName("default_effort") val defaultEffort: String? = null,
)

@Serializable
data class RemoteAgentChoice(
    val id: String,
    @SerialName("display_name") val displayName: String,
    val models: List<RemoteModelOption>,
    @SerialName("mints_session_id") val mintsSessionId: Boolean,
)

@Serializable
data class RemoteAgentCaps(
    val fork: Boolean,
    val clear: Boolean,
    val resume: Boolean,
    @SerialName("tui_drive") val tuiDrive: Boolean,
    val panels: Boolean,
    val tasks: Boolean,
    val delete: Boolean,
    val config: Boolean,
    @SerialName("roster_liveness") val rosterLiveness: Boolean,
)

data class RemoteAgentRoster(
    val agents: List<RemoteAgentChoice>,
    val caps: Map<String, RemoteAgentCaps>,
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
    fun previewSession(sessionId: String): ByteArray = session(sessionId)
    fun openSession(sessionId: String, size: TerminalSize): ByteArray =
        encode(SessionOpenPayload.serializer(), SessionOpenPayload(sessionId, size))
    fun closeSession(sessionId: String, tabId: String?): ByteArray =
        encode(SessionClosePayload.serializer(), SessionClosePayload(sessionId, tabId))
    fun shell(projectPath: String?, title: String?, size: TerminalSize): ByteArray =
        encode(TabOpenPayload.serializer(), TabOpenPayload(projectPath = projectPath, title = title, size = size))
    fun startAgent(
        agentId: String,
        model: String?,
        effort: String?,
        cwd: String,
        title: String,
        size: TerminalSize,
    ): ByteArray = encode(
        AgentStartPayload.serializer(),
        AgentStartPayload(agentId = agentId, model = model, effort = effort, cwd = cwd, title = title, size = size),
    )

    fun sessions(payload: ByteArray): List<RemoteSession> =
        decode(SessionListReply.serializer(), payload).sessions.also { sessions ->
            if (sessions.size > 4_096 || sessions.any { it.id.length !in 1..512 }) malformed()
        }

    fun tabs(payload: ByteArray): List<RemoteTab> = decode(TabListReply.serializer(), payload).tabs.also {
        if (it.size > 128) malformed()
    }
    fun agents(payload: ByteArray): RemoteAgentRoster = decode(AgentListReply.serializer(), payload).let {
        if (it.agents.size > 64 || it.caps.size > 64) malformed()
        RemoteAgentRoster(it.agents, it.caps)
    }

    fun sessionPreview(payload: ByteArray): List<RemotePreviewMessage> =
        decode(SessionPreviewReply.serializer(), payload).messages.also { messages ->
            if (messages.size > 512 || messages.any {
                    it.role.length !in 1..64 || it.text.encodeToByteArray().size > 64 * 1_024
                } || messages.sumOf { it.text.encodeToByteArray().size } >= RemoteWireCodec.MAX_FRAME_BYTES
            ) malformed()
        }

    fun attached(payload: ByteArray): AttachedTerminal = decode(AttachedReply.serializer(), payload).let {
        if (it.tabId.isBlank() || it.attachmentId.isBlank() || it.title.length > 4_096) malformed()
        AttachedTerminal(it.tabId, it.attachmentId, it.hasFocus, it.title)
    }

    fun openedTab(payload: ByteArray): String = decode(TabOpenedReply.serializer(), payload).tabId
    fun openedSessionTab(payload: ByteArray): String = decode(SessionOpenedReply.serializer(), payload).tabId
    fun startedAgentTab(payload: ByteArray): String = decode(AgentStartedReply.serializer(), payload).tabId

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

    fun title(payload: ByteArray): RemoteTitleEvent = decode(TitleReply.serializer(), payload).let {
        if (it.title.length > 4_096) malformed()
        RemoteTitleEvent(it.tabId, it.attachmentId, it.title)
    }

    fun terminalExited(payload: ByteArray): RemoteTerminalExitEvent =
        decode(TerminalExitedReply.serializer(), payload).let {
            if (it.tabId.isBlank() || it.attachmentId.isBlank()) malformed()
            RemoteTerminalExitEvent(it.tabId, it.attachmentId, it.exit)
        }

    private fun <T> encode(serializer: kotlinx.serialization.KSerializer<T>, value: T): ByteArray =
        cbor.encodeToByteArray(serializer, value).also {
            if (it.isEmpty() || it.size >= RemoteWireCodec.MAX_FRAME_BYTES) malformed()
        }

    private fun <T> decode(serializer: kotlinx.serialization.KSerializer<T>, payload: ByteArray): T =
        try {
            RemoteWireCodec.validateCborPayload(payload)
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
    @Serializable private data class AgentStartPayload(
        val action: String = "start",
        @SerialName("agent_id") val agentId: String,
        val model: String?,
        val effort: String?,
        val cwd: String,
        val title: String,
        val size: TerminalSize,
    )
    @Serializable private data class SessionListReply(val sessions: List<RemoteSession>)
    @Serializable private data class SessionPreviewReply(val messages: List<RemotePreviewMessage>)
    @Serializable private data class TabListReply(val tabs: List<RemoteTab>)
    @Serializable private data class AgentListReply(
        val agents: List<RemoteAgentChoice>,
        val caps: Map<String, RemoteAgentCaps>,
    )
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
    @Serializable private data class AgentStartedReply(
        @SerialName("tab_id") val tabId: String,
        @SerialName("session_id") val sessionId: String?,
    )
    @Serializable private data class FocusReply(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
        val focus: WireFocusOwner,
        val size: TerminalSize,
    )
    @Serializable private data class TitleReply(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
        val title: String,
    )
    @Serializable private data class TerminalExitedReply(
        @SerialName("tab_id") val tabId: String,
        @SerialName("attachment_id") val attachmentId: String,
        val exit: RemoteTabExit,
    )
}
