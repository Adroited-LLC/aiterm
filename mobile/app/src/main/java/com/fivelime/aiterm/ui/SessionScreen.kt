package com.fivelime.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import coil3.compose.AsyncImage
import com.fivelime.aiterm.FileEntry
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Build
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.Language
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.foundation.layout.size
import com.fivelime.aiterm.SessionState
import androidx.compose.material3.Badge
import androidx.compose.material3.Button
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.animation.core.animateFloat
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.fivelime.aiterm.AppViewModel
import com.fivelime.aiterm.Session
import com.fivelime.aiterm.ModelOption
import com.fivelime.aiterm.Turn

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionScreen(vm: AppViewModel, s: Session, outer: PaddingValues) {
    val open = s.id in vm.open
    val running = s.id in vm.running
    val state = vm.stateOf(s)
    val working = state == SessionState.Working
    var draft by remember(s.id) { mutableStateOf("") }
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var bringingIn by remember { mutableStateOf(false) }
    if (bringingIn) {
        BringInDialog(vm, s, onDismiss = { bringingIn = false })
    }
    if (renaming) {
        RenameDialog(current = s.title, onDone = { vm.rename(s, it); renaming = false }, onDismiss = { renaming = false })
    }
    val list = rememberLazyListState()

    // New content lands at the bottom, where the eye is. The working row is
    // one extra item past the last turn. The FIRST fill jumps straight to
    // the end — animating from the top replays the whole transcript and
    // looks glitchy on a long session. After that, follow new turns with a
    // short animation, but only when already reading the end: someone
    // scrolled up into history stays where they are.
    var positioned by remember(s.id) { mutableStateOf(false) }
    LaunchedEffect(vm.turns.size, working) {
        val n = vm.turns.size + (if (working) 1 else 0)
        if (n == 0) return@LaunchedEffect
        if (!positioned) {
            list.scrollToItem(n - 1)
            positioned = true
        } else {
            val lastVisible = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            if (lastVisible >= n - 3) list.animateScrollToItem(n - 1)
        }
    }

    Scaffold(
        modifier = Modifier.padding(outer).imePadding().dismissKeyboardOnTap(),
        containerColor = Bg,
        topBar = {
            TopAppBar(
                navigationIcon = { IconButton(onClick = { vm.select(null) }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        AgentIcon(s.agent, 26.dp)
                        Spacer(Modifier.width(10.dp))
                        Column {
                            Text(s.title.ifBlank { "Untitled" }, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMedium)
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(folderName(s.group_path), style = MaterialTheme.typography.labelSmall, color = Muted)
                                Spacer(Modifier.width(10.dp))
                                StateChip(stateLabel(state) ?: "idle", stateColor(state), pulse = working)
                            }
                        }
                    }
                },
                actions = {
                    // Count what the agent produced, not every file the
                    // folder walk caught — "Files 87" full of build noise
                    // says nothing.
                    val produced = vm.files.count { it.via == "made" || it.via == "edited" || it.via == "wrote" }
                    IconButton(onClick = { vm.showFiles = !vm.showFiles }) {
                        BadgedBox(badge = { if (produced > 0) Badge { Text("$produced") } }) {
                            Icon(Icons.Filled.Folder, "Files", tint = if (vm.showFiles) Accent else Muted)
                        }
                    }
                    // A page to look at: a dev server the session started,
                    // or — servers get killed, sandboxed, orphaned — the
                    // index.html it built, served as static files.
                    val servedPort = vm.ports[s.id]?.firstOrNull()
                    val builtPage = vm.files
                        .filter { (it.ext == "html" || it.ext == "htm") && (it.via == "made" || it.via == "edited" || it.via == "wrote") }
                        .minByOrNull { if (it.name == "index.html") 0 else 1 }
                    if (servedPort != null || builtPage != null) {
                        IconButton(onClick = {
                            if (servedPort != null) vm.previewPort(servedPort) else vm.previewFile(builtPage!!.path)
                        }) { Icon(Icons.Filled.Language, "Preview", tint = Accent) }
                    }
                    if (!open) IconButton(onClick = { vm.openOnDesktop(s) }) {
                        Icon(Icons.Filled.PlayArrow, "Open on desktop", tint = Green)
                    }
                    IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, "More") }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(
                            text = { Text("Bring in a second agent") },
                            onClick = { menu = false; bringingIn = true },
                        )
                        DropdownMenuItem(
                            text = { Text(if (s.id in vm.stars) "Unstar" else "Star — keep on top") },
                            onClick = { menu = false; vm.toggleStar(s) },
                        )
                        DropdownMenuItem(text = { Text("Rename") }, onClick = { menu = false; renaming = true })
                        if (open) DropdownMenuItem(text = { Text("Interrupt (Esc)") }, onClick = { menu = false; vm.interrupt(s) })
                        if (running || open) DropdownMenuItem(text = { Text("Stop session") }, onClick = { menu = false; vm.stop(s) })
                        DropdownMenuItem(text = { Text("Refresh") }, onClick = { menu = false; vm.select(s) })
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Bg),
            )
        },
        bottomBar = {
            Column(Modifier.fillMaxWidth().navigationBarsPadding()) {
            vm.relays[s.id]?.let { r -> RelayBanner(vm, s, r) }
            // A brought-in agent parked on a prompt is invisible from here —
            // its dialog lives in its own terminal. Say so on the master's
            // screen, where the person actually is, and take them there.
            vm.crewNeedsYou(s).forEach { c -> CrewNeedsYouBanner(c) { vm.select(c) } }
            // This session itself is waiting on a person: the dialog is a TUI
            // the conversation view cannot render, so offer the keys that
            // answer one. Raw, no Enter appended — Enter is one of the keys.
            if (state == SessionState.NeedsYou) QuickKeysBar { k -> vm.sendKeys(s, k) }
            AttachmentChips(vm)
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 8.dp),
                verticalAlignment = Alignment.Bottom,
            ) {
                AttachButton(vm)
                OutlinedTextField(
                    value = draft, onValueChange = { draft = it },
                    modifier = Modifier.weight(1f),
                    placeholder = { Text(if (open) "Message ${s.agent}…" else "Message ${s.agent} — opens it on the desktop first") },
                    maxLines = 6,
                    shape = RoundedCornerShape(20.dp),
                )
                Spacer(Modifier.width(6.dp))
                when {
                    vm.sending -> Box(Modifier.padding(12.dp)) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
                    // While the agent works, the button is Stop: one tap ends the
                    // turn (Escape) and keeps the session. Send comes back after.
                    working && draft.isBlank() -> FilledIconButton(
                        onClick = { vm.interrupt(s) },
                        colors = IconButtonDefaults.filledIconButtonColors(containerColor = Amber.copy(alpha = 0.2f), contentColor = Amber),
                    ) { Icon(Icons.Filled.Stop, "Stop the current turn") }
                    else -> IconButton(
                        onClick = { val t = draft.trim(); if (t.isNotEmpty() || vm.attachments.isNotEmpty()) { vm.send(t); draft = "" } },
                        enabled = draft.isNotBlank() || vm.attachments.isNotEmpty(),
                    ) { Icon(Icons.AutoMirrored.Filled.Send, "Send", tint = if (draft.isBlank()) Muted else Accent) }
                }
            }
            }
        },
    ) { padding ->
        if (vm.showFiles) {
            FilesList(vm, Modifier.padding(padding))
        } else if (vm.turns.isEmpty()) {
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
                if (vm.loadingTurns) CircularProgressIndicator() else Text("Nothing here yet — say something.", color = Muted)
            }
        } else {
            LazyColumn(
                state = list,
                modifier = Modifier.fillMaxSize().padding(padding),
                contentPadding = PaddingValues(horizontal = 12.dp, vertical = 8.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                itemsIndexed(vm.turns) { _, t -> TurnView(t, onOpenPath = vm::openMentioned) }
                // What the session made, right where the conversation ends —
                // the transcript often never prints a path (tool output is
                // dropped at phone size), but the desktop's ledger knows.
                val made = vm.files.filter { it.via == "made" || it.via == "edited" || it.via == "wrote" }
                if (made.isNotEmpty()) item(key = "made") { MadeStrip(vm, made) }
                if (working) item(key = "working") { WorkingRow(s.agent) }
            }
        }
    }
}

