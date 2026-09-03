package com.fivelime.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.fivelime.aiterm.AppViewModel
import com.fivelime.aiterm.DirEntry
import kotlinx.coroutines.launch

/** Start a session on the desktop. The folder is a chip at the top; the
 *  message and its choices — harness, model, effort, a name, files — sit in
 *  the composer at the bottom, the way Claude Code's app does it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NewSessionScreen(vm: AppViewModel, outer: PaddingValues) {
    val folders = remember(vm.sessions) { vm.sessions.map { it.group_path }.distinct() }
    var folder by remember { mutableStateOf(folders.firstOrNull() ?: "") }
    var agent by remember { mutableStateOf(vm.agents.firstOrNull()) }
    var model by remember(agent) { mutableStateOf(agent?.models?.firstOrNull()) }
    var effort by remember(model) { mutableStateOf(model?.default_effort) }
    var prompt by remember { mutableStateOf("") }
    var title by remember { mutableStateOf("") }
    var folderMenu by remember { mutableStateOf(false) }
    var browsing by remember { mutableStateOf(false) }
    if (browsing) {
        FolderPicker(
            vm,
            start = folder.ifBlank { folders.firstOrNull() ?: "/" },
            onPick = { folder = it; browsing = false },
            onDismiss = { browsing = false },
        )
    }
    val canStart = agent != null && folder.isNotBlank() && (prompt.isNotBlank() || vm.attachments.isNotEmpty() || title.isNotBlank() || true)

    Scaffold(
        modifier = Modifier.padding(outer).imePadding().dismissKeyboardOnTap(),
        containerColor = Bg,
        topBar = {
            TopAppBar(
                navigationIcon = { IconButton(onClick = { vm.composingNew = false }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
                title = { Text("New session", modifier = Modifier.fillMaxWidth(), textAlign = TextAlign.Center) },
                actions = { Spacer(Modifier.width(48.dp)) },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Bg),
            )
        },
        bottomBar = {
            Column(Modifier.fillMaxWidth().navigationBarsPadding()) {
                AttachmentChips(vm)
                Column(
                    Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp)
                        .background(Surface1, RoundedCornerShape(22.dp)).padding(8.dp),
                ) {
                    TextField(
                        value = prompt, onValueChange = { prompt = it },
                        placeholder = { Text("Describe what you want to build…", color = Muted) },
                        modifier = Modifier.fillMaxWidth(), maxLines = 8,
                        colors = TextFieldDefaults.colors(
                            focusedContainerColor = Color.Transparent, unfocusedContainerColor = Color.Transparent,
                            focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                        ),
                    )
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        AttachButton(vm)
                        Spacer(Modifier.weight(1f))
                        FilledIconButton(
                            onClick = { vm.newSession(agent!!.id, folder.trim(), prompt, model?.id, effort, title) },
                            enabled = canStart,
                            colors = IconButtonDefaults.filledIconButtonColors(containerColor = Accent, contentColor = Bg),
                        ) { Icon(Icons.AutoMirrored.Filled.Send, "Start") }
                    }
                }
            }
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding), horizontalAlignment = Alignment.CenterHorizontally) {
            Spacer(Modifier.height(16.dp))
            // Where it runs. The folders offered are where sessions already live.
            Box {
                Row(
                    Modifier.background(Surface1, RoundedCornerShape(24.dp)).clickable { folderMenu = true }.padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.Filled.Folder, null, tint = Muted, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text(if (folder.isBlank()) "Choose a folder" else folderName(folder), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Icon(Icons.Filled.KeyboardArrowDown, null, tint = Muted, modifier = Modifier.size(18.dp))
                }
                DropdownMenu(expanded = folderMenu, onDismissRequest = { folderMenu = false }) {
                    folders.forEach { f -> DropdownMenuItem(text = { Text(f) }, onClick = { folder = f; folderMenu = false }) }
                    HorizontalDivider()
                    DropdownMenuItem(
                        text = { Text("Browse…", color = Accent) },
                        onClick = { folderMenu = false; browsing = true },
                    )
                }
            }
            Text(folder, style = MaterialTheme.typography.labelSmall, color = Muted, modifier = Modifier.padding(top = 6.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Spacer(Modifier.height(20.dp))
            // The choices, one to a row, top to bottom in the order they
            // depend on each other: the engine decides the models, the model
            // the efforts. A row that has nothing to offer is not drawn.
            Column(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp)
                    .background(Surface1, RoundedCornerShape(18.dp)).padding(horizontal = 14.dp, vertical = 4.dp),
            ) {
                ChoiceRow("Agent") {
                    PickerChip(
                        label = agent?.display_name ?: "Choose",
                        options = vm.agents.map { it.id to it.display_name },
                        onPick = { id -> agent = vm.agents.find { it.id == id } },
                        leading = { agent?.let { AgentIcon(it.id, 16.dp) } },
                        icon = { id -> AgentIcon(id, 20.dp) },
                    )
                }
                if (!agent?.models.isNullOrEmpty()) {
                    HorizontalDivider(color = Surface2)
                    ChoiceRow("Model") {
                        PickerChip(
                            label = model?.display_name ?: "Choose",
                            options = agent!!.models.map { it.id to it.display_name },
                            onPick = { id -> model = agent!!.models.find { it.id == id } },
                        )
                    }
                }
                if (!model?.efforts.isNullOrEmpty()) {
                    HorizontalDivider(color = Surface2)
                    ChoiceRow("Effort") {
                        PickerChip(
                            label = effort?.replaceFirstChar { it.uppercase() } ?: "Auto",
                            options = listOf("" to "Auto") + model!!.efforts.map { it to it.replaceFirstChar { c -> c.uppercase() } },
                            onPick = { effort = it.ifEmpty { null } },
                        )
                    }
                }
                HorizontalDivider(color = Surface2)
                ChoiceRow("Name", fill = true) {
                    TextField(
                        value = title, onValueChange = { title = it }, singleLine = true,
                        placeholder = { Text("Optional", color = Muted, textAlign = TextAlign.End, modifier = Modifier.fillMaxWidth()) },
                        textStyle = MaterialTheme.typography.bodyMedium.copy(textAlign = TextAlign.End),
                        modifier = Modifier.weight(1f),
                        colors = TextFieldDefaults.colors(
                            focusedContainerColor = Color.Transparent, unfocusedContainerColor = Color.Transparent,
                            focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                        ),
                    )
                }
            }
            Spacer(Modifier.weight(1f))
            Text("on ${vm.desktop?.name ?: "desktop"}", style = MaterialTheme.typography.labelMedium, color = Muted, modifier = Modifier.padding(bottom = 12.dp))
        }
    }
}

/** One step: what it is on the left, the choice on the right. */
@Composable
fun ChoiceRow(label: String, fill: Boolean = false, choice: @Composable RowScope.() -> Unit) {
    Row(Modifier.fillMaxWidth().height(52.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, style = MaterialTheme.typography.bodyMedium, color = Muted, maxLines = 1)
        Spacer(if (fill) Modifier.width(12.dp) else Modifier.weight(1f))
        choice()
    }
}

