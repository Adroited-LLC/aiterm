package com.adroited.aiterm.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.remote.RemotePreviewMessage
import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.remote.RemoteTab
import kotlinx.coroutines.launch

private const val PAGE_SESSIONS = "sessions"
private const val PAGE_CONVERSATION = "conversation"
private const val PAGE_TERMINAL = "terminal"

/** Conversation-first shell inspired by the 5lime client, backed only by our remote protocol. */
@Composable
fun RemoteDesktopScreen(
    viewModel: RemoteTerminalViewModel,
    desktopName: String,
    onBack: () -> Unit,
    keyBarPreference: TerminalKeyBarPreference,
) {
    val state by viewModel.client.state.collectAsStateWithLifecycle()
    var page by rememberSaveable { mutableStateOf(PAGE_SESSIONS) }
    var selectedSessionId by rememberSaveable { mutableStateOf<String?>(null) }
    val selected = selectedSessionId?.let { id -> state.sessions.firstOrNull { it.id == id } }

    when (page) {
        PAGE_TERMINAL -> RemoteTerminalScreen(
            viewModel = viewModel,
            onBack = {
                selectedSessionId?.let(viewModel::previewSession)
                page = if (selectedSessionId == null) PAGE_SESSIONS else PAGE_CONVERSATION
            },
            keyBarPreference = keyBarPreference,
        )

        PAGE_CONVERSATION -> if (selected == null) {
            LaunchedEffect(selectedSessionId) {
                selectedSessionId = null
                page = PAGE_SESSIONS
            }
        } else {
            RemoteConversationContent(
                state = state,
                session = selected,
                onBack = { page = PAGE_SESSIONS },
                onRefresh = { viewModel.previewSession(selected.id) },
                onOpenTerminal = {
                    state.tabs.firstOrNull { it.sessionId == selected.id }
                        ?.let { viewModel.selectTab(it.id) }
                        ?: viewModel.openSession(selected.id, 80, 24)
                    page = PAGE_TERMINAL
                },
                onSend = viewModel::sendConversationPrompt,
            )
        }

        else -> RemoteSessionDashboard(
            state = state,
            desktopName = desktopName,
            onBack = onBack,
            onReconnect = viewModel::reconnect,
            onRefresh = { viewModel.client.refreshSessions() },
            onOpenSession = { session ->
                selectedSessionId = session.id
                viewModel.previewSession(session.id)
                page = PAGE_CONVERSATION
            },
            onOpenTerminal = {
                selectedSessionId = null
                page = PAGE_TERMINAL
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RemoteSessionDashboard(
    state: RemoteClientState,
    desktopName: String,
    onBack: () -> Unit,
    onReconnect: () -> Unit,
    onRefresh: () -> Unit,
    onOpenSession: (RemoteSession) -> Unit,
    onOpenTerminal: () -> Unit,
) {
    var query by rememberSaveable { mutableStateOf("") }
    val sessions = remember(state.sessions, state.tabs, query) {
        conversationSessions(state.sessions, state.tabs, query)
    }
    Scaffold(
        topBar = {
            TopAppBar(
                navigationIcon = { TextButton(onClick = onBack) { Text("Desktops") } },
                title = {
                    Column {
                        Text(desktopName, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        ConnectionLabel(state.connection)
                    }
                },
                actions = {
                    TextButton(onClick = onRefresh) { Text("Refresh") }
                    TextButton(onClick = onOpenTerminal) { Text("Terminal") }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
        containerColor = MaterialTheme.colorScheme.background,
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            state.lastError?.let { error ->
                Row(
                    Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.errorContainer)
                        .padding(horizontal = 14.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(error, color = MaterialTheme.colorScheme.onErrorContainer, modifier = Modifier.weight(1f))
                    if (state.connection == ConnectionState.Disconnected) {
                        TextButton(onClick = onReconnect) { Text("Reconnect") }
                    }
                }
            }
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
                placeholder = { Text("Search sessions") },
                singleLine = true,
                shape = RoundedCornerShape(14.dp),
            )
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("SESSIONS", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                Spacer(Modifier.weight(1f))
                Text(
                    "${state.tabs.count { it.sessionId != null }} live · ${state.sessions.size} total",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            when {
                state.sessions.isEmpty() -> DashboardEmptyState(state.connection)
                sessions.isEmpty() -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text("No sessions match that search.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                else -> LazyColumn(
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(bottom = 20.dp),
                ) {
                    items(sessions, key = RemoteSession::id) { session ->
                        SessionDashboardRow(
                            session = session,
                            live = isConversationSessionLive(session, state.tabs),
                            onClick = { onOpenSession(session) },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun DashboardEmptyState(connection: ConnectionState) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            if (connection == ConnectionState.Connecting || connection == ConnectionState.Reconnecting) {
                CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
                Spacer(Modifier.height(12.dp))
                Text("Reading sessions from the desktop…", color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else {
                Text("No sessions on this desktop yet.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

@Composable
private fun SessionDashboardRow(session: RemoteSession, live: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(38.dp)
                .background(agentColor(session.agent).copy(alpha = 0.16f), CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                session.agent.take(1).uppercase(),
                color = agentColor(session.agent),
                fontWeight = FontWeight.Bold,
            )
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                session.title.ifBlank { "Untitled session" },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                "${session.agent} · ${session.projectPath.trimEnd('/').substringAfterLast('/').ifBlank { session.projectPath }}",
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Column(horizontalAlignment = Alignment.End) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier.size(7.dp).background(
                        if (live) MaterialTheme.colorScheme.tertiary else MaterialTheme.colorScheme.outline,
                        CircleShape,
                    ),
                )
                Spacer(Modifier.width(6.dp))
                Text(
                    if (live) "LIVE" else relativeSessionTime(session.lastActive),
                    style = MaterialTheme.typography.labelMedium,
                    color = if (live) MaterialTheme.colorScheme.tertiary else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.45f))
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RemoteConversationContent(
    state: RemoteClientState,
    session: RemoteSession,
    onBack: () -> Unit,
    onRefresh: () -> Unit,
    onOpenTerminal: () -> Unit,
    onSend: suspend (String, String) -> Result<Unit>,
) {
    BackHandler(onBack = onBack)
    var draft by rememberSaveable(session.id) { mutableStateOf("") }
    var sending by remember(session.id) { mutableStateOf(false) }
    var sendError by remember(session.id) { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val messages = if (state.previewSessionId == session.id) state.previewMessages else emptyList()
    val listState = rememberLazyListState()

    LaunchedEffect(session.id) { onRefresh() }
    LaunchedEffect(messages.size) {
        if (messages.isNotEmpty()) listState.animateScrollToItem(messages.lastIndex)
    }

    fun submit() {
        val text = draft.trim()
        if (text.isEmpty() || sending) return
        sending = true
        sendError = null
        scope.launch {
            onSend(session.id, text).fold(
                onSuccess = { draft = "" },
                onFailure = { sendError = it.message ?: "The desktop did not accept the message." },
            )
            sending = false
        }
    }

    Scaffold(
        modifier = Modifier.imePadding(),
        topBar = {
            TopAppBar(
                navigationIcon = { TextButton(onClick = onBack) { Text("Sessions") } },
                title = {
                    Column {
                        Text(session.title.ifBlank { "Untitled session" }, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            "${session.agent} · ${if (isConversationSessionLive(session, state.tabs)) "live" else "history"}",
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                },
                actions = {
                    TextButton(onClick = onRefresh) { Text("Refresh") }
                    TextButton(onClick = onOpenTerminal) { Text("Terminal") }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
        bottomBar = {
            Column(
                Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface)
                    .padding(horizontal = 10.dp, vertical = 8.dp),
            ) {
                sendError?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                    Spacer(Modifier.height(4.dp))
                }
                Row(verticalAlignment = Alignment.Bottom) {
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it },
                        modifier = Modifier.weight(1f),
                        placeholder = { Text("Message ${session.agent}") },
                        maxLines = 5,
                        shape = RoundedCornerShape(18.dp),
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                        keyboardActions = KeyboardActions(onSend = { submit() }),
                        enabled = !sending,
                    )
                    Spacer(Modifier.width(8.dp))
                    Button(onClick = { submit() }, enabled = draft.isNotBlank() && !sending) {
                        if (sending) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        else Text("Send")
                    }
                }
            }
        },
        containerColor = MaterialTheme.colorScheme.background,
    ) { padding ->
        when {
            state.previewSessionId != session.id -> Box(
                Modifier.fillMaxSize().padding(padding),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }
            messages.isEmpty() -> Box(
                Modifier.fillMaxSize().padding(padding),
                contentAlignment = Alignment.Center,
            ) { Text("No conversation history yet.", color = MaterialTheme.colorScheme.onSurfaceVariant) }
            else -> LazyColumn(
                state = listState,
                modifier = Modifier.fillMaxSize().padding(padding),
                contentPadding = PaddingValues(horizontal = 12.dp, vertical = 10.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                itemsIndexed(messages) { _, message ->
                    ConversationTurn(message)
                }
            }
        }
    }
}

@Composable
private fun ConversationTurn(message: RemotePreviewMessage) {
    when (message.role.lowercase()) {
        "user" -> Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
            Box(
                Modifier.widthIn(max = 330.dp)
                    .background(MaterialTheme.colorScheme.primaryContainer, RoundedCornerShape(18.dp, 18.dp, 5.dp, 18.dp))
                    .padding(horizontal = 13.dp, vertical = 10.dp),
            ) { ConversationMarkdown(message.text, MaterialTheme.colorScheme.onPrimaryContainer) }
        }
        "assistant" -> Column(Modifier.fillMaxWidth().padding(end = 14.dp)) {
            ConversationMarkdown(message.text)
        }
        "thinking" -> Text(
            message.text,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontStyle = FontStyle.Italic,
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
        )
        else -> Text(
            message.text,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontFamily = FontFamily.Monospace,
            style = MaterialTheme.typography.labelMedium,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
        )
    }
}

@Composable
private fun ConnectionLabel(connection: ConnectionState) {
    val (label, color) = when (connection) {
        ConnectionState.Connected -> "connected" to MaterialTheme.colorScheme.tertiary
        ConnectionState.Connecting -> "connecting" to MaterialTheme.colorScheme.primary
        ConnectionState.Reconnecting -> "reconnecting" to MaterialTheme.colorScheme.primary
        ConnectionState.Locked -> "locked" to MaterialTheme.colorScheme.error
        ConnectionState.Revoked -> "revoked" to MaterialTheme.colorScheme.error
        ConnectionState.Disconnected -> "offline" to MaterialTheme.colorScheme.onSurfaceVariant
    }
    Text(label, style = MaterialTheme.typography.labelMedium, color = color)
}

internal fun isConversationSessionLive(session: RemoteSession, tabs: List<RemoteTab>): Boolean =
    tabs.any { it.sessionId == session.id }

internal fun conversationSessions(
    sessions: List<RemoteSession>,
    tabs: List<RemoteTab>,
    query: String,
): List<RemoteSession> {
    val needle = query.trim().lowercase()
    return sessions.asSequence()
        .filter { session ->
            needle.isEmpty() || listOf(session.title, session.agent, session.projectPath, session.groupPath)
                .any { needle in it.lowercase() }
        }
        .sortedWith(
            compareByDescending<RemoteSession> { isConversationSessionLive(it, tabs) }
                .thenByDescending { it.lastActive },
        )
        .toList()
}

private fun relativeSessionTime(lastActive: Long, nowMillis: Long = System.currentTimeMillis()): String {
    if (lastActive <= 0) return "RECENT"
    val timestamp = if (lastActive > 100_000_000_000L) lastActive else lastActive * 1_000
    val seconds = ((nowMillis - timestamp) / 1_000).coerceAtLeast(0)
    return when {
        seconds < 60 -> "NOW"
        seconds < 3_600 -> "${seconds / 60}M"
        seconds < 86_400 -> "${seconds / 3_600}H"
        else -> "${seconds / 86_400}D"
    }
}

private fun agentColor(agent: String): Color = when (agent.lowercase()) {
    "claude", "anthropic" -> Color(0xFFE8956B)
    "codex", "openai" -> Color(0xFF7DB7FF)
    "grok", "xai" -> Color(0xFFB0BEC5)
    "opencode" -> Color(0xFFB39DDB)
    else -> Color(0xFF75D8B4)
}
