package com.adroited.aiterm.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.remote.AuthenticatedRemoteTransport
import com.adroited.aiterm.remote.OkHttpRemoteSocketDialer
import com.adroited.aiterm.remote.RemoteClient
import com.adroited.aiterm.remote.RemoteUploadProgress
import com.adroited.aiterm.remote.RemoteUploadSource
import com.adroited.aiterm.remote.TerminalSize
import com.adroited.aiterm.security.AppLock
import com.adroited.aiterm.security.DeviceKeys
import com.adroited.aiterm.terminal.DefaultTerminalScreenStore
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.collectLatest

class RemoteTerminalViewModel(
    desktop: PairedDesktop,
    deviceKeys: DeviceKeys,
    private val appLock: AppLock,
) : ViewModel() {
    /** Survives terminal tab changes and configuration changes for this ViewModel's lifetime. */
    internal val terminalDrafts = TerminalDraftStore()
    private val screenStore = DefaultTerminalScreenStore()
    private val dialer = OkHttpRemoteSocketDialer()
    val client = RemoteClient(
        transportFactory = {
            AuthenticatedRemoteTransport(
                desktop = desktop,
                deviceKeys = deviceKeys,
                appLock = appLock,
                dialer = dialer,
                scope = viewModelScope,
            )
        },
        screenStore = screenStore,
        isUnlocked = { !appLock.isLocked.value },
        scope = viewModelScope,
    )

    init {
        reconnect()
        viewModelScope.launch {
            appLock.isLocked.collectLatest { locked ->
                if (locked) {
                    client.lock()
                } else if (client.state.value.connection == com.adroited.aiterm.remote.ConnectionState.Locked) {
                    reconnect()
                }
            }
        }
    }

    fun reconnect() {
        viewModelScope.launch {
            if (client.connect()) {
                client.refreshSessions()
                client.refreshAgents()
            }
        }
    }

    fun selectTab(tabId: String) = client.selectTab(tabId)
    fun sendInput(text: String) = client.sendInput(text)
    /**
     * Uploads normalized drafts only. Prompt formatting and terminal input stay in the UI submit
     * path so failed uploads can never inject a partial prompt.
     */
    suspend fun uploadImages(
        images: List<NormalizedTerminalImage>,
        onProgress: (RemoteUploadProgress) -> Unit = {},
    ): Result<List<String>> = client.uploadImages(images.map(::remoteUploadSource), onProgress)

    /** Upload counterpart for the immutable images retained by [terminalDrafts]. */
    internal suspend fun uploadDraftImages(
        images: List<TerminalAttachmentImage>,
        onProgress: (RemoteUploadProgress) -> Unit = {},
    ): Result<List<String>> = client.uploadImages(images.map { it.asRemoteUploadSource() }, onProgress)
    fun takeFocus(cols: Int, rows: Int) = client.takeFocus(TerminalSize(cols, rows))
    fun resize(cols: Int, rows: Int) = client.resize(TerminalSize(cols, rows))
    fun loadOlderScrollback() = client.requestNextScrollbackPage()
    fun openSession(id: String, cols: Int, rows: Int) =
        client.openSession(id, TerminalSize(cols, rows))
    fun previewSession(id: String) = client.previewSession(id)
    fun closeSession(id: String) = client.closeSession(id)
    fun stopSession(id: String) = client.stopSession(id)
    fun forkSession(id: String) = client.forkSession(id)
    fun deleteSession(id: String) = client.deleteSession(id)
    fun closeTab(id: String) = client.closeTab(id)
    fun openShell(projectPath: String?, cols: Int, rows: Int) =
        client.openShell(projectPath, TerminalSize(cols, rows))
    fun startAgent(
        agent: com.adroited.aiterm.remote.RemoteAgentChoice,
        modelId: String?,
        effort: String?,
        cwd: String,
        cols: Int,
        rows: Int,
    ) = client.startAgent(agent, modelId, effort, cwd, TerminalSize(cols, rows))

    override fun onCleared() {
        client.lock()
    }

    companion object {
        fun factory(
            desktop: PairedDesktop,
            deviceKeys: DeviceKeys,
            appLock: AppLock,
        ): ViewModelProvider.Factory = viewModelFactory {
            initializer { RemoteTerminalViewModel(desktop, deviceKeys, appLock) }
        }
    }
}

internal fun remoteUploadSource(image: NormalizedTerminalImage): RemoteUploadSource = RemoteUploadSource(
    id = image.id,
    file = image.file,
    length = image.length,
    sha256 = image.sha256.copyOf(),
)
