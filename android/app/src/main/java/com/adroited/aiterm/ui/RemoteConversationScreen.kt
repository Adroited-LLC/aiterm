package com.adroited.aiterm.ui

import android.graphics.BitmapFactory
import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imeNestedScroll
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.text.selection.DisableSelection
import androidx.compose.foundation.text.selection.rememberSelectionState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.rememberDrawerState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.InsertDriveFile
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.CameraAlt
import androidx.compose.material.icons.filled.Code
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Image as ImageIcon
import androidx.compose.material.icons.filled.PictureAsPdf
import androidx.compose.material.icons.filled.PhotoLibrary
import androidx.compose.material.icons.filled.Screenshot
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.material.icons.filled.Audiotrack
import androidx.compose.material.icons.filled.Language
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Speed
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.remote.RemoteAgentChoice
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.remote.RemotePreviewMessage
import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.remote.RemoteSessionChange
import com.adroited.aiterm.remote.RemoteMarkdownDocument
import com.adroited.aiterm.remote.RemoteTab
import com.adroited.aiterm.remote.RemoteUploadProgress
import com.adroited.aiterm.remote.RemoteUsageSource
import com.adroited.aiterm.remote.SpinePhase
import com.adroited.aiterm.ui.theme.AgentIcon
import java.io.File
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private const val PAGE_SESSIONS = "sessions"
private const val PAGE_CONVERSATION = "conversation"
private const val PAGE_TERMINAL = "terminal"
private const val PAGE_WEB_PREVIEW = "web_preview"

/** Conversation-first shell inspired by the 5lime client, backed only by our remote protocol. */
@Composable
fun RemoteDesktopScreen(
    viewModel: RemoteTerminalViewModel,
    desktop: PairedDesktop,
    pairedDesktops: List<PairedDesktop>,
    onBack: () -> Unit,
    onOpenDesktop: (PairedDesktop) -> Unit,
    keyBarPreference: TerminalKeyBarPreference,
) {
    val state by viewModel.client.state.collectAsStateWithLifecycle()
    var page by rememberSaveable { mutableStateOf(PAGE_SESSIONS) }
    var selectedSessionId by rememberSaveable { mutableStateOf<String?>(null) }
    var webPreviewUrl by rememberSaveable { mutableStateOf<String?>(null) }
    val selected = selectedSessionId?.let { id -> state.sessions.firstOrNull { it.id == id } }

    when (page) {
        PAGE_WEB_PREVIEW -> {
            val url = webPreviewUrl
            if (url == null) {
                LaunchedEffect(Unit) { page = PAGE_CONVERSATION }
            } else {
                RemoteWebPreviewScreen(
                    url = url,
                    serverSpkiFingerprint = viewModel.desktopSpkiFingerprint(),
                    onClose = {
                        webPreviewUrl = null
                        page = PAGE_CONVERSATION
                    },
                )
            }
        }

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
                onSend = viewModel::sendConversationPrompt,
                onBringIn = viewModel.client::bringInSession,
                onStar = viewModel.client::starSession,
                onOpen = { viewModel.openSession(it, 80, 24) },
                onStop = viewModel::stopSession,
                onLoadFiles = viewModel::sessionChanges,
                onLoadFile = viewModel::sessionFilePreview,
                onParseMarkdown = viewModel::parseMarkdown,
                onSaveMarkdown = viewModel::saveMarkdown,
                onRenderSvg = viewModel::renderSvg,
                onProbeWebPreview = viewModel::hasWebPreview,
                onOpenWebPreview = viewModel::openWebPreview,
                onShowWebPreview = { url ->
                    webPreviewUrl = url
                    page = PAGE_WEB_PREVIEW
                },
                onSelectSession = { target ->
                    selectedSessionId = target.id
                    viewModel.previewSession(target.id)
                },
                onQuickInput = { tabId, key -> viewModel.sendInputs(tabId, listOf(key)) },
            )
        }

        else -> RemoteSessionDashboard(
            state = state,
            desktop = desktop,
            pairedDesktops = pairedDesktops,
            onOpenDesktop = onOpenDesktop,
            onManageDesktops = onBack,
            onRefresh = { viewModel.client.refreshSessions() },
            onLoadUsage = viewModel.client::refreshUsage,
            onStarSession = viewModel.client::starSession,
            onRenameSession = viewModel.client::renameSession,
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
internal fun RemoteSessionDashboard(
    state: RemoteClientState,
    desktop: PairedDesktop,
    pairedDesktops: List<PairedDesktop>,
    onOpenDesktop: (PairedDesktop) -> Unit,
    onManageDesktops: () -> Unit,
    onRefresh: () -> Unit,
    onLoadUsage: () -> Unit,
    onStarSession: (String, Boolean) -> Unit,
    onRenameSession: (String, String) -> Unit,
    onOpenSession: (RemoteSession) -> Unit,
    onOpenTerminal: () -> Unit,
) {
    var query by rememberSaveable { mutableStateOf("") }
    var agentFilter by rememberSaveable { mutableStateOf<String?>(null) }
    var filesOnly by rememberSaveable { mutableStateOf(false) }
    var activeOnly by rememberSaveable { mutableStateOf(false) }
    var foldedCrews by remember { mutableStateOf(emptySet<String>()) }
    var renameTarget by remember { mutableStateOf<RemoteSession?>(null) }
    var pullRefreshing by remember { mutableStateOf(false) }
    LaunchedEffect(state.sessionsRefreshing, pullRefreshing) {
        if (!state.sessionsRefreshing) pullRefreshing = false
    }
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val drawerScope = rememberCoroutineScope()
    val agents = remember(state.sessions) { state.sessions.map { it.agent }.distinct().sorted() }
    val sessions = remember(
        state.sessions,
        state.tabs,
        state.sessionsWithFiles,
        state.starredSessions,
        state.broughtInSessions,
        query,
        agentFilter,
        filesOnly,
        activeOnly,
        foldedCrews,
    ) {
        conversationSessions(
            sessions = state.sessions,
            tabs = state.tabs,
            query = query,
            starred = state.starredSessions,
            withFiles = state.sessionsWithFiles,
            broughtIn = state.broughtInSessions,
            agentFilter = agentFilter,
            filesOnly = filesOnly,
            activeOnly = activeOnly,
            foldedCrews = foldedCrews,
        )
    }
    renameTarget?.let { session ->
        SessionRenameDialog(
            session = session,
            onRename = { title ->
                onRenameSession(session.id, title)
                renameTarget = null
            },
            onDismiss = { renameTarget = null },
        )
    }
    LaunchedEffect(state.connection) {
        while (state.connection == ConnectionState.Connected) {
            delay(3_000)
            onRefresh()
        }
    }
    ModalNavigationDrawer(
        drawerState = drawerState,
        drawerContent = {
            RemoteAppDrawer(
                state = state,
                desktop = desktop,
                pairedDesktops = pairedDesktops,
                onClose = { drawerScope.launch { drawerState.close() } },
                onOpenDesktop = { target ->
                    drawerScope.launch { drawerState.close() }
                    onOpenDesktop(target)
                },
                onLoadUsage = onLoadUsage,
                onManageDesktops = {
                    drawerScope.launch { drawerState.close() }
                    onManageDesktops()
                },
            )
        },
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    navigationIcon = {
                        IconButton(
                            onClick = { drawerScope.launch { drawerState.open() } },
                            modifier = Modifier.semantics { contentDescription = "Open menu" },
                        ) {
                            Icon(Icons.Filled.Menu, contentDescription = "Open menu")
                        }
                    },
                    title = {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            ConnectionDot(state.connection)
                            Spacer(Modifier.width(10.dp))
                            Column {
                                Text(desktop.displayName, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                ConnectionLabel(state.connection, state.connectedEndpoint?.path)
                            }
                        }
                    },
                    actions = {
                        IconButton(
                            onClick = onOpenTerminal,
                            enabled = state.connection == ConnectionState.Connected,
                        ) {
                            Icon(
                                Icons.Filled.Terminal,
                                contentDescription = "Open terminal",
                                tint = if (state.connection == ConnectionState.Connected) {
                                    MaterialTheme.colorScheme.primary
                                } else {
                                    MaterialTheme.colorScheme.onSurfaceVariant
                                },
                            )
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
                )
            },
            containerColor = MaterialTheme.colorScheme.background,
        ) { padding ->
            Column(Modifier.fillMaxSize().padding(padding)) {
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
                placeholder = { Text("Search sessions…") },
                leadingIcon = {
                    Icon(
                        Icons.Filled.Search,
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                },
                singleLine = true,
                shape = RoundedCornerShape(12.dp),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                    unfocusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                    focusedIndicatorColor = Color.Transparent,
                    unfocusedIndicatorColor = Color.Transparent,
                ),
            )
            LazyRow(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(7.dp),
                contentPadding = PaddingValues(bottom = 7.dp),
            ) {
                items(agents, key = { "agent:$it" }) { agent ->
                    FilterChip(
                        selected = agentFilter == agent,
                        onClick = { agentFilter = agent.takeUnless { it == agentFilter } },
                        label = { Text(agent.replaceFirstChar(Char::uppercase), maxLines = 1) },
                        leadingIcon = { AgentIcon(agent, size = 16.dp) },
                    )
                }
                item(key = "files") {
                    FilterChip(
                        selected = filesOnly,
                        onClick = { filesOnly = !filesOnly },
                        label = { Text("Has files") },
                    )
                }
                item(key = "active") {
                    FilterChip(
                        selected = activeOnly,
                        onClick = { activeOnly = !activeOnly },
                        label = { Text("Active") },
                    )
                }
            }
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("SESSIONS", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                Spacer(Modifier.weight(1f))
                Text(
                    "${liveConversationCount(state.sessions, state.tabs)} live · ${state.sessions.size} total",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            PullToRefreshBox(
                isRefreshing = pullRefreshing && state.sessionsRefreshing,
                onRefresh = { pullRefreshing = true; onRefresh(); onLoadUsage() },
                modifier = Modifier.fillMaxSize(),
            ) {
                when {
                    state.sessions.isEmpty() -> DashboardEmptyState(state.connection)
                    sessions.isEmpty() -> Box(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), contentAlignment = Alignment.Center) {
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
                                activity = state.sessionActivity[session.id],
                                starred = session.id in state.starredSessions,
                                hasFiles = session.id in state.sessionsWithFiles,
                                satellite = state.broughtInSessions[session.id]?.let { parent ->
                                    sessions.any { it.id == parent }
                                } == true,
                                crewAgents = state.broughtInSessions
                                    .filterValues { it == session.id }
                                    .keys
                                    .mapNotNull { child -> state.sessions.firstOrNull { it.id == child }?.agent },
                                crewFolded = session.id in foldedCrews,
                                onToggleCrew = {
                                    foldedCrews = if (session.id in foldedCrews) {
                                        foldedCrews - session.id
                                    } else {
                                        foldedCrews + session.id
                                    }
                                },
                                onToggleStar = { onStarSession(session.id, session.id !in state.starredSessions) },
                                onRename = { renameTarget = session },
                                onClick = { onOpenSession(session) },
                            )
                        }
                    }
                }
            }
            }
        }
    }
}