/** The gap between "start" tapped and the session existing on disk, made to
 *  look like the conversation it is about to become: the ask echoed as a
 *  sent bubble, the engine "working" underneath. The real session replaces
 *  this screen the moment discovery finds it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun StartingScreen(vm: AppViewModel, s: AppViewModel.Starting, outer: PaddingValues) {
    Scaffold(
        modifier = Modifier.padding(outer),
        containerColor = Bg,
        topBar = {
            TopAppBar(
                navigationIcon = { IconButton(onClick = { vm.cancelStarting() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        AgentIcon(s.agentId.removePrefix("api:"), 26.dp)
                        Spacer(Modifier.width(10.dp))
                        Column {
                            Text("Starting ${s.agentName}", style = MaterialTheme.typography.titleMedium)
                            Text(folderName(s.cwd), style = MaterialTheme.typography.labelSmall, color = Muted)
                        }
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Bg),
            )
        },
    ) { padding ->
        Column(
            Modifier.fillMaxSize().padding(padding).padding(horizontal = 12.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (!s.prompt.isNullOrBlank()) {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
                    Box(
                        Modifier.widthIn(max = 320.dp)
                            .background(MaterialTheme.colorScheme.primaryContainer, RoundedCornerShape(18.dp, 18.dp, 4.dp, 18.dp))
                            .padding(12.dp),
                    ) { Text(s.prompt, color = MaterialTheme.colorScheme.onPrimaryContainer) }
                }
            }
            WorkingRow(s.agentName)
        }
    }
}

/** A brought-in agent is waiting on a person. Its approval dialog lives in
 *  its own terminal, so the master's screen names it and a tap goes there. */