/** Walk the desktop's folders and pick one — or make one. Up goes toward
 *  home; refusals (outside home) surface as a notice and stay put. */
@Composable
private fun FolderPicker(vm: AppViewModel, start: String, onPick: (String) -> Unit, onDismiss: () -> Unit) {
    var path by remember { mutableStateOf(start.trimEnd('/')) }
    var dirs by remember { mutableStateOf<List<DirEntry>>(emptyList()) }
    var newFolder by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(path) {
        dirs = runCatching { vm.listDirs(path) }.getOrElse {
            vm.notice = "Can't open $path"
            emptyList()
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Column {
                Text("Folder on ${vm.desktop?.name ?: "desktop"}")
                Text(path, style = MaterialTheme.typography.labelSmall, color = Muted, maxLines = 2)
            }
        },
        text = {
            Column(Modifier.height(380.dp)) {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    TextButton(
                        onClick = {
                            val up = path.substringBeforeLast('/').ifEmpty { "/" }
                            if (up != path) path = up
                        },
                    ) { Text("↑ Up") }
                    Spacer(Modifier.weight(1f))
                    TextButton(onClick = { newFolder = true }) { Text("New folder") }
                }
                if (dirs.isEmpty()) {
                    Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                        Text("No subfolders", color = Muted)
                    }
                } else {
                    LazyColumn(Modifier.weight(1f)) {
                        items(dirs.filter { !it.name.startsWith(".") }, key = { it.path }) { d ->
                            Row(
                                Modifier.fillMaxWidth().clickable { path = d.path }.padding(vertical = 10.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Icon(Icons.Filled.Folder, null, tint = Accent, modifier = Modifier.size(20.dp))
                                Spacer(Modifier.width(10.dp))
                                Text(d.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = { onPick(path) }) { Text("Use this folder") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
    if (newFolder) {
        var name by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { newFolder = false },
            title = { Text("New folder in ${folderName(path)}") },
            text = {
                OutlinedTextField(
                    value = name, onValueChange = { name = it }, singleLine = true,
                    placeholder = { Text("folder-name", color = Muted) },
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        val n = name.trim().trim('/')
                        if (n.isEmpty()) return@TextButton
                        scope.launch {
                            runCatching { vm.createDir("$path/$n") }
                                .onSuccess { newFolder = false; path = "$path/$n" }
                                .onFailure { vm.notice = "Could not create: ${it.message}" }
                        }
                    },
                ) { Text("Create") }
            },
            dismissButton = { TextButton(onClick = { newFolder = false }) { Text("Cancel") } },
        )
    }
}