@Composable
internal fun RemoteAppDrawer(
    state: RemoteClientState,
    desktop: PairedDesktop,
    pairedDesktops: List<PairedDesktop>,
    onClose: () -> Unit,
    onOpenDesktop: (PairedDesktop) -> Unit,
    onLoadUsage: () -> Unit,
    onManageDesktops: () -> Unit,
) {
    LaunchedEffect(Unit) { onLoadUsage() }
    ModalDrawerSheet(
        modifier = Modifier.fillMaxHeight().widthIn(max = 340.dp),
        drawerContainerColor = MaterialTheme.colorScheme.surface,
    ) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(vertical = 14.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, bottom = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ConnectionDot(state.connection)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(
                        desktop.displayName,
                        style = MaterialTheme.typography.titleLarge,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    ConnectionLabel(state.connection, state.connectedEndpoint?.path)
                }
                IconButton(onClick = onClose) {
                    Icon(Icons.Filled.Close, contentDescription = "Close menu")
                }
            }

            if (pairedDesktops.size > 1) {
                DrawerSectionLabel("Desktops")
                pairedDesktops.forEach { candidate ->
                    DrawerRow(
                        title = candidate.displayName,
                        detail = if (candidate.deviceId == desktop.deviceId) "Current desktop" else "Paired desktop",
                        icon = Icons.Filled.Devices,
                        selected = candidate.deviceId == desktop.deviceId,
                        onClick = { if (candidate.deviceId != desktop.deviceId) onOpenDesktop(candidate) },
                    )
                }
            }

            HorizontalDivider(Modifier.padding(vertical = 10.dp), color = MaterialTheme.colorScheme.surfaceVariant)
            DrawerUsage(state.usage)
            HorizontalDivider(Modifier.padding(vertical = 10.dp), color = MaterialTheme.colorScheme.surfaceVariant)
            DrawerRow("Manage desktops", "View and remove trusted computers", Icons.Filled.Devices, onClick = onManageDesktops)
        }
    }
}

@Composable
private fun ConnectionDot(connection: ConnectionState) {
    val color = when (connection) {
        ConnectionState.Connected -> MaterialTheme.colorScheme.tertiary
        ConnectionState.Connecting, ConnectionState.Reconnecting -> MaterialTheme.colorScheme.primary
        ConnectionState.Locked, ConnectionState.Revoked -> MaterialTheme.colorScheme.error
        ConnectionState.Disconnected -> MaterialTheme.colorScheme.onSurfaceVariant
    }
    val transition = rememberInfiniteTransition(label = "connection-pulse")
    val alpha by transition.animateFloat(
        initialValue = if (connection == ConnectionState.Connecting || connection == ConnectionState.Reconnecting) .28f else 1f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(720), RepeatMode.Reverse),
        label = "connection-alpha",
    )
    Box(Modifier.size(10.dp).background(color.copy(alpha = alpha), CircleShape))
}

/** John’s compact operational badge: state stays beside identity instead of
 * competing with the conversation as a full-width banner. */
@Composable
private fun SessionStateChip(label: String, color: Color) {
    val transition = rememberInfiniteTransition(label = "session-state")
    val alpha by transition.animateFloat(
        initialValue = if (label == "working") .35f else 1f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(760), RepeatMode.Reverse),
        label = "session-state-alpha",
    )
    Row(
        modifier = Modifier.background(color.copy(alpha = 0.14f), RoundedCornerShape(50))
            .padding(horizontal = 7.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(6.dp).background(color.copy(alpha = alpha), CircleShape))
        Spacer(Modifier.width(5.dp))
        Text(label, style = MaterialTheme.typography.labelSmall, color = color, maxLines = 1)
    }
}

@Composable
private fun DrawerSectionLabel(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
    )
}

