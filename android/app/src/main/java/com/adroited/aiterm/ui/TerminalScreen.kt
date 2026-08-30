package com.adroited.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.text
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.terminal.CellAttributes
import com.adroited.aiterm.terminal.CursorShape
import com.adroited.aiterm.terminal.ScreenCell
import com.adroited.aiterm.terminal.ScreenSnapshot
import com.adroited.aiterm.terminal.ScreenRow
import com.adroited.aiterm.terminal.TerminalColor
import kotlinx.coroutines.launch
import java.net.URI

@Composable
fun RemoteTerminalScreen(viewModel: RemoteTerminalViewModel, onBack: () -> Unit) {
    val state by viewModel.client.state.collectAsStateWithLifecycle()
    val screen by viewModel.client.screen.collectAsStateWithLifecycle()
    val scrollback by viewModel.client.scrollback.collectAsStateWithLifecycle()
    TerminalScreenContent(
        state = state,
        screen = screen,
        scrollback = scrollback,
        onBack = onBack,
        onReconnect = viewModel::reconnect,
        onSelectTab = viewModel::selectTab,
        onCloseTab = viewModel::closeTab,
        onOpenSession = { id, cols, rows -> viewModel.openSession(id, cols, rows) },
        onPreviewSession = viewModel::previewSession,
        onCloseSession = viewModel::closeSession,
        onStopSession = viewModel::stopSession,
        onForkSession = viewModel::forkSession,
        onDeleteSession = viewModel::deleteSession,
        onOpenShell = { cols, rows -> viewModel.openShell(null, cols, rows) },
        onStartAgent = { agent, model, effort, cwd, cols, rows ->
            viewModel.startAgent(agent, model, effort, cwd, cols, rows)
        },
        onInput = viewModel::sendInput,
        onTakeFocus = viewModel::takeFocus,
        onResize = viewModel::resize,
        onLoadScrollback = viewModel::loadOlderScrollback,
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TerminalScreenContent(
    state: RemoteClientState,
    screen: ScreenSnapshot?,
    scrollback: List<ScreenRow> = emptyList(),
    onBack: () -> Unit = {},
    onReconnect: () -> Unit = {},
    onSelectTab: (String) -> Unit = {},
    onCloseTab: (String) -> Unit = {},
    onOpenSession: (String, Int, Int) -> Unit = { _, _, _ -> },
    onPreviewSession: (String) -> Unit = {},
    onCloseSession: (String) -> Unit = {},
    onStopSession: (String) -> Unit = {},
    onForkSession: (String) -> Unit = {},
    onDeleteSession: (String) -> Unit = {},
    onOpenShell: (Int, Int) -> Unit = { _, _ -> },
    onStartAgent: (
        com.adroited.aiterm.remote.RemoteAgentChoice,
        String?,
        String?,
        String,
        Int,
        Int,
    ) -> Unit = { _, _, _, _, _, _ -> },
    onInput: (String) -> Unit = {},
    onTakeFocus: (Int, Int) -> Unit = { _, _ -> },
    onResize: (Int, Int) -> Unit = { _, _ -> },
    onLoadScrollback: () -> Unit = {},
) {
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val coroutineScope = rememberCoroutineScope()
    var cols by remember { mutableIntStateOf(screen?.cols ?: 80) }
    var rows by remember { mutableIntStateOf(screen?.rows ?: 24) }
    var deleteTarget by remember { mutableStateOf<RemoteSession?>(null) }
    val terminalMetrics = rememberTerminalMetrics()

    ModalNavigationDrawer(
        drawerState = drawerState,
        drawerContent = {
            ModalDrawerSheet(modifier = Modifier.fillMaxHeight().width(340.dp)) {
                SessionDrawer(
                    state = state,
                    cols = cols,
                    rows = rows,
                    onSelectTab = {
                        onSelectTab(it)
                        coroutineScope.launch { drawerState.close() }
                    },
                    onCloseTab = onCloseTab,
                    onOpenSession = { onOpenSession(it, cols, rows) },
                    onPreviewSession = onPreviewSession,
                    onCloseSession = onCloseSession,
                    onStopSession = onStopSession,
                    onForkSession = onForkSession,
                    onDeleteSession = { id -> deleteTarget = state.sessions.firstOrNull { it.id == id } },
                    onOpenShell = { onOpenShell(cols, rows) },
                    onStartAgent = { agent, model, effort, cwd ->
                        onStartAgent(agent, model, effort, cwd, cols, rows)
                    },
                )
            }
        },
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    navigationIcon = {
                        TextButton(onClick = { coroutineScope.launch { drawerState.open() } }) {
                            Text("Sessions")
                        }
                    },
                    title = {
                        Column {
                            Text(state.activeTitle ?: "Remote terminal")
                            Text(
                                state.connection.label(),
                                style = MaterialTheme.typography.labelMedium,
                                color = state.connection.color(),
                            )
                        }
                    },
                    actions = { TextButton(onClick = onBack) { Text("Back") } },
                )
            },
        ) { padding ->
            Column(Modifier.fillMaxSize().padding(padding)) {
                ConnectionRail(state, onReconnect)
                BoxWithConstraints(
                    Modifier.weight(1f).fillMaxWidth()
                        .background(Color(0xFF07111B))
                        .padding(horizontal = 4.dp, vertical = 3.dp),
                ) {
                    val measuredCols = (maxWidth / terminalMetrics.cellWidth).toInt().coerceIn(1, 512)
                    val measuredRows = (maxHeight / terminalMetrics.lineHeight).toInt().coerceIn(1, 512)
                    LaunchedEffect(measuredCols, measuredRows, screen?.tabId) {
                        cols = measuredCols
                        rows = measuredRows
                        if (screen != null) onResize(measuredCols, measuredRows)
                    }
                    TerminalGrid(
                        screen = screen,
                        scrollback = scrollback,
                        modifier = Modifier.fillMaxSize(),
                        onInput = onInput,
                        metrics = terminalMetrics,
                    )
                    if (screen == null) {
                        Text(
                            if (state.tabs.isEmpty()) "No remote tabs are open." else "Choose a tab from Sessions.",
                            modifier = Modifier.align(Alignment.Center),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                if (state.showTakeFocus && screen != null) {
                    Button(
                        onClick = { onTakeFocus(cols, rows) },
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
                    ) { Text("Take Focus") }
                }
                if (screen != null) {
                    TextButton(
                        onClick = onLoadScrollback,
                        modifier = Modifier.fillMaxWidth().height(36.dp).testTag("load-scrollback"),
                    ) { Text("Load older history · ${scrollback.size} rows") }
                }
                ExtraKeys(screen, scrollback, onInput)
            }
        }
    }

    deleteTarget?.let { session ->
        AlertDialog(
            onDismissRequest = { deleteTarget = null },
            title = { Text("Delete transcript?") },
            text = { Text("${session.title}\n\nThis permanently removes the desktop session after its protected archive transaction completes.") },
            confirmButton = {
                Button(onClick = {
                    onDeleteSession(session.id)
                    deleteTarget = null
                }) { Text("Delete") }
            },
            dismissButton = { TextButton(onClick = { deleteTarget = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun ConnectionRail(state: RemoteClientState, onReconnect: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth()
            .background(state.connection.color().copy(alpha = 0.14f))
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(state.connection.label(), style = MaterialTheme.typography.labelMedium)
        state.lastError?.let {
            Text("  $it", modifier = Modifier.weight(1f), maxLines = 1)
        } ?: Spacer(Modifier.weight(1f))
        if (state.connection == ConnectionState.Disconnected) {
            TextButton(onClick = onReconnect) { Text("Reconnect") }
        }
    }
}

@Composable
private fun SessionDrawer(
    state: RemoteClientState,
    cols: Int,
    rows: Int,
    onSelectTab: (String) -> Unit,
    onCloseTab: (String) -> Unit,
    onOpenSession: (String) -> Unit,
    onPreviewSession: (String) -> Unit,
    onCloseSession: (String) -> Unit,
    onStopSession: (String) -> Unit,
    onForkSession: (String) -> Unit,
    onDeleteSession: (String) -> Unit,
    onOpenShell: () -> Unit,
    onStartAgent: (com.adroited.aiterm.remote.RemoteAgentChoice, String?, String?, String) -> Unit,
) {
    Text("LIVE TABS", modifier = Modifier.padding(16.dp), style = MaterialTheme.typography.labelMedium)
    state.tabs.forEach { tab ->
        Row(
            Modifier.fillMaxWidth().clickable { onSelectTab(tab.id) }.padding(horizontal = 16.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(if (tab.id == state.activeTabId) "●" else "○", color = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Text(tab.title, maxLines = 1)
                Text("${tab.size.cols}×${tab.size.rows} · ${tab.focus.name.lowercase()}", style = MaterialTheme.typography.labelMedium)
            }
            TextButton(onClick = { onCloseTab(tab.id) }) { Text("Close") }
        }
    }
    OutlinedButton(onClick = onOpenShell, modifier = Modifier.padding(horizontal = 16.dp)) {
        Text("New shell ${cols}×${rows}")
    }
    HorizontalDivider(Modifier.padding(vertical = 12.dp))
    val launchPath = state.sessions.firstOrNull()?.projectPath
    if (state.agents.isNotEmpty() && launchPath != null) {
        Text("NEW AGENT", modifier = Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.labelMedium)
        state.agents.forEach { agent ->
            Column(Modifier.padding(horizontal = 12.dp)) {
                if (agent.models.isEmpty()) {
                    TextButton(onClick = { onStartAgent(agent, null, null, launchPath) }) {
                        Text("Start ${agent.displayName} · default")
                    }
                }
                agent.models.forEach { model ->
                    val efforts = model.efforts.ifEmpty { listOfNotNull(model.defaultEffort) }
                    if (efforts.isEmpty()) {
                        TextButton(onClick = { onStartAgent(agent, model.id, null, launchPath) }) {
                            Text("Start ${agent.displayName} · ${model.displayName}")
                        }
                    } else {
                        efforts.forEach { effort ->
                            TextButton(onClick = { onStartAgent(agent, model.id, effort, launchPath) }) {
                                Text("Start ${agent.displayName} · ${model.displayName} · $effort")
                            }
                        }
                    }
                }
                val caps = state.agentCaps[agent.id]
                if (caps != null) {
                    Text(
                        listOfNotNull(
                            "resume".takeIf { caps.resume },
                            "fork".takeIf { caps.fork },
                            "tasks".takeIf { caps.tasks },
                            "delete".takeIf { caps.delete },
                        ).joinToString(" · ").ifBlank { "terminal only" },
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
        HorizontalDivider(Modifier.padding(vertical = 12.dp))
    }
    Text("SESSIONS", modifier = Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.labelMedium)
    LazyColumn(modifier = Modifier.fillMaxHeight()) {
        items(state.sessions, key = RemoteSession::id) { session ->
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 9.dp)) {
                Text(session.title, maxLines = 1)
                Text("${session.agent} · ${session.projectPath}", style = MaterialTheme.typography.labelMedium, maxLines = 1)
                Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                    TextButton(onClick = { onOpenSession(session.id) }) { Text("Open") }
                    TextButton(onClick = { onPreviewSession(session.id) }) { Text("Preview") }
                    TextButton(onClick = { onCloseSession(session.id) }) { Text("Close") }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                    TextButton(onClick = { onStopSession(session.id) }) { Text("Stop") }
                    TextButton(onClick = { onForkSession(session.id) }) { Text("Fork") }
                    TextButton(onClick = { onDeleteSession(session.id) }) { Text("Delete") }
                }
                if (state.previewSessionId == session.id) {
                    state.previewMessages.takeLast(8).forEach { message ->
                        Text(
                            "${message.role}: ${message.text}",
                            style = MaterialTheme.typography.bodySmall,
                            maxLines = 3,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun TerminalGrid(
    screen: ScreenSnapshot?,
    scrollback: List<ScreenRow>,
    modifier: Modifier,
    onInput: (String) -> Unit,
    metrics: TerminalMetrics,
) {
    val focusRequester = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    val terminalRows = remember(scrollback, screen?.visible) {
        scrollback.asReversed() + (screen?.visible ?: emptyList())
    }
    val listState = rememberLazyListState(
        initialFirstVisibleItemIndex = scrollback.size.coerceAtMost(terminalRows.lastIndex.coerceAtLeast(0)),
    )
    var imeValue by remember { mutableStateOf(TextFieldValue()) }
    val density = LocalDensity.current
    Box(
        modifier.clickable {
            focusRequester.requestFocus()
            keyboard?.show()
        }.testTag("terminal-grid"),
    ) {
        SelectionContainer {
            LazyColumn(
                state = listState,
                modifier = Modifier.fillMaxSize().testTag("terminal-render-content"),
            ) {
                itemsIndexed(
                    terminalRows,
                    key = { index, _ -> "${screen?.tabId ?: "none"}:$index" },
                ) { index, row ->
                    TerminalRowGrid(row, index, metrics)
                }
            }
        }
        val cursor = screen?.cursor
        val cursorIndex = cursor?.let { scrollback.size + it.row }
        val cursorItem = cursorIndex?.let { wanted ->
            listState.layoutInfo.visibleItemsInfo.firstOrNull { it.index == wanted }
        }
        if (cursor != null && cursor.visible && cursorItem != null) {
            val color = Color(0xFF63D3E1)
            Box(
                Modifier
                    .offset(
                        x = metrics.cellWidth * cursor.col,
                        y = with(density) { cursorItem.offset.toDp() },
                    )
                    .size(metrics.cellWidth, metrics.lineHeight)
                    .testTag("terminal-cursor")
                    .drawBehind {
                        val thickness = 2.dp.toPx()
                        when (cursor.shape) {
                            CursorShape.Block -> drawRect(color.copy(alpha = 0.35f))
                            CursorShape.Beam -> drawRect(color, size = androidx.compose.ui.geometry.Size(thickness, size.height))
                            CursorShape.Underline -> drawRect(
                                color,
                                topLeft = androidx.compose.ui.geometry.Offset(0f, size.height - thickness),
                                size = androidx.compose.ui.geometry.Size(size.width, thickness),
                            )
                        }
                    },
            )
        }
        BasicTextField(
            value = imeValue,
            onValueChange = { next ->
                if (next.composition == null && next.text.isNotEmpty()) {
                    onInput(next.text.replace("\n", "\r"))
                    imeValue = TextFieldValue()
                } else {
                    imeValue = next
                }
            },
            modifier = Modifier.size(1.dp).focusRequester(focusRequester)
                .onPreviewKeyEvent { event ->
                    if (event.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                    terminalKeySequence(event.key, screen?.modes?.applicationCursor == true)?.let {
                        onInput(it)
                        true
                    } ?: false
                }.testTag("terminal-input"),
            textStyle = TextStyle(color = Color.Transparent),
        )
    }
}

@Composable
private fun TerminalRowGrid(row: ScreenRow, rowIndex: Int, metrics: TerminalMetrics) {
    val uriHandler = LocalUriHandler.current
    val plain = row.plainText()
    val links = SAFE_LINK.findAll(plain).mapNotNull { match ->
        val candidate = match.value.trimEnd('.', ',', ')', ']', '}')
        candidate.takeIf { it.length <= 2_048 && isSafeRemoteLink(it) }?.let {
            match.range.first until (match.range.first + candidate.length) to candidate
        }
    }.toList()
    var textOffset = 0
    Row(
        Modifier.height(metrics.lineHeight).testTag("terminal-row")
            .semantics(mergeDescendants = true) { text = AnnotatedString(plain) },
    ) {
        row.cells.forEachIndexed { cellIndex, cell ->
            if (cell.continuation) return@forEachIndexed
            val cellRange = textOffset until (textOffset + cell.text.length)
            val link = links.firstOrNull { (range, _) ->
                cellRange.first < range.last + 1 && range.first < cellRange.last + 1
            }?.second
            textOffset += cell.text.length
            val slotBackground = when {
                cell.attributes.inverse -> cell.foreground.color(Color(0xFFD8E6EF))
                else -> cell.background.color(Color.Transparent)
            }
            var slot = Modifier.width(metrics.cellWidth * cell.width).height(metrics.lineHeight)
                .background(slotBackground).testTag("terminal-cell-$rowIndex-$cellIndex")
            if (link != null) slot = slot.clickable { uriHandler.openUri(link) }
            Box(slot) {
                TerminalCellText(cell, linked = link != null, metrics = metrics)
            }
        }
    }
}

@Composable
private fun TerminalCellText(cell: ScreenCell, linked: Boolean, metrics: TerminalMetrics) {
    val text = buildAnnotatedString {
        val foreground = cell.foreground.color(default = Color(0xFFD8E6EF))
        val background = cell.background.color(default = Color.Transparent)
        val effectiveForeground = when {
            cell.attributes.hidden -> Color.Transparent
            cell.attributes.inverse -> background.ifTransparent(Color(0xFF07111B))
            cell.attributes.faint -> foreground.copy(alpha = 0.58f)
            else -> foreground
        }
        val effectiveBackground = when {
            else -> Color.Transparent
        }
        val decoration = when {
            linked && cell.attributes.strikethrough ->
                TextDecoration.combine(listOf(TextDecoration.Underline, TextDecoration.LineThrough))
            linked -> TextDecoration.Underline
            else -> null
        }
        pushStyle(cell.attributes.span(effectiveForeground, effectiveBackground).let { style ->
            if (decoration == null) style else style.copy(textDecoration = decoration)
        })
        append(cell.text)
        pop()
    }
    BasicText(
        text = text,
        style = metrics.textStyle,
        maxLines = 1,
        softWrap = false,
    )
}

private data class TerminalMetrics(
    val cellWidth: Dp,
    val lineHeight: Dp,
    val textStyle: TextStyle,
)

@Composable
private fun rememberTerminalMetrics(): TerminalMetrics {
    val density = LocalDensity.current
    val measurer = rememberTextMeasurer()
    val textStyle = remember(density.density, density.fontScale) {
        TextStyle(
            color = Color(0xFFD8E6EF),
            fontFamily = FontFamily.Monospace,
            fontSize = 13.sp,
            lineHeight = 16.sp,
        )
    }
    val measured = measurer.measure(
        text = AnnotatedString("M"),
        style = textStyle,
        maxLines = 1,
        softWrap = false,
    )
    return TerminalMetrics(
        cellWidth = with(density) { measured.size.width.toDp() },
        lineHeight = with(density) { measured.size.height.toDp() },
        textStyle = textStyle,
    )
}

@Composable
private fun ExtraKeys(screen: ScreenSnapshot?, scrollback: List<ScreenRow>, onInput: (String) -> Unit) {
    var control by remember { mutableStateOf(false) }
    var alt by remember { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current
    val applicationCursor = screen?.modes?.applicationCursor == true
    fun send(value: String) {
        var output = value
        if (control && output.length == 1) {
            val code = output[0].uppercaseChar().code
            if (code in 64..95) output = (code and 0x1f).toChar().toString()
        }
        if (alt) output = "\u001b$output"
        onInput(output)
        control = false
        alt = false
    }
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())
            .background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 4.dp, vertical = 3.dp)
            .testTag("extra-keys"),
        horizontalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        ExtraKey("Esc") { send("\u001b") }
        ExtraKey(if (control) "Ctrl ●" else "Ctrl") { control = !control }
        ExtraKey(if (alt) "Alt ●" else "Alt") { alt = !alt }
        ExtraKey("Tab") { send("\t") }
        ExtraKey("Enter") { send("\r") }
        ExtraKey("⌫") { send("\u007f") }
        ExtraKey("←") { send(if (applicationCursor) "\u001bOD" else "\u001b[D") }
        ExtraKey("↑") { send(if (applicationCursor) "\u001bOA" else "\u001b[A") }
        ExtraKey("↓") { send(if (applicationCursor) "\u001bOB" else "\u001b[B") }
        ExtraKey("→") { send(if (applicationCursor) "\u001bOC" else "\u001b[C") }
        ExtraKey("PgUp") { send("\u001b[5~") }
        ExtraKey("PgDn") { send("\u001b[6~") }
        ExtraKey("|") { send("|") }
        ExtraKey("/") { send("/") }
        ExtraKey("~") { send("~") }
        ExtraKey("Paste") {
            clipboard.getText()?.text?.let { text ->
                send(if (screen?.modes?.bracketedPaste == true) "\u001b[200~$text\u001b[201~" else text)
            }
        }
        ExtraKey("Copy screen") {
            val text = (scrollback.asReversed() + (screen?.visible ?: emptyList()))
                .joinToString("\n", transform = ScreenRow::plainText)
            clipboard.setText(AnnotatedString(text))
        }
    }
}

@Composable
private fun ExtraKey(label: String, action: () -> Unit) {
    OutlinedButton(onClick = action, modifier = Modifier.height(38.dp)) { Text(label) }
}

private fun CellAttributes.span(foreground: Color, background: Color) = SpanStyle(
    color = foreground,
    background = background,
    fontWeight = if (bold) FontWeight.Bold else FontWeight.Normal,
    fontStyle = if (italic) FontStyle.Italic else FontStyle.Normal,
    textDecoration = when {
        underline && strikethrough -> TextDecoration.combine(listOf(TextDecoration.Underline, TextDecoration.LineThrough))
        underline -> TextDecoration.Underline
        strikethrough -> TextDecoration.LineThrough
        else -> null
    },
)

private fun TerminalColor.color(default: Color): Color = when (this) {
    TerminalColor.Default -> default
    is TerminalColor.Rgb -> Color(red, green, blue)
    is TerminalColor.Indexed -> terminalIndexedColor(index)
}

internal fun terminalIndexedColor(index: Int): Color {
    val value = index.coerceIn(0, 255)
    if (value < 16) return TERMINAL_PALETTE[value]
    if (value < 232) {
        val cube = value - 16
        val levels = intArrayOf(0, 95, 135, 175, 215, 255)
        return Color(levels[cube / 36], levels[(cube / 6) % 6], levels[cube % 6])
    }
    val gray = 8 + (value - 232) * 10
    return Color(gray, gray, gray)
}

internal fun terminalKeySequence(key: Key, applicationCursor: Boolean): String? = when (key) {
    Key.Backspace -> "\u007f"
    Key.Enter, Key.NumPadEnter -> "\r"
    Key.Tab -> "\t"
    Key.Escape -> "\u001b"
    Key.DirectionLeft -> if (applicationCursor) "\u001bOD" else "\u001b[D"
    Key.DirectionUp -> if (applicationCursor) "\u001bOA" else "\u001b[A"
    Key.DirectionDown -> if (applicationCursor) "\u001bOB" else "\u001b[B"
    Key.DirectionRight -> if (applicationCursor) "\u001bOC" else "\u001b[C"
    else -> null
}

private fun Color.ifTransparent(fallback: Color): Color = if (alpha == 0f) fallback else this

private fun ConnectionState.label(): String = when (this) {
    ConnectionState.Disconnected -> "DISCONNECTED"
    ConnectionState.Connecting -> "CONNECTING"
    ConnectionState.Connected -> "CONNECTED"
    ConnectionState.Reconnecting -> "RECONNECTING"
    ConnectionState.Locked -> "LOCKED"
    ConnectionState.Revoked -> "ACCESS REVOKED"
}

@Composable
private fun ConnectionState.color(): Color = when (this) {
    ConnectionState.Connected -> MaterialTheme.colorScheme.tertiary
    ConnectionState.Connecting, ConnectionState.Reconnecting -> MaterialTheme.colorScheme.primary
    ConnectionState.Disconnected, ConnectionState.Locked, ConnectionState.Revoked -> MaterialTheme.colorScheme.error
}

private val TERMINAL_PALETTE = listOf(
    Color(0xFF07111B), Color(0xFFC94F56), Color(0xFF54B399), Color(0xFFD6A84B),
    Color(0xFF5C91D9), Color(0xFFB677D0), Color(0xFF52B8C8), Color(0xFFD8E6EF),
    Color(0xFF536575), Color(0xFFFF7378), Color(0xFF70D9B7), Color(0xFFFFCC66),
    Color(0xFF79AFFF), Color(0xFFD892EA), Color(0xFF74D9EA), Color(0xFFFFFFFF),
)

private val SAFE_LINK = Regex("https?://[^\\s<>{}\\[\\]\\\"']+")

internal fun isSafeRemoteLink(candidate: String): Boolean = try {
    val uri = URI(candidate)
    uri.scheme?.lowercase() in setOf("http", "https") && !uri.host.isNullOrBlank() && uri.userInfo == null
} catch (_: Exception) {
    false
}
