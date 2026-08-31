package com.fivelime.aiterm.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.SubdirectoryArrowRight
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.rememberDrawerState
import androidx.compose.material.icons.filled.LinkOff
import androidx.compose.material.icons.filled.Menu
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.launch
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.TextFieldDefaults
import com.fivelime.aiterm.SessionState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.animation.core.animateFloat
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.clickable
import com.fivelime.aiterm.AppViewModel
import com.fivelime.aiterm.Session

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionsScreen(vm: AppViewModel, outer: PaddingValues) {
    var renaming by remember { mutableStateOf<Session?>(null) }
    val drawer = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    val visible = vm.visibleSessions

    renaming?.let { s ->
        RenameDialog(current = s.title, onDone = { vm.rename(s, it); renaming = null }, onDismiss = { renaming = null })
    }
    // Opening the drawer is also the moment to freshen what it shows.
    LaunchedEffect(drawer.isOpen) { if (drawer.isOpen) { vm.loadUsage(); vm.checkDesktops() } }
    ModalNavigationDrawer(
        drawerState = drawer,
        drawerContent = { AppDrawer(vm, close = { scope.launch { drawer.close() } }) },
    ) {
    Scaffold(
        modifier = Modifier.padding(outer).dismissKeyboardOnTap(),
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(onClick = { scope.launch { drawer.open() } }) { Icon(Icons.Filled.Menu, "Menu") }
                },
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Dot(if (vm.connected) Green else Muted)
                        Spacer(Modifier.width(10.dp))
                        Text(vm.desktop?.name ?: "Desktop")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Bg),
            )
        },
        floatingActionButton = {
            FloatingActionButton(onClick = { vm.composingNew = true }) { Icon(Icons.Filled.Add, "New session") }
        },
        containerColor = Bg,
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            Dashboard(vm)
            if (vm.sessions.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text(if (vm.connected) "No sessions yet" else "Connecting…", color = Muted)
                }
            } else if (visible.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text("Nothing matches these filters", color = Muted)
                }
            } else {
                LazyColumn(Modifier.fillMaxSize()) {
                    items(visible, key = { it.id }) { s ->
                        SessionRow(
                            s, vm.stateOf(s), showFolder = true,
                            starred = s.id in vm.stars,
                            satellite = vm.broughtIn[s.id] != null && visible.any { it.id == vm.broughtIn[s.id] },
                            crew = vm.broughtIn.count { it.value == s.id },
                            folded = s.id in vm.foldedCrews,
                            onCrewTap = { vm.toggleCrew(s.id) },
                            crewNeedsYou = vm.broughtIn.any { it.value == s.id && vm.activity[it.key] == "attention" },
                            onLongClick = { renaming = s },
                        ) { vm.select(s) }
                    }
                }
            }
        }
    }
    }
}

/** Search plus the filter chips. Usage lives in the drawer now — this
 *  strip is for finding sessions. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Dashboard(vm: AppViewModel) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp)) {
        OutlinedTextField(
            value = vm.query, onValueChange = { vm.search(it) },
            placeholder = { Text("Search sessions…", color = Muted) },
            leadingIcon = { Icon(Icons.Filled.Search, null, tint = Muted) },
            trailingIcon = { if (vm.query.isNotEmpty() && vm.results == null) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) },
            singleLine = true, shape = RoundedCornerShape(12.dp),
            colors = TextFieldDefaults.colors(focusedContainerColor = Surface1, unfocusedContainerColor = Surface1,
                focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(8.dp))
        // One tap on, one tap off: an engine, sessions that made files,
        // sessions alive right now. They combine.
        val agents = remember(vm.sessions) { vm.sessions.map { it.agent }.distinct().sorted() }
        // The engine chips arrive after the first frame (sessions load
        // async) and a keyed LazyRow anchors to what it was already
        // showing — leaving the row scrolled past the new first chip.
        // Snap back to the start whenever the set changes.
        val chipRow = rememberLazyListState()
        LaunchedEffect(agents) { chipRow.scrollToItem(0) }
        LazyRow(state = chipRow, horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(6.dp)) {
            items(agents, key = { it }) { a ->
                FilterChip(
                    selected = vm.agentFilter == a,
                    onClick = { vm.agentFilter = if (vm.agentFilter == a) null else a },
                    label = { Text(a.replaceFirstChar { it.uppercase() }) },
                    leadingIcon = { AgentIcon(a, 16.dp) },
                    colors = filterColors(),
                )
            }
            item(key = "files") {
                FilterChip(
                    selected = vm.filesOnly, onClick = { vm.filesOnly = !vm.filesOnly },
                    label = { Text("Has files") }, colors = filterColors(),
                )
            }
            item(key = "active") {
                FilterChip(
                    selected = vm.activeOnly, onClick = { vm.activeOnly = !vm.activeOnly },
                    label = { Text("Active") }, colors = filterColors(),
                )
            }
        }
    }
}

@Composable
private fun filterColors() = FilterChipDefaults.filterChipColors(
    selectedContainerColor = Accent.copy(alpha = 0.2f),
    selectedLabelColor = Accent,
)

/** The app's one menu: who we're connected to, every usage source in
 *  full — bars with resets, balances, the error line when a source is
 *  failing — then the few actions. Usage lives here, not on the home
 *  page: the list is for sessions. */
