package com.adroited.aiterm.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.UriHandler
import com.adroited.aiterm.remote.RemoteSessionChange
import java.net.URI
import java.nio.file.Paths

/** Host paths must never be sent to Android's local file/intent handler. */
internal fun conversationFilePath(target: String): String? {
    if (target.any { it.isISOControl() }) return null
    val path = when {
        target.startsWith("/") && !target.startsWith("//") -> target
        target.startsWith("file:", ignoreCase = true) -> {
            val uri = runCatching { URI(target) }.getOrNull() ?: return null
            if (uri.isOpaque || uri.rawQuery != null || uri.rawFragment != null) return null
            if (!uri.rawAuthority.isNullOrEmpty() && !uri.rawAuthority.equals("localhost", true)) return null
            uri.path ?: return null
        }
        else -> return null
    }.replace(Regex(":\\d+(?::\\d+)?$"), "")
    if (!path.startsWith("/") || path.startsWith("//") || path.any { it.isISOControl() }) return null
    return runCatching { Paths.get(path).normalize().toString() }.getOrNull()
}

internal fun conversationLinkedFile(
    path: String,
    projectPath: String,
    files: List<RemoteSessionChange>,
): RemoteSessionChange? = files.firstOrNull { file ->
    val absolute = if (file.path.startsWith("/")) file.path else "${projectPath.trimEnd('/')}/${file.path}"
    // Ledger paths are literal filenames, not links with line-number suffixes.
    runCatching { Paths.get(absolute).normalize().toString() }.getOrNull() == path
}

@Composable
internal fun ConversationLinkHandler(
    onOpenFile: (String) -> Unit,
    onError: (String) -> Unit,
    content: @Composable () -> Unit,
) {
    val parent = LocalUriHandler.current
    val openFile = rememberUpdatedState(onOpenFile)
    val error = rememberUpdatedState(onError)
    val handler = remember(parent) {
        object : UriHandler {
            override fun openUri(uri: String) {
                val path = conversationFilePath(uri)
                when {
                    path != null -> openFile.value(path)
                    isSafeRemoteLink(uri) -> runCatching { parent.openUri(uri) }
                        .onFailure { error.value("Could not open this link.") }
                    else -> error.value("This link is not a supported desktop file or web address.")
                }
            }
        }
    }
    CompositionLocalProvider(LocalUriHandler provides handler, content = content)
}