@Composable
private fun DrawerRow(
    title: String,
    detail: String,
    icon: androidx.compose.ui.graphics.vector.ImageVector? = null,
    selected: Boolean = false,
    titleColor: Color = MaterialTheme.colorScheme.onSurface,
    onClick: () -> Unit,
) {
    val background = if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.12f) else Color.Transparent
    Row(
        Modifier.fillMaxWidth()
            .padding(horizontal = 10.dp, vertical = 2.dp)
            .background(background, RoundedCornerShape(12.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            Icon(icon, null, tint = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(14.dp))
        }
        Column {
            Text(title, style = MaterialTheme.typography.bodyLarge, color = titleColor)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun DrawerUsage(sources: List<RemoteUsageSource>) {
    var expanded by rememberSaveable { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 6.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth()
                .semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" }
                .clickable(onClickLabel = if (expanded) "Collapse usage" else "Expand usage") { expanded = !expanded }
                .padding(vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.Speed, null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(20.dp))
            Spacer(Modifier.width(10.dp))
            Text("Usage", style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
            Icon(if (expanded) Icons.Filled.ExpandLess else Icons.Filled.ExpandMore, contentDescription = null)
        }
        if (!expanded) return@Column
        if (sources.isEmpty()) {
            Text("Reading account limits…", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 30.dp, top = 5.dp))
        } else {
            sources.forEach { source ->
                Column(Modifier.padding(start = 30.dp, top = 8.dp)) {
                    Row {
                        Text(source.name, style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f))
                        source.plan.takeIf(String::isNotBlank)?.let { Text(it, style = MaterialTheme.typography.labelSmall) }
                    }
                    source.bars.forEach { bar ->
                        Row { Text(bar.label, style = MaterialTheme.typography.labelSmall, modifier = Modifier.weight(1f)); Text("${bar.percent.toInt()}%", style = MaterialTheme.typography.labelSmall) }
                        LinearProgressIndicator(
                            progress = { (bar.percent / 100.0).toFloat() },
                            modifier = Modifier.fillMaxWidth().height(3.dp),
                            color = if (bar.severity == "critical") MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun UsageDialog(sources: List<RemoteUsageSource>, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Usage") },
        text = {
            if (sources.isEmpty()) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.width(10.dp))
                    Text("Reading usage from the desktop…")
                }
            } else {
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 520.dp)) {
                    items(sources, key = { it.id }) { source ->
                        Column(Modifier.fillMaxWidth().padding(vertical = 9.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(
                                    source.name,
                                    style = MaterialTheme.typography.titleMedium,
                                    modifier = Modifier.weight(1f),
                                )
                                source.plan.takeIf(String::isNotBlank)?.let {
                                    Text(it, style = MaterialTheme.typography.labelMedium)
                                }
                            }
                            source.account.takeIf(String::isNotBlank)?.let {
                                Text(
                                    it,
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                            if (source.state != "ok" && source.state != "no_balance") {
                                Text(
                                    source.detail.ifBlank { source.state.replace('_', ' ') },
                                    color = MaterialTheme.colorScheme.error,
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                            source.bars.forEach { bar ->
                                Spacer(Modifier.height(7.dp))
                                Row {
                                    Text(bar.label, style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f))
                                    Text("${bar.percent.toInt()}%", style = MaterialTheme.typography.labelMedium)
                                }
                                LinearProgressIndicator(
                                    progress = { (bar.percent / 100.0).toFloat() },
                                    modifier = Modifier.fillMaxWidth(),
                                    color = when (bar.severity) {
                                        "critical" -> MaterialTheme.colorScheme.error
                                        "warning" -> Color(0xFFFFC857)
                                        else -> MaterialTheme.colorScheme.primary
                                    },
                                )
                            }
                            source.amounts.forEach { amount ->
                                Text(
                                    buildString {
                                        append(amount.label)
                                        append(": ")
                                        if (amount.currency == "USD") append('$')
                                        append("%.2f".format(amount.amount))
                                        amount.of?.let { append(" of %.2f".format(it)) }
                                    },
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                            source.notes.forEach {
                                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.45f))
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

@Composable
private fun DashboardEmptyState(connection: ConnectionState) {
    Box(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), contentAlignment = Alignment.Center) {
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

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun SessionDashboardRow(
    session: RemoteSession,
    live: Boolean,
    activity: String?,
    starred: Boolean,
    hasFiles: Boolean,
    satellite: Boolean,
    crewAgents: List<String>,
    crewFolded: Boolean,
    onToggleCrew: () -> Unit,
    onToggleStar: () -> Unit,
    onRename: () -> Unit,
    onClick: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth()
            .combinedClickable(onClick = onClick, onLongClick = onRename)
            .padding(start = if (satellite) 30.dp else 14.dp, end = 14.dp, top = 11.dp, bottom = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (satellite) {
            Text("↳", color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(6.dp))
        }
        AgentIcon(session.agent, size = if (satellite) 30.dp else 38.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (starred) {
                    Text(
                        "★",
                        color = Color(0xFFFFC857),
                        modifier = Modifier.clickable(onClick = onToggleStar).padding(end = 5.dp),
                    )
                }
                Text(
                    session.title.ifBlank { "Untitled session" },
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f),
                )
            }
            Text(
                buildString {
                    append(relativeSessionTime(session.lastActive).lowercase())
                    append(" · ")
                    append(session.projectPath.trimEnd('/').substringAfterLast('/').ifBlank { session.projectPath })
                    session.branch?.takeIf(String::isNotBlank)?.let { append(" · "); append(it) }
                    if (session.forked) append(" · fork")
                    if (hasFiles) append(" · files")
                },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (!starred) {
                Text(
                    "☆",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.clickable(onClick = onToggleStar).padding(4.dp),
                )
            }
            if (crewAgents.isNotEmpty()) {
                Text(
                    crewAgents.take(3).joinToString("") { it.take(1).uppercase() } +
                        (if (crewAgents.size > 3) "+${crewAgents.size - 3}" else "") +
                        (if (crewFolded) " ›" else " ⌄"),
                    color = MaterialTheme.colorScheme.primary,
                    style = MaterialTheme.typography.labelMedium,
                    modifier = Modifier.background(
                        MaterialTheme.colorScheme.primary.copy(alpha = 0.12f),
                        RoundedCornerShape(9.dp),
                    ).clickable(onClick = onToggleCrew).padding(horizontal = 7.dp, vertical = 6.dp),
                )
            }
            Column(horizontalAlignment = Alignment.End) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier.size(7.dp).background(
                            when {
                                activity == "attention" -> MaterialTheme.colorScheme.error
                                activity == "output" -> MaterialTheme.colorScheme.primary
                                live -> MaterialTheme.colorScheme.tertiary
                                else -> MaterialTheme.colorScheme.outline
                            },
                            CircleShape,
                        ),
                    )
                    Spacer(Modifier.width(6.dp))
                    Text(
                        when {
                            activity == "attention" -> "NEEDS YOU"
                            activity == "output" -> "WORKING"
                            live -> "OPEN"
                            else -> relativeSessionTime(session.lastActive)
                        },
                        style = MaterialTheme.typography.labelMedium,
                        color = when {
                            activity == "attention" -> MaterialTheme.colorScheme.error
                            activity == "output" -> MaterialTheme.colorScheme.primary
                            live -> MaterialTheme.colorScheme.tertiary
                            else -> MaterialTheme.colorScheme.onSurfaceVariant
                        },
                    )
                }
            }
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.45f))
}

@Composable
private fun SessionRenameDialog(
    session: RemoteSession,
    onRename: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var draft by remember(session.id) { mutableStateOf(session.title) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Rename session") },
        text = {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                singleLine = true,
                supportingText = { Text("Leave empty to restore the generated name.") },
            )
        },
        confirmButton = { TextButton(onClick = { onRename(draft) }) { Text("Rename") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun RemoteConversationContent(
    state: RemoteClientState,
    session: RemoteSession,
    onBack: () -> Unit,
    onRefresh: () -> Unit,
    onSend: suspend (
        String,
        String,
        List<TerminalAttachmentImage>,
        (RemoteUploadProgress) -> Unit,
    ) -> Result<Unit>,
    onBringIn: (String, String, String?, String?, String, Int, Boolean) -> Unit,
    onStar: (String, Boolean) -> Unit,
    onOpen: (String) -> Unit,
    onStop: (String) -> Unit,
    onLoadFiles: suspend (String) -> Result<List<RemoteSessionChange>>,
    onLoadFile: suspend (String, String, Int) -> Result<RemoteSessionFilePreview>,
    onParseMarkdown: suspend (String) -> Result<RemoteMarkdownDocument>,
    onSaveMarkdown: suspend (String, String, String, ByteArray) -> Result<ByteArray>,
    onRenderSvg: suspend (String, String) -> Result<ByteArray>,
    onProbeWebPreview: suspend (String) -> Result<Boolean>,
    onOpenWebPreview: suspend (String) -> Result<String>,
    onShowWebPreview: (String) -> Unit,
    onSelectSession: (RemoteSession) -> Unit,
    onQuickInput: (String, String) -> Unit,
) {
    var pullRefreshing by remember(session.id) { mutableStateOf(false) }
    LaunchedEffect(state.previewLoadingSessionId, pullRefreshing) {
        if (state.previewLoadingSessionId != session.id) pullRefreshing = false
    }
    var draft by rememberSaveable(session.id) { mutableStateOf("") }
    var sending by remember(session.id) { mutableStateOf(false) }
    var sendError by remember(session.id) { mutableStateOf<String?>(null) }
    var attachments by remember(session.id) { mutableStateOf(TerminalAttachmentDraft()) }
    var showImageSources by remember(session.id) { mutableStateOf(false) }
    var showFiles by remember(session.id) { mutableStateOf(false) }
    var showBringIn by remember(session.id) { mutableStateOf(false) }
    var showActions by remember(session.id) { mutableStateOf(false) }
    var filesLoading by remember(session.id) { mutableStateOf(false) }
    var files by remember(session.id) { mutableStateOf<List<RemoteSessionChange>>(emptyList()) }
    var filesError by remember(session.id) { mutableStateOf<String?>(null) }
    var filePreviewTarget by remember(session.id) { mutableStateOf<RemoteSessionChange?>(null) }
    var filePreviewLoading by remember(session.id) { mutableStateOf(false) }
    var filePreview by remember(session.id) { mutableStateOf<RemoteSessionFilePreview?>(null) }
    var filePreviewError by remember(session.id) { mutableStateOf<String?>(null) }
    var webPreviewAvailable by remember(session.id) { mutableStateOf(false) }
    var webPreviewOpening by remember(session.id) { mutableStateOf(false) }
    var messageActions by remember(session.id) { mutableStateOf<SpineTimelineItem?>(null) }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val captureView = LocalView.current.rootView
    val keyboard = LocalSoftwareKeyboardController.current
    val composerFocus = remember { FocusRequester() }
    val normalizer = remember(context) { TerminalImageNormalizer(context) }
    val previewItems = if (state.previewSessionId == session.id) state.previewItems else emptyList()
    val timeline = remember(previewItems) { spineTimeline(previewItems) }
    val listState = rememberLazyListState()
    val conversationSelection = rememberSelectionState()
    val working = isConversationWorking(
        phase = state.previewPhase,
        spineLive = state.previewLive,
        turnOpen = state.previewTurnOpen,
        rosterActivity = state.sessionActivity[session.id],
    )
    val live = isConversationSessionLive(session, state.tabs)
    val needsYou = state.previewPhase == SpinePhase.NeedsYou
    val starred = session.id in state.starredSessions
    val imeBottom = WindowInsets.ime.getBottom(LocalDensity.current)
    var positionedAtNewest by remember(session.id) { mutableStateOf(false) }
    var previousTimelineCount by remember(session.id) { mutableStateOf(0) }
    val awayFromNewest by remember(session.id) {
        derivedStateOf {
            val layout = listState.layoutInfo
            val last = layout.visibleItemsInfo.lastOrNull()?.index ?: 0
            layout.totalItemsCount > 0 && last < layout.totalItemsCount - 1
        }
    }
    val latestAttachments by rememberUpdatedState(attachments)

    BackHandler(enabled = !sending && !attachments.preparing) {
        when {
            filePreviewTarget != null -> {
                filePreviewTarget = null
                filePreview = null
                filePreviewError = null
                showFiles = true
            }
            showFiles -> showFiles = false
            else -> onBack()
        }
    }
    DisposableEffect(session.id) {
        onDispose { latestAttachments.items.forEach { it.image.file.delete() } }
    }

    val refreshConversation by rememberUpdatedState(onRefresh)
    // Tab membership is discovery metadata, not a subscription lifetime.
    // Keep reading the visible conversation even when that metadata is stale,
    // and refresh immediately when the transport reconnects.
    LaunchedEffect(session.id, state.connection) {
        if (state.connection == ConnectionState.Connected) {
            while (true) {
                refreshConversation()
                // Push notifications drive updates; this heals missed signals
                // and remains compatible with older polling-only desktops.
                delay(5_000)
            }
        }
    }
    LaunchedEffect(session.id, state.connection) {
        if (state.connection == ConnectionState.Connected) {
            onLoadFiles(session.id).onSuccess { files = it }
        }
    }
    LaunchedEffect(session.id, state.connection) {
        if (state.connection != ConnectionState.Connected) {
            webPreviewAvailable = false
            return@LaunchedEffect
        }
        while (true) {
            onProbeWebPreview(session.id).onSuccess { webPreviewAvailable = it }
            delay(5_000)
        }
    }
    // Text/tool upserts can change a row without changing the row count.
    LaunchedEffect(previewItems, timeline.size, working) {
        val itemCount = conversationListItemCount(timeline.size, working)
        val previousCount = previousTimelineCount
        previousTimelineCount = itemCount
        if (itemCount == 0) return@LaunchedEffect
        if (!positionedAtNewest) {
            listState.scrollToItem(itemCount - 1)
            positionedAtNewest = true
        } else {
            val lastVisible = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            // Judge the reader's position against the list they were reading,
            // before an arbitrarily large catch-up page added more rows.
            if (shouldFollowConversationUpdate(previousCount, lastVisible, listState.isScrollInProgress)) {
                listState.scrollToItem(itemCount - 1)
            }
        }
    }
    LaunchedEffect(imeBottom) {
        if (imeBottom > 0) {
            val newest = conversationListItemCount(timeline.size, working)
            if (newest > 0) listState.scrollToItem(newest - 1)
        }
    }

    fun updateAttachments(
        transition: (TerminalAttachmentDraft) -> TerminalAttachmentDraftUpdate,
    ): TerminalAttachmentDraftUpdate = transition(attachments).also { attachments = it.draft }

    fun handlePickerResult(result: TerminalImagePickerResult) {
        when (result) {
            TerminalImagePickerResult.Cancelled -> Unit
            is TerminalImagePickerResult.Failed -> {
                attachments = attachments.copy(message = result.message)
            }
            is TerminalImagePickerResult.Selected -> scope.launch(start = CoroutineStart.UNDISPATCHED) {
                val preparation = updateAttachments { it.beginPreparation() }
                if (!preparation.accepted) {
                    result.ownedCaptureFiles.forEach(File::delete)
                    return@launch
                }
                try {
                    val distinct = result.uris.distinct()
                    val remaining = TerminalAttachmentDraft.MAX_IMAGES - attachments.items.size
                    var message: String? = null
                    for (uri in distinct.take(remaining.coerceAtLeast(0))) {
                        val normalized = normalizer.normalize(uri).getOrElse { error ->
                            message = terminalImageErrorMessage(error)
                            continue
                        }
                        val added = updateAttachments { it.add(normalized) }
                        if (!added.accepted) {
                            message = added.draft.message
                            normalized.file.delete()
                        }
                    }
                    attachments = attachments.copy(
                        message = when {
                            result.uris.size != distinct.size -> "This image is already attached."
                            distinct.size > remaining -> "You can attach up to 4 images."
                            else -> message
                        },
                    )
                } finally {
                    result.ownedCaptureFiles.forEach(File::delete)
                    updateAttachments { it.finishPreparation() }
                }
            }
        }
    }
    val picker = rememberTerminalImagePicker { _, result -> handlePickerResult(result) }

    fun submit() {
        val text = draft.trim()
        if ((text.isEmpty() && attachments.items.isEmpty()) || sending || attachments.preparing) return
        sending = true
        sendError = null
        if (attachments.items.isNotEmpty()) {
            val began = updateAttachments { it.beginSubmission() }
            if (!began.accepted) {
                sending = false
                return
            }
        }
        val submittedImages = attachments.items.map { it.image }
        scope.launch {
            onSend(session.id, text, submittedImages) { progress ->
                updateAttachments { it.recordProgress(progress.sourceId, progress.sent, progress.total) }
            }.fold(
                onSuccess = {
                    draft = ""
                    val removed = attachments.items
                    attachments = TerminalAttachmentDraft()
                    removed.forEach { it.image.file.delete() }
                },
                onFailure = {
                    sendError = it.message ?: "The desktop did not accept the message."
                    if (attachments.submitting) {
                        updateAttachments { draftState -> draftState.failSubmission(sendError!!) }
                    }
                },
            )
            sending = false
        }
    }

    fun sendAgain(text: String) {
        if (text.isBlank() || sending) return
        sending = true
        sendError = null
        scope.launch {
            onSend(session.id, text, emptyList()) {}.onFailure {
                sendError = it.message ?: "The desktop did not accept the message."
            }
            sending = false
        }
    }

    fun openFile(target: RemoteSessionChange) {
        if (target.kind == "deleted") return
        filePreviewTarget = target
        filePreview = null
        filePreviewError = null
        filePreviewLoading = true
        scope.launch {
            val extension = target.name.substringAfterLast('.', "").lowercase()
            val limit = when (extension) {
                "md", "markdown", "mdx" -> 512 * 1024
                "svg" -> 2 * 1024 * 1024
                else -> 8 * 1024 * 1024
            }
            onLoadFile(session.id, target.path, limit).fold(
                onSuccess = { filePreview = it },
                onFailure = { filePreviewError = it.message ?: "Could not read this file." },
            )
            filePreviewLoading = false
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(
                        onClick = {
                            when {
                                filePreviewTarget != null -> {
                                    filePreviewTarget = null
                                    filePreview = null
                                    filePreviewError = null
                                    showFiles = true
                                }
                                showFiles -> showFiles = false
                                else -> onBack()
                            }
                        },
                        enabled = !sending && !attachments.preparing,
                    ) {
                        Icon(
                            Icons.AutoMirrored.Filled.ArrowBack,
                            contentDescription = when {
                                filePreviewTarget != null -> "Back to files"
                                showFiles -> "Back to conversation"
                                else -> "Back to sessions"
                            },
                        )
                    }
                },
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        AgentIcon(session.agent, size = 26.dp)
                        Spacer(Modifier.width(10.dp))
                        Column {
                            Text(
                                session.title.ifBlank { "Untitled session" },
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                style = MaterialTheme.typography.titleMedium,
                            )
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(
                                    session.groupPath.trimEnd('/').substringAfterLast('/').ifBlank {
                                        session.projectPath.trimEnd('/').substringAfterLast('/')
                                    },
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    modifier = Modifier.widthIn(max = 120.dp),
                                )
                                Spacer(Modifier.width(8.dp))
                                SessionStateChip(
                                    label = when {
                                        state.connection != ConnectionState.Connected -> "disconnected"
                                        state.previewError != null -> "sync interrupted"
                                        needsYou -> "needs you"
                                        working -> "working"
                                        live -> "on desktop"
                                        else -> "history"
                                    },
                                    color = when {
                                        state.connection != ConnectionState.Connected || state.previewError != null -> MaterialTheme.colorScheme.error
                                        needsYou -> MaterialTheme.colorScheme.error
                                        working -> Color(0xFFF6C453)
                                        live -> MaterialTheme.colorScheme.tertiary
                                        else -> MaterialTheme.colorScheme.onSurfaceVariant
                                    },
                                )
                            }
                        }
                    }
                },
                actions = {
                    IconButton(onClick = {
                        if (filePreviewTarget != null) {
                            filePreviewTarget = null
                            filePreview = null
                            filePreviewError = null
                            showFiles = true
                            return@IconButton
                        }
                        if (showFiles) {
                            showFiles = false
                            return@IconButton
                        }
                        showFiles = true
                        filesLoading = true
                        filesError = null
                            scope.launch {
                                onLoadFiles(session.id).fold(
                                    onSuccess = { files = it },
                                    onFailure = { filesError = it.message ?: "Could not load files." },
                                )
                                filesLoading = false
                            }
                        }) {
                        Icon(
                            Icons.Filled.Folder,
                            contentDescription = "Files",
                            tint = if (showFiles || filePreviewTarget != null) {
                                MaterialTheme.colorScheme.primary
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                        )
                    }
                    if (webPreviewAvailable) {
                        IconButton(
                            onClick = {
                                if (webPreviewOpening) return@IconButton
                                webPreviewOpening = true
                                scope.launch {
                                    onOpenWebPreview(session.id).fold(
                                        onSuccess = onShowWebPreview,
                                        onFailure = {
                                            sendError = it.message ?: "Could not open the webpage preview."
                                        },
                                    )
                                    webPreviewOpening = false
                                }
                            },
                            enabled = !webPreviewOpening,
                        ) {
                            if (webPreviewOpening) {
                                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                            } else {
                                Icon(Icons.Filled.Language, contentDescription = "Preview webpage")
                            }
                        }
                    }
                    IconButton(onClick = { showActions = true }) {
                        Icon(Icons.Filled.MoreVert, contentDescription = "Session actions")
                    }
                    DropdownMenu(expanded = showActions, onDismissRequest = { showActions = false }) {
                        DropdownMenuItem(
                            text = { Text("Bring in a second agent") },
                            onClick = { showActions = false; showBringIn = true },
                        )
                        DropdownMenuItem(
                            text = { Text(if (starred) "Unstar" else "Star — keep on top") },
                            onClick = { showActions = false; onStar(session.id, !starred) },
                        )
                        if (!live) {
                            DropdownMenuItem(
                                text = { Text("Open on desktop") },
                                leadingIcon = { Icon(Icons.Filled.PlayArrow, contentDescription = null) },
                                onClick = { showActions = false; onOpen(session.id) },
                            )
                        } else {
                            DropdownMenuItem(
                                text = { Text("Stop session") },
                                leadingIcon = { Icon(Icons.Filled.Stop, contentDescription = null) },
                                onClick = { showActions = false; onStop(session.id) },
                            )
                        }
                        DropdownMenuItem(
                            text = { Text("Refresh") },
                            onClick = { showActions = false; onRefresh() },
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
            )
        },
        bottomBar = {
            if (!showFiles && filePreviewTarget == null) {
            Column(
                Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface)
                    .windowInsetsPadding(WindowInsets.navigationBars.union(WindowInsets.ime))
                    .padding(horizontal = 10.dp, vertical = 8.dp),
            ) {
                sendError?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                    Spacer(Modifier.height(4.dp))
                }
                TerminalAttachmentStrip(
                    draft = attachments,
                    onRemove = { imageId ->
                        val removed = updateAttachments { it.remove(imageId) }
                        removed.removed.forEach { it.image.file.delete() }
                    },
                )
                Row(verticalAlignment = Alignment.Bottom) {
                    IconButton(
                        onClick = { showImageSources = true },
                        enabled = !sending && !attachments.preparing &&
                            attachments.items.size < TerminalAttachmentDraft.MAX_IMAGES,
                    ) {
                        Icon(
                            Icons.Filled.Add,
                            contentDescription = "Attach an image",
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it },
                        modifier = Modifier.weight(1f).focusRequester(composerFocus),
                        placeholder = { Text("Message ${session.agent}") },
                        maxLines = 5,
                        shape = RoundedCornerShape(18.dp),
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                        keyboardActions = KeyboardActions(onSend = { submit() }),
                        enabled = !sending,
                        trailingIcon = {
                            if (sending) {
                                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                            }
                        },
                    )
                }
            }
            }
        },
        containerColor = MaterialTheme.colorScheme.background,
    ) { padding ->
        when {
            filePreviewTarget != null -> SessionFilePreviewPane(
                sessionId = session.id,
                target = filePreviewTarget!!,
                loading = filePreviewLoading,
                preview = filePreview,
                error = filePreviewError,
                onParseMarkdown = onParseMarkdown,
                onSaveMarkdown = onSaveMarkdown,
                onRenderSvg = onRenderSvg,
                modifier = Modifier.fillMaxSize().padding(padding),
            )
            showFiles -> SessionFilesPane(
                loading = filesLoading,
                files = files,
                error = filesError,
                modifier = Modifier.fillMaxSize().padding(padding),
                onRetry = {
                    filesLoading = true
                    filesError = null
                    scope.launch {
                        onLoadFiles(session.id).fold(
                            onSuccess = { files = it },
                            onFailure = { filesError = it.message ?: "Could not load files." },
                        )
                        filesLoading = false
                    }
                },
                onOpen = ::openFile,
            )
            state.previewSessionId != session.id && state.previewLoadingSessionId == session.id -> Box(
                Modifier.fillMaxSize().padding(padding),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }
            state.previewSessionId != session.id -> Column(
                Modifier.fillMaxSize().padding(padding).padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Text(
                    state.previewError ?: "The conversation could not be loaded.",
                    color = MaterialTheme.colorScheme.error,
                )
                Spacer(Modifier.height(8.dp))
                TextButton(onClick = onRefresh) { Text("Retry") }
            }
            previewItems.isEmpty() && !working -> PullToRefreshBox(
                isRefreshing = pullRefreshing && state.previewLoadingSessionId == session.id,
                onRefresh = { pullRefreshing = true; onRefresh() },
                modifier = Modifier.fillMaxSize().padding(padding),
            ) {
                Box(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), contentAlignment = Alignment.Center) {
                    Text("No conversation history yet.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            else -> PullToRefreshBox(
                isRefreshing = pullRefreshing && state.previewLoadingSessionId == session.id,
                onRefresh = { pullRefreshing = true; onRefresh() },
                modifier = Modifier.fillMaxSize().padding(padding),
            ) {
                SelectionContainer(state = conversationSelection) {
                    LazyColumn(
                        state = listState,
                        modifier = Modifier.fillMaxSize().then(conversationScrollIndicator(listState)).then(
                            if (imeBottom > 0) Modifier.imeNestedScroll() else Modifier,
                        ),
                        contentPadding = PaddingValues(horizontal = 12.dp, vertical = 10.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        item(key = "crew") {
                            DisableSelection {
                                Column {
                                    ConversationCrewStrip(
                                        current = session,
                                        sessions = state.sessions,
                                        broughtIn = state.broughtInSessions,
                                        activity = state.sessionActivity,
                                        onSelect = onSelectSession,
                                    )
                                    state.previewError?.let { error ->
                                        Text("Conversation updates interrupted: $error", color = MaterialTheme.colorScheme.error)
                                    }
                                    if (needsYou) {
                                        state.tabs.firstOrNull { it.sessionId == session.id }?.let { tab ->
                                            NeedsYouQuickKeys { key -> onQuickInput(tab.id, key) }
                                        }
                                    }
                                }
                            }
                        }
                        items(timeline, key = { it.key }) { item ->
                            SpineTimelineRow(item, onLongPress = {
                                conversationSelection.clear()
                                messageActions = it
                            })
                        }
                        if (working) {
                            item(key = "working") {
                                DisableSelection {
                                    ConversationWorkingRow(session.agent, state.previewPhaseDetail)
                                }
                            }
                        }
                    }
                }
                if (awayFromNewest) {
                    Text(
                        "Newest ↓",
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.align(Alignment.BottomCenter)
                            .padding(bottom = 10.dp)
                            .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(50))
                            .clickable {
                                scope.launch {
                                    val end = listState.layoutInfo.totalItemsCount - 1
                                    if (end >= 0) listState.animateScrollToItem(end)
                                }
                            }
                            .padding(horizontal = 14.dp, vertical = 7.dp),
                    )
                }
            }
        }
    }

    if (showImageSources) {
        AlertDialog(
            onDismissRequest = { showImageSources = false },
            title = { Text("Attach image") },
            text = {
                Column {
                    Text(
                        "Choose a source",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(bottom = 8.dp),
                    )
                    ListItem(
                        headlineContent = { Text("Camera") },
                        leadingContent = {
                            Icon(Icons.Filled.CameraAlt, contentDescription = null)
                        },
                        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        modifier = Modifier.clickable {
                            showImageSources = false
                            picker.launch(
                                TerminalImageSource.Camera,
                                TerminalAttachmentDraft.MAX_IMAGES - attachments.items.size,
                                session.id,
                            ) { handlePickerResult(it) }
                        },
                    )
                    ListItem(
                        headlineContent = { Text("Screenshot") },
                        supportingContent = { Text("Capture this AITerm screen") },
                        leadingContent = {
                            Icon(Icons.Filled.Screenshot, contentDescription = null)
                        },
                        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        modifier = Modifier.clickable {
                            showImageSources = false
                            scope.launch {
                                // Let Compose remove the dialog before drawing the app window.
                                withFrameNanos { }
                                withFrameNanos { }
                                handlePickerResult(captureTerminalScreenshot(context, captureView))
                            }
                        },
                    )
                    ListItem(
                        headlineContent = { Text("Gallery") },
                        leadingContent = {
                            Icon(Icons.Filled.PhotoLibrary, contentDescription = null)
                        },
                        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        modifier = Modifier.clickable {
                            showImageSources = false
                            picker.launch(
                                TerminalImageSource.Gallery,
                                TerminalAttachmentDraft.MAX_IMAGES - attachments.items.size,
                                session.id,
                            ) { handlePickerResult(it) }
                        },
                    )
                }
            },
            confirmButton = { },
        )
    }
    if (showBringIn) {
        BringInDialog(
            session = session,
            agents = state.agents,
            onBringIn = { agent, model, effort, focus, rounds, auto ->
                onBringIn(session.id, agent, model, effort, focus, rounds, auto)
                showBringIn = false
            },
            onDismiss = { showBringIn = false },
        )
    }
    messageActions?.let { held ->
        val rowItem = (held as? SpineTimelineItem.Row)?.item
        val promptAbove = if (rowItem is com.adroited.aiterm.remote.Item.AgentText) {
            val index = previewItems.indexOfFirst { it.key == rowItem.key }
            previewItems.take(index.coerceAtLeast(0)).lastOrNull { it is com.adroited.aiterm.remote.Item.User }
                ?.let { (it as com.adroited.aiterm.remote.Item.User).text }
        } else null
        ConversationMessageSheet(
            row = held,
            promptAbove = promptAbove,
            onDismiss = { messageActions = null },
            onEdit = { text ->
                draft = text
                scope.launch {
                    composerFocus.requestFocus()
                    keyboard?.show()
                }
            },
            onSendAgain = ::sendAgain,
        )
    }
}

@Composable
private fun SessionFilesPane(
    loading: Boolean,
    files: List<RemoteSessionChange>,
    error: String?,
    modifier: Modifier = Modifier,
    onRetry: () -> Unit,
    onOpen: (RemoteSessionChange) -> Unit,
) {
    when {
        loading && files.isEmpty() -> Box(modifier, contentAlignment = Alignment.Center) {
            CircularProgressIndicator()
        }
        error != null && files.isEmpty() -> Column(
            modifier.padding(32.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            Text(error, color = MaterialTheme.colorScheme.error)
            Spacer(Modifier.height(10.dp))
            TextButton(onClick = onRetry) { Text("Retry") }
        }
        files.isEmpty() -> Box(modifier.padding(32.dp), contentAlignment = Alignment.Center) {
            Text(
                "Nothing produced yet. Files this session writes will appear here.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        else -> LazyColumn(modifier) {
            item(key = "files-heading") {
                Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
                    Text("Files from this session", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "Newest changes first. Tap a file to open it.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            items(files, key = { it.path }) { file ->
                val deleted = file.kind == "deleted"
                ListItem(
                    modifier = Modifier.clickable(enabled = !deleted) { onOpen(file) },
                    colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                    leadingContent = {
                        Icon(
                            imageVector = sessionFileIcon(file),
                            contentDescription = null,
                            tint = when {
                                deleted -> MaterialTheme.colorScheme.error
                                sessionFileType(file) == "image" -> MaterialTheme.colorScheme.primary
                                else -> MaterialTheme.colorScheme.onSurfaceVariant
                            },
                        )
                    },
                    headlineContent = {
                        Text(
                            file.name,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            color = if (deleted) {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            } else {
                                MaterialTheme.colorScheme.onSurface
                            },
                        )
                    },
                    supportingContent = {
                        Text(
                            buildString {
                                append(sessionFileChangeLabel(file.kind))
                                append(" · ")
                                append(sessionFileSizeLabel(file.bytes))
                                append(" · ")
                                append(relativeSessionTime(file.at).lowercase())
                                append('\n')
                                append(file.path)
                            },
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    },
                )
                HorizontalDivider(
                    color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.45f),
                    modifier = Modifier.padding(start = 56.dp),
                )
            }
            if (error != null) {
                item(key = "files-stale-error") {
                    Text(
                        "Could not refresh: $error",
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(16.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun SessionFilePreviewPane(
    sessionId: String,
    target: RemoteSessionChange,
    loading: Boolean,
    preview: RemoteSessionFilePreview?,
    error: String?,
    onParseMarkdown: suspend (String) -> Result<RemoteMarkdownDocument>,
    onSaveMarkdown: suspend (String, String, String, ByteArray) -> Result<ByteArray>,
    onRenderSvg: suspend (String, String) -> Result<ByteArray>,
    modifier: Modifier = Modifier,
) {
    Column(modifier) {
        ListItem(
            colors = ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
            leadingContent = {
                Icon(
                    sessionFileIcon(target),
                    contentDescription = null,
                    tint = if (sessionFileType(target) == "image") {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                )
            },
            headlineContent = {
                Text(target.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
            },
            supportingContent = {
                Text(
                    "${sessionFileSizeLabel(target.bytes)} · ${target.path}",
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    fontFamily = FontFamily.Monospace,
                )
            },
        )
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        RichSessionFilePreviewBody(
            sessionId = sessionId,
            target = target,
            loading = loading,
            preview = preview,
            error = error,
            onParseMarkdown = onParseMarkdown,
            onSaveMarkdown = onSaveMarkdown,
            onRenderSvg = onRenderSvg,
            modifier = Modifier.fillMaxWidth().weight(1f).padding(horizontal = 14.dp, vertical = 10.dp),
        )
    }
}

private fun sessionFileType(file: RemoteSessionChange): String = when (
    file.name.substringAfterLast('.', "").lowercase()
) {
    "png", "jpg", "jpeg", "webp", "gif", "svg" -> "image"
    "pdf" -> "pdf"
    "mp4", "webm", "mkv", "mov", "avi" -> "video"
    "mp3", "m4a", "wav", "ogg", "flac" -> "audio"
    "kt", "kts", "java", "rs", "go", "py", "js", "jsx", "ts", "tsx", "html", "css", "scss",
    "json", "toml", "yaml", "yml", "xml", "sh", "zsh", "bash", "sql" -> "code"
    "txt", "md", "markdown", "log", "csv" -> "text"
    else -> "file"
}

private fun sessionFileIcon(file: RemoteSessionChange) = when {
    file.kind == "deleted" -> Icons.Filled.Delete
    sessionFileType(file) == "image" -> Icons.Filled.ImageIcon
    sessionFileType(file) == "pdf" -> Icons.Filled.PictureAsPdf
    sessionFileType(file) == "video" -> Icons.Filled.Videocam
    sessionFileType(file) == "audio" -> Icons.Filled.Audiotrack
    sessionFileType(file) == "code" -> Icons.Filled.Code
    sessionFileType(file) == "text" -> Icons.Filled.Description
    else -> Icons.AutoMirrored.Filled.InsertDriveFile
}

internal fun sessionFileChangeLabel(kind: String): String = when (kind) {
    "created" -> "Created"
    "modified" -> "Modified"
    "deleted" -> "Deleted"
    else -> kind.replace('_', ' ').replaceFirstChar(Char::uppercase)
}

internal fun sessionFileSizeLabel(bytes: Long): String = when {
    bytes < 1_024 -> "$bytes B"
    bytes < 1_048_576 -> "${bytes / 1_024} KB"
    else -> "%.1f MB".format(bytes / 1_048_576.0)
}

@Composable
private fun BringInDialog(
    session: RemoteSession,
    agents: List<RemoteAgentChoice>,
    onBringIn: (String, String?, String?, String, Int, Boolean) -> Unit,
    onDismiss: () -> Unit,
) {
    val choices = remember(agents, session.agent) { agents.filter { it.id != session.agent } }
    var agentId by remember(choices) { mutableStateOf(choices.firstOrNull()?.id) }
    val agent = choices.firstOrNull { it.id == agentId }
    var model by remember(agentId) { mutableStateOf<String?>(null) }
    var effort by remember(agentId, model) { mutableStateOf<String?>(null) }
    var focus by remember(session.id) { mutableStateOf("") }
    var rounds by remember(session.id) { mutableStateOf(2) }
    var auto by remember(session.id) { mutableStateOf(false) }
    val efforts = agent?.models?.firstOrNull { it.id == model }?.efforts.orEmpty()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Bring in a second agent") },
        text = {
            Column {
                Text(
                    "They read this session and talk it through in a desktop tab. The exchange appears here as it lands.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(12.dp))
                if (choices.isEmpty()) {
                    Text("No other agent is available on this desktop.")
                } else {
                    LazyRow(horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                        items(choices, key = RemoteAgentChoice::id) { choice ->
                            FilterChip(
                                selected = agentId == choice.id,
                                onClick = { agentId = choice.id },
                                label = { Text(choice.displayName) },
                            )
                        }
                    }
                    if (agent?.models?.isNotEmpty() == true) {
                        Text("Model", style = MaterialTheme.typography.labelMedium)
                        LazyRow(horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                            item(key = "default") {
                                FilterChip(
                                    selected = model == null,
                                    onClick = { model = null },
                                    label = { Text("Default") },
                                )
                            }
                            items(agent.models, key = { it.id }) { option ->
                                FilterChip(
                                    selected = model == option.id,
                                    onClick = { model = option.id },
                                    label = { Text(option.displayName) },
                                )
                            }
                        }
                    }
                    if (efforts.isNotEmpty()) {
                        Text("Effort", style = MaterialTheme.typography.labelMedium)
                        LazyRow(horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                            item(key = "default") {
                                FilterChip(
                                    selected = effort == null,
                                    onClick = { effort = null },
                                    label = { Text("Default") },
                                )
                            }
                            items(efforts, key = { it }) { option ->
                                FilterChip(
                                    selected = effort == option,
                                    onClick = { effort = option },
                                    label = { Text(option) },
                                )
                            }
                        }
                    }
                    OutlinedTextField(
                        value = focus,
                        onValueChange = { focus = it },
                        modifier = Modifier.fillMaxWidth(),
                        placeholder = { Text("What should they look at? (optional)") },
                        minLines = 2,
                        maxLines = 4,
                    )
                    Spacer(Modifier.height(8.dp))
                    LazyRow(horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                        items(listOf(1, 2, 3), key = { it }) { count ->
                            FilterChip(
                                selected = rounds == count,
                                onClick = { rounds = count },
                                label = { Text(if (count == 1) "Quick" else if (count == 2) "Normal" else "Long") },
                            )
                        }
                        item(key = "auto") {
                            FilterChip(
                                selected = auto,
                                onClick = { auto = !auto },
                                label = { Text("Auto-approve") },
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    agentId?.let { onBringIn(it, model, effort, focus.trim(), rounds, auto) }
                },
                enabled = agentId != null,
            ) { Text("Bring them in") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun ConversationWorkingRow(agent: String, detail: String = "") {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
        Spacer(Modifier.width(9.dp))
        Column {
            Text(
                "${agent.replaceFirstChar(Char::uppercase)} is working…",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (detail.isNotBlank()) {
                Text(
                    detail,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.78f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
private fun SessionFilePreviewBody(
    loading: Boolean,
    preview: RemoteSessionFilePreview?,
    error: String?,
    modifier: Modifier = Modifier,
) {
    when {
        loading -> Box(
            modifier,
            contentAlignment = Alignment.Center,
        ) { CircularProgressIndicator() }
        error != null -> Box(modifier, contentAlignment = Alignment.Center) {
            Text(error, color = MaterialTheme.colorScheme.error)
        }
        preview == null -> Box(modifier, contentAlignment = Alignment.Center) {
            Text("No preview available.")
        }
        preview.mime.startsWith("image/") && preview.truncated -> Text(
            "This image is larger than the 8 MB phone preview limit (${preview.total} bytes).",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = modifier,
        )
        preview.mime.startsWith("image/") -> {
            val bitmap = remember(preview.data) { decodeBoundedPreviewBitmap(preview.data) }
            if (bitmap == null) {
                Text("Android could not decode this image.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else {
                Image(
                    bitmap = bitmap.asImageBitmap(),
                    contentDescription = preview.path.substringAfterLast('/'),
                    modifier = modifier,
                    contentScale = ContentScale.Fit,
                )
            }
        }
        preview.mime.startsWith("text/") -> LazyColumn(
            modifier,
        ) {
            item {
                SelectionContainer {
                    Text(
                        preview.data.decodeToString() + if (preview.truncated) "\n\n…preview truncated…" else "",
                        fontFamily = FontFamily.Monospace,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
        else -> Text(
            "No inline preview for ${preview.mime}. The file is ${preview.total} bytes.",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = modifier,
        )
    }
}

private fun decodeBoundedPreviewBitmap(data: ByteArray): android.graphics.Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(data, 0, data.size, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
    var sample = 1
    while (bounds.outWidth / sample > 2_048 || bounds.outHeight / sample > 2_048) sample *= 2
    return BitmapFactory.decodeByteArray(
        data,
        0,
        data.size,
        BitmapFactory.Options().apply { inSampleSize = sample },
    )
}

@Composable
private fun ConversationTurn(message: RemotePreviewMessage) {
    when (message.role.lowercase()) {
        "user" -> Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
            val content = remember(message.text) { splitConversationAttachments(message.text) }
            Box(
                Modifier.widthIn(max = 330.dp)
                    .background(MaterialTheme.colorScheme.primaryContainer, RoundedCornerShape(18.dp, 18.dp, 5.dp, 18.dp))
                    .padding(horizontal = 13.dp, vertical = 10.dp),
            ) {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    if (content.text.isNotBlank()) {
                        ConversationMarkdown(content.text, MaterialTheme.colorScheme.onPrimaryContainer)
                    }
                    content.imagePaths.forEach { path ->
                        ConversationActivityRow(
                            label = "Image attachment",
                            summary = path.substringAfterLast('/').ifBlank { path },
                            detail = path,
                            foreground = MaterialTheme.colorScheme.onPrimaryContainer,
                        )
                    }
                }
            }
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
        "system" -> Text(
            message.text,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontFamily = FontFamily.Monospace,
            style = MaterialTheme.typography.labelMedium,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
        )
        else -> ConversationActivityRow(
            label = conversationActivityLabel(message.role),
            summary = conversationActivitySummary(message.text),
            detail = message.text,
        )
    }
}

@Composable
private fun ConversationActivityRow(
    label: String,
    summary: String,
    detail: String,
    foreground: Color = MaterialTheme.colorScheme.onSurfaceVariant,
) {
    var expanded by rememberSaveable(label, detail) { mutableStateOf(false) }
    Column(
        Modifier.fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.58f), RoundedCornerShape(7.dp))
            .clickable { expanded = !expanded }
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                label,
                color = MaterialTheme.colorScheme.primary,
                fontFamily = FontFamily.Monospace,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.labelSmall,
                maxLines = 1,
            )
            Spacer(Modifier.width(8.dp))
            Text(
                summary,
                color = foreground,
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.labelSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(8.dp))
            Text(
                if (expanded) "⌃" else "⌄",
                color = foreground,
                style = MaterialTheme.typography.labelMedium,
            )
        }
        if (expanded) {
            HorizontalDivider(
                modifier = Modifier.padding(top = 7.dp, bottom = 7.dp),
                color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.55f),
            )
            SelectionContainer {
                Text(
                    detail,
                    color = foreground,
                    fontFamily = FontFamily.Monospace,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

@Composable
private fun ConversationActivityGroup(messages: List<RemotePreviewMessage>) {
    var expanded by rememberSaveable(
        messages.firstOrNull()?.role,
        messages.firstOrNull()?.text,
    ) { mutableStateOf(false) }
    Column(
        Modifier.fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.30f), RoundedCornerShape(7.dp)),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth()
                .clickable { expanded = !expanded }
                .padding(horizontal = 10.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "Activity",
                color = MaterialTheme.colorScheme.primary,
                fontFamily = FontFamily.Monospace,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.labelSmall,
            )
            Spacer(Modifier.width(8.dp))
            Text(
                "${messages.size} steps",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.labelSmall,
                modifier = Modifier.weight(1f),
            )
            Text(
                if (expanded) "⌃" else "⌄",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.labelMedium,
            )
        }
        if (expanded) {
            HorizontalDivider(
                color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.45f),
            )
            Column(
                modifier = Modifier.padding(start = 8.dp, top = 6.dp, end = 6.dp, bottom = 6.dp),
                verticalArrangement = Arrangement.spacedBy(5.dp),
            ) {
                messages.forEach { message ->
                    ConversationActivityRow(
                        label = conversationActivityLabel(message.role),
                        summary = conversationActivitySummary(message.text),
                        detail = message.text,
                    )
                }
            }
        }
    }
}

internal sealed interface ConversationTimelineItem {
    data class Turn(val message: RemotePreviewMessage) : ConversationTimelineItem
    data class ActivityGroup(val messages: List<RemotePreviewMessage>) : ConversationTimelineItem
}

/** Consecutive machine activity is one transcript group; human-readable turns remain independent. */
internal fun conversationTimeline(messages: List<RemotePreviewMessage>): List<ConversationTimelineItem> {
    val output = mutableListOf<ConversationTimelineItem>()
    val activity = mutableListOf<RemotePreviewMessage>()
    fun flushActivity() {
        when (activity.size) {
            0 -> Unit
            1 -> output += ConversationTimelineItem.Turn(activity.single())
            else -> output += ConversationTimelineItem.ActivityGroup(activity.toList())
        }
        activity.clear()
    }
    messages.forEach { message ->
        if (isConversationActivity(message.role)) {
            activity += message
        } else {
            flushActivity()
            output += ConversationTimelineItem.Turn(message)
        }
    }
    flushActivity()
    return output
}

internal fun isConversationActivity(role: String): Boolean = role.lowercase() !in setOf(
    "user",
    "assistant",
    "thinking",
    "system",
)

internal data class ConversationAttachmentContent(
    val text: String,
    val imagePaths: List<String>,
)

/** Pulls the terminal submission's generated path list out of the human message. */
internal fun splitConversationAttachments(text: String): ConversationAttachmentContent {
    val lines = text.lines()
    val body = mutableListOf<String>()
    val paths = mutableListOf<String>()
    var index = 0
    while (index < lines.size) {
        if (lines[index].trim() != "Attached images:") {
            body += lines[index]
            index += 1
            continue
        }
        var cursor = index + 1
        val found = mutableListOf<String>()
        while (cursor < lines.size) {
            val line = lines[cursor].trim()
            if (!line.startsWith("- ") || line.length <= 2) break
            found += line.removePrefix("- ").trim()
            cursor += 1
        }
        if (found.isEmpty()) {
            body += lines[index]
            index += 1
        } else {
            paths += found
            index = cursor
        }
    }
    return ConversationAttachmentContent(body.joinToString("\n").trim(), paths)
}

internal fun conversationActivityLabel(role: String): String = when (role.lowercase()) {
    "exec", "exec_command", "bash", "shell" -> "Command"
    "apply_patch", "edit", "write" -> "File edit"
    "image" -> "Image generation"
    "tool_output" -> "Output"
    "agent_message" -> "Agent message"
    else -> role.replace('_', ' ').trim().replaceFirstChar(Char::uppercase).ifBlank { "Tool" }
}

internal fun conversationActivitySummary(text: String): String {
    val compact = text.trim().replace(Regex("\\s+"), " ")
    if (compact.isEmpty()) return "No details"
    return if (compact.length <= 110) compact else compact.take(109).trimEnd() + "…"
}

@Composable
private fun ConnectionLabel(connection: ConnectionState, path: com.adroited.aiterm.remote.RemotePath?) {
    val (label, color) = when (connection) {
        ConnectionState.Connected -> when (path) {
            com.adroited.aiterm.remote.RemotePath.DIRECT -> "connected · direct"
            com.adroited.aiterm.remote.RemotePath.RELAY -> "connected · relay"
            com.adroited.aiterm.remote.RemotePath.LAN -> "connected · LAN"
            com.adroited.aiterm.remote.RemotePath.VPN -> "connected · VPN"
            com.adroited.aiterm.remote.RemotePath.IROH -> "connected · Iroh"
            else -> "connected"
        } to MaterialTheme.colorScheme.tertiary
        ConnectionState.Connecting -> "connecting" to MaterialTheme.colorScheme.primary
        ConnectionState.Reconnecting -> "reconnecting" to MaterialTheme.colorScheme.primary
        ConnectionState.Locked -> "locked" to MaterialTheme.colorScheme.error
        ConnectionState.Revoked -> "revoked" to MaterialTheme.colorScheme.error
        ConnectionState.Disconnected -> "offline" to MaterialTheme.colorScheme.onSurfaceVariant
    }
    Text(label, style = MaterialTheme.typography.labelMedium, color = color)
}

internal fun shouldFollowConversationUpdate(previousCount: Int, lastVisible: Int, scrolling: Boolean): Boolean =
    !scrolling && lastVisible >= previousCount - 2

// LazyColumn always starts with the crew strip, before timeline and status.
internal fun conversationListItemCount(timelineCount: Int, working: Boolean): Int =
    1 + timelineCount + if (working) 1 else 0

internal fun isConversationSessionLive(session: RemoteSession, tabs: List<RemoteTab>): Boolean =
    tabs.any { it.sessionId == session.id }

/** Shell tabs are useful in the raw terminal, but are not conversation rows. */
internal fun liveConversationCount(sessions: List<RemoteSession>, tabs: List<RemoteTab>): Int =
    sessions.count { isConversationSessionLive(it, tabs) }

/**
 * A native spine reads the agent's own turn boundaries and therefore owns
 * the verdict. The roster is a coarse terminal-cadence fallback for older
 * desktops; allowing it to override a native Idle makes a completed turn
 * keep spinning whenever the two refreshes race.
 */
internal fun isConversationWorking(
    phase: SpinePhase,
    spineLive: Boolean,
    turnOpen: Boolean?,
    rosterActivity: String?,
): Boolean {
    // A completed native turn is stronger evidence than the last phase
    // packet. It also makes the UI robust if an idle phase is delayed or
    // lost while the conversation is paging through a large history.
    if (spineLive && turnOpen == false) return false
    return when (phase) {
        SpinePhase.Working -> true
        SpinePhase.NeedsYou -> false
        SpinePhase.Idle -> !spineLive && rosterActivity == "output"
    }
}

internal fun conversationSessions(
    sessions: List<RemoteSession>,
    tabs: List<RemoteTab>,
    query: String,
    starred: Set<String> = emptySet(),
    withFiles: Set<String> = emptySet(),
    broughtIn: Map<String, String> = emptyMap(),
    agentFilter: String? = null,
    filesOnly: Boolean = false,
    activeOnly: Boolean = false,
    foldedCrews: Set<String> = emptySet(),
): List<RemoteSession> {
    val needle = query.trim().lowercase()
    val sorted = sessions.asSequence()
        .filter { session ->
            needle.isEmpty() || listOf(session.title, session.agent, session.projectPath, session.groupPath)
                .any { needle in it.lowercase() }
        }
        .filter { agentFilter == null || it.agent == agentFilter }
        .filter { !filesOnly || it.id in withFiles }
        .filter { !activeOnly || isConversationSessionLive(it, tabs) }
        .sortedWith(
            compareByDescending<RemoteSession> { it.id in starred }
                .thenByDescending { isConversationSessionLive(it, tabs) }
                .thenByDescending { it.lastActive },
        )
        .toList()
    if (broughtIn.isEmpty()) return sorted
    val visibleIds = sorted.mapTo(hashSetOf()) { it.id }
    val result = ArrayList<RemoteSession>(sorted.size)
    val placed = hashSetOf<String>()
    for (session in sorted) {
        if (session.id in placed) continue
        if (broughtIn[session.id] in visibleIds) continue
        result += session
        placed += session.id
        if (session.id !in foldedCrews) {
            sorted.filterTo(result) { child ->
                broughtIn[child.id] == session.id && placed.add(child.id)
            }
        } else {
            sorted.filter { child -> broughtIn[child.id] == session.id }.forEach { placed += it.id }
        }
    }
    return result
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