@Composable
private fun AppDrawer(vm: AppViewModel, close: () -> Unit) {
    ModalDrawerSheet(drawerContainerColor = Bg) {
        Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 16.dp)) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth().padding(start = 20.dp, top = 24.dp, end = 8.dp, bottom = 4.dp),
            ) {
                Dot(if (vm.connected) Green else Muted)
                Spacer(Modifier.width(10.dp))
                Column {
                    Text(vm.desktop?.name ?: "Desktop", style = MaterialTheme.typography.titleLarge)
                    Text(if (vm.connected) "connected" else "connecting…", style = MaterialTheme.typography.labelSmall, color = Muted)
                }
                Spacer(Modifier.weight(1f))
                // Tapping the dimmed strip also closes, but nothing says so;
                // an X does.
                IconButton(onClick = close) { Icon(Icons.Filled.Close, "Close menu") }
            }
            // Every paired desktop, when there is more than one: tap to
            // switch. Every dot is a status — the shown one live, the rest
            // from the probe the drawer's opening fired.
            if (vm.desktops.size > 1) {
                HorizontalDivider(Modifier.padding(vertical = 12.dp), color = Surface1)
                Text("DESKTOPS", style = MaterialTheme.typography.labelSmall, color = Muted, modifier = Modifier.padding(horizontal = 20.dp))
                vm.desktops.forEach { d ->
                    val active = d.fingerprint == vm.desktop?.fingerprint
                    NavigationDrawerItem(
                        label = { Text(d.name, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal) },
                        icon = {
                            Dot(
                                if (active) { if (vm.connected) Green else Muted }
                                else when (vm.reachable[d.fingerprint]) {
                                    true -> Green
                                    false -> Surface1
                                    null -> Muted // probing, no answer yet
                                },
                            )
                        },
                        selected = active,
                        onClick = { if (!active) { vm.switchTo(d); close() } },
                        modifier = Modifier.padding(horizontal = 12.dp),
                    )
                }
            }
            HorizontalDivider(Modifier.padding(vertical = 12.dp), color = Surface1)
            Text("USAGE", style = MaterialTheme.typography.labelSmall, color = Muted, modifier = Modifier.padding(horizontal = 20.dp))
            if (vm.usage.isEmpty()) {
                Text("Nothing reported yet", color = Muted, style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
            }
            vm.usage.forEach { u ->
                Column(Modifier.padding(horizontal = 20.dp, vertical = 5.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        AgentIcon(u.id.removePrefix("provider:"), 16.dp)
                        Spacer(Modifier.width(8.dp))
                        Text(u.name, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.bodyMedium)
                        if (u.plan.isNotBlank()) {
                            Spacer(Modifier.width(6.dp))
                            Text(u.plan, style = MaterialTheme.typography.labelSmall, color = Muted)
                        }
                    }
                    if (u.state != "ok") {
                        Text(u.detail.ifBlank { u.state }, style = MaterialTheme.typography.labelSmall, color = Amber)
                    }
                    // One line per limit: what it is, how full, when it lets go.
                    u.bars.forEach { b ->
                        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 3.dp)) {
                            Text(
                                b.label, style = MaterialTheme.typography.labelSmall, color = Muted,
                                modifier = Modifier.width(84.dp), maxLines = 1, overflow = TextOverflow.Ellipsis,
                            )
                            LinearProgressIndicator(
                                progress = { (b.percent / 100.0).toFloat().coerceIn(0f, 1f) },
                                modifier = Modifier.weight(1f),
                                color = when { b.percent < 50 -> Green; b.percent < 80 -> Amber; else -> Red },
                                trackColor = Surface1,
                            )
                            Spacer(Modifier.width(8.dp))
                            Text(
                                "${b.percent.toInt()}%", style = MaterialTheme.typography.labelSmall,
                                fontWeight = FontWeight.SemiBold,
                                modifier = Modifier.width(34.dp), textAlign = TextAlign.End,
                            )
                            Text(
                                resetsIn(b.resets_at), style = MaterialTheme.typography.labelSmall, color = Muted,
                                modifier = Modifier.width(56.dp), textAlign = TextAlign.End, maxLines = 1,
                            )
                        }
                    }
                    u.amounts.forEach { am ->
                        Row(modifier = Modifier.padding(top = 3.dp)) {
                            Text(am.label, style = MaterialTheme.typography.labelSmall, color = Muted, modifier = Modifier.width(84.dp))
                            Text(
                                (if (am.currency == "USD") "$" else "") + "%.2f".format(am.amount) + (am.of?.let { " of %.0f".format(it) } ?: ""),
                                style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold,
                            )
                        }
                    }
                }
            }
            Text(
                "limit · bar · used · resets in",
                style = MaterialTheme.typography.labelSmall, color = Muted.copy(alpha = 0.7f),
                modifier = Modifier.padding(start = 20.dp, top = 2.dp),
            )
            HorizontalDivider(Modifier.padding(vertical = 12.dp), color = Surface1)
            NavigationDrawerItem(
                label = { Text("Refresh") },
                icon = { Icon(Icons.Filled.Refresh, null) },
                selected = false,
                onClick = { vm.refreshNow(); vm.loadUsage(); close() },
                modifier = Modifier.padding(horizontal = 12.dp),
            )
            NavigationDrawerItem(
                label = { Text("Settings") },
                icon = { Icon(Icons.Filled.Settings, null) },
                selected = false,
                onClick = { close(); vm.showSettings = true },
                modifier = Modifier.padding(horizontal = 12.dp),
            )
            NavigationDrawerItem(
                label = { Text("Add a desktop") },
                icon = { Icon(Icons.Filled.Add, null) },
                selected = false,
                onClick = { close(); vm.showPair = true },
                modifier = Modifier.padding(horizontal = 12.dp),
            )
            NavigationDrawerItem(
                label = { Text("Forget this desktop") },
                icon = { Icon(Icons.Filled.LinkOff, null) },
                selected = false,
                onClick = { close(); vm.forget() },
                modifier = Modifier.padding(horizontal = 12.dp),
            )
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun SessionRow(
    s: Session, state: SessionState, showFolder: Boolean, starred: Boolean = false,
    satellite: Boolean = false, crew: Int = 0, folded: Boolean = false, onCrewTap: () -> Unit = {},
    crewNeedsYou: Boolean = false,
    onLongClick: () -> Unit = {}, onClick: () -> Unit,
) {
    ListItem(
        modifier = Modifier.combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(start = if (satellite) 26.dp else 0.dp),
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        leadingContent = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (satellite) {
                    Icon(Icons.Filled.SubdirectoryArrowRight, "Brought in", tint = Muted, modifier = Modifier.size(16.dp))
                    Spacer(Modifier.width(4.dp))
                }
                AgentIcon(s.agent, if (satellite) 24.dp else 30.dp)
            }
        },
        headlineContent = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (starred) {
                    Icon(Icons.Filled.Star, "Starred", tint = Amber, modifier = Modifier.size(15.dp))
                    Spacer(Modifier.width(5.dp))
                }
                Text(s.title.ifBlank { "Untitled" }, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
            }
        },
        supportingContent = {
            Text(
                buildString {
                    append(relativeTime(s.last_active))
                    if (showFolder) { append(" · "); append(folderName(s.group_path)) }
                    s.branch?.let { append(" · "); append(it) }
                    if (s.forked) append(" · fork")
                },
                color = Muted, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
        },
        trailingContent = {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(6.dp)) {
                // The state indicator is always the rightmost thing, so the
                // dots right-align down the list; the crew fold sits beside
                // it — a count and a caret, padded into a real touch target.
                if (crew > 0) {
                    Row(
                        Modifier.clip(RoundedCornerShape(10.dp))
                            .background(Accent.copy(alpha = if (folded) 0.08f else 0.15f))
                            .clickable(onClick = onCrewTap)
                            .padding(start = 10.dp, end = 6.dp, top = 8.dp, bottom = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text("+$crew", style = MaterialTheme.typography.labelMedium, color = Accent)
                        Icon(
                            if (folded) Icons.Filled.KeyboardArrowRight else Icons.Filled.KeyboardArrowDown,
                            if (folded) "Show crew" else "Hide crew",
                            tint = Accent, modifier = Modifier.size(18.dp),
                        )
                    }
                }
                // A brought-in agent of THIS session is parked on a prompt —
                // its own row may be folded away, so the master's row says so.
                if (crewNeedsYou) StateChip("crew needs you", stateColor(SessionState.NeedsYou), pulse = true)
                // Open on the desktop is ambient, not news — a quiet dot, no words.
                if (state == SessionState.OnDesktop) Dot(stateColor(state))
                else stateLabel(state)?.let { StateChip(it, stateColor(state), pulse = state == SessionState.Working) }
            }
        },
    )
}