@Composable
private fun CrewNeedsYouBanner(c: com.fivelime.aiterm.Session, onOpen: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onOpen)
            .background(Amber.copy(alpha = 0.12f))
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AgentIcon(c.agent, 20.dp)
        Spacer(Modifier.width(8.dp))
        Text(
            "${c.agent} needs you — tap to answer",
            style = MaterialTheme.typography.labelMedium, color = Amber,
            modifier = Modifier.weight(1f),
        )
    }
}

/** The keys that answer a terminal dialog, for a session in "needs you":
 *  approve/deny shortcuts, digits for numbered pickers, arrows and Enter for
 *  selection lists, Esc to back out. Sent raw — the dialog reads keystrokes,
 *  not messages. */
@Composable
private fun QuickKeysBar(onKey: (String) -> Unit) {
    val keys = listOf(
        "Enter" to "\r", "y" to "y", "n" to "n",
        "1" to "1", "2" to "2", "3" to "3",
        "↑" to "\u001B[A", "↓" to "\u001B[B", "Esc" to "\u001B",
    )
    Row(
        Modifier.fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 8.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("Answer:", style = MaterialTheme.typography.labelSmall, color = Muted)
        keys.forEach { (label, seq) ->
            Box(
                Modifier.background(Amber.copy(alpha = 0.15f), RoundedCornerShape(12.dp))
                    .clickable { onKey(seq) }
                    .padding(horizontal = 12.dp, vertical = 6.dp),
            ) { Text(label, style = MaterialTheme.typography.labelMedium, color = Amber) }
        }
    }
}

/** Three dots that breathe. Shown while the desktop reports progress, or
 *  right after a message goes out — before the first progress arrives. */
