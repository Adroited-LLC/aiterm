package com.adroited.aiterm.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import com.adroited.aiterm.remote.*
import com.adroited.aiterm.ui.theme.AgentIcon
import kotlinx.coroutines.launch

internal enum class SessionMenuAction(val label: String, val icon: ImageVector) {
    Open("Open conversation", Icons.Filled.ChatBubbleOutline),
    Resume("Resume session", Icons.Filled.PlayArrow),
    Fork("Fork session", Icons.Filled.CallSplit),
    Rename("Rename session", Icons.Filled.Edit),
    Star("Star session", Icons.Filled.StarOutline),
    NewShell("New shell here", Icons.Filled.Terminal),
    Close("Close session", Icons.Filled.Close),
    Stop("Stop session", Icons.Filled.Stop),
    Delete("Delete session", Icons.Filled.DeleteOutline);

    val mutation: RemoteSessionMutation? get() = when (this) {
        Fork -> RemoteSessionMutation.Fork
        Close -> RemoteSessionMutation.Close
        Stop -> RemoteSessionMutation.Stop
        Delete -> RemoteSessionMutation.Delete
        else -> null
    }
    val requiresConfirmation get() = this == Close || this == Stop || this == Delete
}

internal fun sessionMenuActions(state: RemoteClientState, session: RemoteSession): List<SessionMenuAction> {
    val hasTab = isConversationSessionLive(session, state.tabs)
    // Activity is an additional reason to refuse deletion while tab discovery catches up.
    val running = hasTab || state.sessionActivity[session.id] in setOf("output", "attention")
    val caps = state.agentCaps[session.agent]
    return buildList {
        add(SessionMenuAction.Open)
        if (!running && caps?.resume == true) add(SessionMenuAction.Resume)
        if (caps?.fork == true) add(SessionMenuAction.Fork)
        add(SessionMenuAction.Rename)
        add(SessionMenuAction.Star)
        add(SessionMenuAction.NewShell)
        if (hasTab) add(SessionMenuAction.Close)
        else if (running) add(SessionMenuAction.Stop)
        if (!running && caps?.delete == true) add(SessionMenuAction.Delete)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SessionActionsSheet(
    state: RemoteClientState,
    session: RemoteSession,
    onDismiss: () -> Unit,
    onAction: (SessionMenuAction) -> Unit,
    onMutate: suspend (RemoteSessionMutation) -> Result<Unit>,
) {
    var confirmation by remember(session.id) { mutableStateOf<SessionMenuAction?>(null) }
    var busy by remember(session.id) { mutableStateOf(false) }
    var error by remember(session.id) { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val actions = sessionMenuActions(state, session)
    val connected = state.connection == ConnectionState.Connected
    fun mutate(action: SessionMenuAction) {
        if (busy || !connected || action !in actions) return
        busy = true
        error = null
        confirmation = null
        scope.launch {
            try {
                onMutate(checkNotNull(action.mutation)).fold(
                    onSuccess = { onDismiss() },
                    onFailure = { error = it.message ?: "The desktop could not complete this action." },
                )
            } finally { busy = false }
        }
    }
    ModalBottomSheet(
        onDismissRequest = { if (!busy) onDismiss() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true, confirmValueChange = { !busy }),
    ) {
        Column(Modifier.verticalScroll(rememberScrollState())) {
        Row(Modifier.padding(horizontal = 24.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            AgentIcon(session.agent, size = 32.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(session.title.ifBlank { "Untitled session" }, style = MaterialTheme.typography.titleLarge)
                Text(session.projectPath, style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2)
            }
        }
        HorizontalDivider(Modifier.padding(vertical = 12.dp))
        if (!connected) Text("Connect to the desktop to manage this session.",
            modifier = Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodyMedium)
        error?.let { Text(it, modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
            color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium) }
        if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        actions.forEach { action ->
            DropdownMenuItem(
                text = { Text(if (action == SessionMenuAction.Star && session.id in state.starredSessions)
                    "Unstar session" else action.label,
                    color = if (action.requiresConfirmation) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface) },
                leadingIcon = { Icon(action.icon, null) },
                enabled = !busy && (connected || action == SessionMenuAction.Open),
                contentPadding = PaddingValues(horizontal = 24.dp),
                onClick = {
                    if (action.requiresConfirmation) confirmation = action
                    else if (action.mutation != null) mutate(action)
                    else onAction(action)
                },
            )
        }
        Spacer(Modifier.height(16.dp))
        }
    }
    confirmation?.let { action ->
        AlertDialog(
            onDismissRequest = { confirmation = null },
            title = { Text(action.label + "?") },
            text = { Text(when (action) {
                SessionMenuAction.Delete -> "Move “${session.title}” to the desktop's trash?"
                SessionMenuAction.Close -> "Stop “${session.title}” and close its running terminal? Its conversation history will be kept."
                else -> "Stop “${session.title}”? Its conversation history will be kept."
            }) },
            confirmButton = {
                TextButton(onClick = { mutate(action) }, enabled = connected && action in actions) { Text(action.label) }
            },
            dismissButton = { TextButton(onClick = { confirmation = null }) { Text("Cancel") } },
        )
    }
}