@Composable
fun Dot(color: Color) = Box(Modifier.size(8.dp).background(color, CircleShape))

@Composable
fun StateChip(label: String, color: Color, pulse: Boolean = false) {
    // Label first, dot last: the dot is the indicator, and it sits at the
    // right edge so every row's indicator lines up in one column.
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, style = MaterialTheme.typography.labelSmall, color = color)
        Spacer(Modifier.width(6.dp))
        if (pulse) PulsingDot(color) else Dot(color)
    }
}

@Composable
fun PulsingDot(color: Color) {
    val t = androidx.compose.animation.core.rememberInfiniteTransition(label = "pulse")
    val a by t.animateFloat(
        initialValue = 0.3f, targetValue = 1f,
        animationSpec = androidx.compose.animation.core.infiniteRepeatable(
            androidx.compose.animation.core.tween(700), androidx.compose.animation.core.RepeatMode.Reverse),
        label = "alpha",
    )
    Box(Modifier.size(8.dp).background(color.copy(alpha = a), CircleShape))
}

/** "1h 56m" / "6d" — the reset, sized for the end of a row. */
private fun resetsIn(iso: String): String = runCatching {
    val t = java.time.OffsetDateTime.parse(iso).toInstant()
    val mins = java.time.Duration.between(java.time.Instant.now(), t).toMinutes()
    when {
        mins <= 0 -> "soon"
        mins < 60 -> "${mins}m"
        mins < 10 * 60 -> "${mins / 60}h ${mins % 60}m"
        mins < 48 * 60 -> "${mins / 60}h"
        else -> "${mins / (60 * 24)}d"
    }
}.getOrDefault("")

/** One rename dialog for everywhere a session can be renamed. Clearing the
 *  field and saving restores the engine's own name. */
@Composable
internal fun RenameDialog(current: String, onDone: (String) -> Unit, onDismiss: () -> Unit) {
    var draft by remember { mutableStateOf(current) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Rename session") },
        text = {
            OutlinedTextField(
                value = draft, onValueChange = { draft = it },
                singleLine = true,
                placeholder = { Text("Leave empty to restore the original name", color = Muted) },
            )
        },
        confirmButton = { TextButton(onClick = { onDone(draft) }) { Text("Rename") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