@Composable
private fun WorkingRow(agent: String) {
    Row(Modifier.padding(start = 4.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        val t = androidx.compose.animation.core.rememberInfiniteTransition(label = "dots")
        val phase by t.animateFloat(0f, 3f, androidx.compose.animation.core.infiniteRepeatable(
            androidx.compose.animation.core.tween(900, easing = androidx.compose.animation.core.LinearEasing)), label = "phase")
        repeat(3) { i ->
            val on = ((phase.toInt() % 3) == i)
            Box(Modifier.padding(horizontal = 2.dp).size(7.dp).background(Amber.copy(alpha = if (on) 1f else 0.3f), RoundedCornerShape(50)))
        }
        Spacer(Modifier.width(8.dp))
        Text("$agent is working…", style = MaterialTheme.typography.labelMedium, color = Muted)
    }
}

/** A user turn sits right in the accent colour, the assistant left on a
 *  card, and anything else — a tool call, a system line — is small, dim and
 *  monospace, folded until tapped. */
/** Codex folds its AGENTS.md and an environment block into the first user
 *  turn; the person only typed the last line. Hide the harness's part. */
private val HARNESS_BLOCKS = Regex("(?s)<(INSTRUCTIONS|environment_context|user_instructions)>.*?</\\1>\\s*")
private val HARNESS_HEADING = Regex("(?m)^#\\s*AGENTS\\.md instructions for \\S+\\s*$")
/** What the person typed, with the harness's preamble gone. Empty means
 *  the whole turn was scaffolding — codex sends AGENTS.md as its own
 *  message before the first real prompt — and deserves no bubble. */
private fun personSaid(text: String): String =
    text.replace(HARNESS_BLOCKS, "").replace(HARNESS_HEADING, "").trim()

/** File paths an agent says out loud — "Saved to: file:///…", "wrote
 *  /home/…/main.rs" — pulled from a turn so they can be tapped. Conservative:
 *  file:// URIs, and absolute paths bearing an extension. */
private val FILE_MENTION = Regex("""file:///\S+|(?:/[\w.@%+~-]+){2,}\.[A-Za-z0-9]{1,8}""")
private fun mentionedFiles(text: String): List<String> =
    FILE_MENTION.findAll(text).map { m ->
        val p = m.value.removePrefix("file://").trimEnd('.', ',', ':', ';', ')', ']', '"', '\'')
        if ('%' in p) runCatching { android.net.Uri.decode(p) }.getOrDefault(p) else p
    }.distinct().take(6).toList()

@Composable
private fun TurnView(t: Turn, onOpenPath: (String) -> Unit = {}) {
    // Long-press selects within a turn; taps (chips, tool-card folds,
    // links) still work as taps.
    androidx.compose.foundation.text.selection.SelectionContainer {
        TurnBody(t, onOpenPath)
    }
}

@Composable
private fun TurnBody(t: Turn, onOpenPath: (String) -> Unit) {
    when (t.role) {
        "user" -> {
            val said = personSaid(t.text)
            if (said.isNotEmpty()) Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
                Box(
                    Modifier.widthIn(max = 320.dp).background(MaterialTheme.colorScheme.primaryContainer, RoundedCornerShape(18.dp, 18.dp, 4.dp, 18.dp)).padding(12.dp),
                ) { MarkdownText(said, color = MaterialTheme.colorScheme.onPrimaryContainer) }
            }
        }
        "assistant" -> Column(Modifier.fillMaxWidth().padding(end = 12.dp)) {
            MarkdownText(t.text)
            val paths = remember(t.text) { mentionedFiles(t.text) }
            if (paths.isNotEmpty()) Row(
                Modifier.padding(top = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                paths.forEach { p -> FileChip(p.substringAfterLast('/'), onClick = { onOpenPath(p) }) }
            }
        }
        "system" -> Text(t.text, style = MaterialTheme.typography.labelSmall, color = Muted,
            modifier = Modifier.fillMaxWidth().padding(vertical = 2.dp), maxLines = 2, overflow = TextOverflow.Ellipsis)
        // The agent's reasoning summary: quiet, italic, folded to a line.
        "thinking" -> {
            var expanded by remember { mutableStateOf(false) }
            Text(
                t.text, style = MaterialTheme.typography.bodySmall, color = Muted,
                fontStyle = androidx.compose.ui.text.font.FontStyle.Italic,
                maxLines = if (expanded) Int.MAX_VALUE else 2, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.fillMaxWidth().clickable { expanded = !expanded }.padding(horizontal = 4.dp, vertical = 2.dp),
            )
        }
        else -> {
            // A tool call: its name and the first line, the rest on tap.
            var expanded by remember { mutableStateOf(false) }
            val first = t.text.lineSequence().firstOrNull { it.isNotBlank() }?.trim() ?: ""
            Column(
                Modifier.fillMaxWidth().background(Surface1.copy(alpha = 0.6f), RoundedCornerShape(10.dp))
                    .clickable { expanded = !expanded }.padding(horizontal = 10.dp, vertical = 8.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.Filled.Build, null, tint = Accent, modifier = Modifier.size(14.dp))
                    Spacer(Modifier.width(6.dp))
                    Text(t.role, style = MaterialTheme.typography.labelMedium, color = Accent)
                    Spacer(Modifier.width(8.dp))
                    if (!expanded) Text(first, style = MaterialTheme.typography.labelSmall, fontFamily = FontFamily.Monospace,
                        color = Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                if (expanded) Text(
                    t.text, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, color = Muted,
                    modifier = Modifier.padding(top = 6.dp),
                )
            }
        }
    }
}

/** A small tappable pill for a file the conversation mentions. */
@Composable
private fun FileChip(name: String, onClick: () -> Unit) {
    Row(
        Modifier.clip(RoundedCornerShape(10.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val kind = FileEntry(name, name, 0, 0, "").kind
        Icon(iconFor(kind), null, Modifier.size(16.dp), tint = if (kind == "image" || kind == "video") Accent else Muted)
        Spacer(Modifier.width(6.dp))
        Text(name, style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** What the agent produced, at the end of the conversation where the eye
 *  lands. Images show themselves; anything else is a pill. Tap for the full
 *  view. The transcript rarely prints a usable path — the desktop's change
 *  ledger is what knows, and it rides in on /v1/sessions/{id}/files. */
@Composable
private fun MadeStrip(vm: AppViewModel, made: List<FileEntry>) {
    Column(Modifier.fillMaxWidth().padding(top = 4.dp)) {
        Text("Files from this session", style = MaterialTheme.typography.labelSmall, color = Muted)
        Spacer(Modifier.height(6.dp))
        LazyRow(
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            items(made.take(12), key = { it.path }) { f ->
                if (f.kind == "image") ImageCard(vm, f) else FileChip(f.name, onClick = { vm.open(f) })
            }
        }
    }
}

/** An image the agent made, visible right in the chat. */
@Composable
private fun ImageCard(vm: AppViewModel, f: FileEntry) {
    LaunchedEffect(f.path) { vm.fetchInline(f) }
    val local = vm.inlineFiles[f.path]
    Box(
        Modifier.size(width = 168.dp, height = 126.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable { vm.open(f) },
        contentAlignment = Alignment.Center,
    ) {
        if (local != null) AsyncImage(model = local, contentDescription = f.name, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        else CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
    }
}

/** Who joins, what they look at, how long they talk — a bottom sheet,
 *  full width, nothing crushed. The desktop runs the relay; the whole
 *  exchange appears in this conversation, live. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun BringInDialog(vm: AppViewModel, s: Session, onDismiss: () -> Unit) {
    var agentId by remember {
        mutableStateOf(vm.agents.firstOrNull { it.id != s.agent }?.id ?: vm.agents.firstOrNull()?.id ?: "claude")
    }
    var focus by remember { mutableStateOf("") }
    var rounds by remember { mutableStateOf(2) }
    var auto by remember { mutableStateOf(false) }
    // API choices (OpenRouter et al) need a model; CLIs default to their own.
    val sel = vm.agents.firstOrNull { it.id == agentId }
    var model by remember(agentId) {
        mutableStateOf(if (agentId.startsWith("api:")) vm.agents.firstOrNull { it.id == agentId }?.models?.firstOrNull()?.id else null)
    }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = Surface1,
    ) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 28.dp)) {
            Text("Bring in a second agent", style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(4.dp))
            Text(
                "They read this chat and talk it out with ${s.agent.replaceFirstChar { it.uppercase() }} right here. No files change; you decide after.",
                style = MaterialTheme.typography.bodySmall, color = Muted,
            )
            Spacer(Modifier.height(16.dp))
            vm.agents.forEach { a ->
                Row(
                    Modifier.fillMaxWidth()
                        .clip(RoundedCornerShape(12.dp))
                        .background(if (agentId == a.id) MaterialTheme.colorScheme.primaryContainer.copy(alpha = 0.4f) else Color.Transparent)
                        .clickable { agentId = a.id }
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    AgentIcon(a.id, 24.dp)
                    Spacer(Modifier.width(12.dp))
                    Text(a.display_name, style = MaterialTheme.typography.bodyLarge)
                    if (a.id == s.agent) {
                        Spacer(Modifier.width(8.dp))
                        Text("already here", style = MaterialTheme.typography.labelSmall, color = Muted)
                    }
                    Spacer(Modifier.weight(1f))
                    RadioButton(selected = agentId == a.id, onClick = { agentId = a.id })
                }
            }
            if (!sel?.models.isNullOrEmpty()) {
                Spacer(Modifier.height(6.dp))
                var modelPicker by remember { mutableStateOf(false) }
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).clickable { modelPicker = true }
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Model", style = MaterialTheme.typography.labelMedium, color = Muted)
                    Spacer(Modifier.weight(1f))
                    model?.let { AgentIcon(modelBrand(it, agentId.removePrefix("api:")), 16.dp); Spacer(Modifier.width(6.dp)) }
                    Text(
                        sel!!.models.firstOrNull { it.id == model }?.display_name ?: "Default",
                        style = MaterialTheme.typography.bodyMedium,
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                    Icon(Icons.Filled.KeyboardArrowDown, null, tint = Muted)
                }
                if (modelPicker) {
                    ModelPickerSheet(
                        models = sel.models,
                        fallbackBrand = agentId.removePrefix("api:"),
                        allowDefault = !agentId.startsWith("api:"),
                        onPick = { model = it; modelPicker = false },
                        onDismiss = { modelPicker = false },
                    )
                }
            }
            Spacer(Modifier.height(14.dp))
            OutlinedTextField(
                value = focus, onValueChange = { focus = it },
                placeholder = { Text("What should they look at? (optional)", color = Muted) },
                minLines = 2, maxLines = 4,
                shape = RoundedCornerShape(12.dp),
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(16.dp))
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                listOf(1 to "Quick", 2 to "Normal", 3 to "Long").forEachIndexed { i, (n, label) ->
                    SegmentedButton(
                        selected = rounds == n,
                        onClick = { rounds = n },
                        shape = SegmentedButtonDefaults.itemShape(i, 3),
                    ) { Text(label) }
                }
            }
            Spacer(Modifier.height(6.dp))
            Text(
                when (rounds) {
                    1 -> "They speak once; the agent answers."
                    2 -> "One exchange, then a reply back."
                    else -> "Three full exchanges."
                },
                style = MaterialTheme.typography.labelSmall, color = Muted,
                modifier = Modifier.fillMaxWidth(), textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            )
            Spacer(Modifier.height(12.dp))
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).clickable { auto = !auto }
                    .padding(horizontal = 12.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text("Auto-continue", style = MaterialTheme.typography.bodyMedium)
                    Text(
                        "When they finish, ${s.agent.replaceFirstChar { it.uppercase() }} acts on the outcome without waiting for you",
                        style = MaterialTheme.typography.labelSmall, color = Muted,
                    )
                }
                androidx.compose.material3.Switch(checked = auto, onCheckedChange = { auto = it })
            }
            Spacer(Modifier.height(12.dp))
            Button(
                onClick = { vm.bringIn(s, agentId, model, focus.trim(), rounds, auto); onDismiss() },
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(14.dp),
            ) { Text("Bring them in", modifier = Modifier.padding(vertical = 4.dp)) }
        }
    }
}

/** Every model the source offers — searchable, each wearing its vendor's
 *  mark. The desktop's dropdown, given room to breathe. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ModelPickerSheet(
    models: List<com.fivelime.aiterm.ModelOption>,
    fallbackBrand: String,
    allowDefault: Boolean,
    onPick: (String?) -> Unit,
    onDismiss: () -> Unit,
) {
    var q by remember { mutableStateOf("") }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = Surface1,
    ) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 16.dp)) {
            OutlinedTextField(
                value = q, onValueChange = { q = it }, singleLine = true,
                placeholder = { Text("Search models…", color = Muted) },
                shape = RoundedCornerShape(12.dp),
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(8.dp))
            val hits = models.filter { q.isBlank() || it.id.contains(q, true) || it.display_name.contains(q, true) }
            LazyColumn(Modifier.heightIn(max = 520.dp)) {
                if (allowDefault && q.isBlank()) item(key = "default") {
                    Row(
                        Modifier.fillMaxWidth().clickable { onPick(null) }.padding(horizontal = 8.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        AgentIcon(fallbackBrand, 20.dp)
                        Spacer(Modifier.width(12.dp))
                        Text("Default model")
                    }
                }
                items(hits, key = { it.id }) { m ->
                    Row(
                        Modifier.fillMaxWidth().clickable { onPick(m.id) }.padding(horizontal = 8.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        AgentIcon(modelBrand(m.id, fallbackBrand), 20.dp)
                        Spacer(Modifier.width(12.dp))
                        Column {
                            Text(m.display_name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (m.id != m.display_name) {
                                Text(m.id, style = MaterialTheme.typography.labelSmall, color = Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
            }
        }
    }
}

/** Where the second-agent exchange stands — visible from the couch. */
@Composable
private fun RelayBanner(vm: AppViewModel, s: Session, r: com.fivelime.aiterm.RelayInfo) {
    val finished = r.phase == "done" || r.phase == "stopped" || r.phase == "error"
    val text = when (r.phase) {
        "opening" -> "Bringing in ${r.bName}…"
        "waitB" -> "${r.bName} is reading and writing… (round ${r.round}/${r.rounds})"
        "waitA" -> "${s.agent.replaceFirstChar { it.uppercase() }} is replying to ${r.bName}… (round ${r.round}/${r.rounds})"
        "done" -> "Done — ${r.note.ifBlank { "both views are in" }}"
        "error" -> "Bring-in failed: ${r.note}"
        "stopped" -> "Stopped"
        else -> r.phase
    }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(if (r.phase == "error") Red.copy(alpha = 0.15f) else Surface1)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (!finished) {
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = Accent)
        } else {
            Icon(Icons.Filled.Build, null, tint = if (r.phase == "error") Red else Green, modifier = Modifier.size(14.dp))
        }
        Spacer(Modifier.width(8.dp))
        Text(text, style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f), maxLines = 2)
        r.bSessionId?.let { bid ->
            TextButton(onClick = { vm.sessions.firstOrNull { it.id == bid }?.let { vm.select(it) } }) {
                Text("Their side", style = MaterialTheme.typography.labelMedium)
            }
        }
        if (finished) {
            IconButton(onClick = { vm.dismissRelay(s.id) }, modifier = Modifier.size(28.dp)) {
                Icon(Icons.Filled.Close, "Dismiss", tint = Muted, modifier = Modifier.size(14.dp))
            }
        }
    }
}
