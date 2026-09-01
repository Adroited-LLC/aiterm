package com.fivelime.aiterm.ui

import android.content.Intent
import android.widget.MediaController
import android.widget.VideoView
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Audiotrack
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Image
import androidx.compose.material.icons.filled.InsertDriveFile
import androidx.compose.material.icons.filled.OpenInNew
import androidx.compose.material.icons.filled.PictureAsPdf
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.FileProvider
import coil3.compose.AsyncImage
import com.fivelime.aiterm.AppViewModel
import com.fivelime.aiterm.FileEntry
import java.io.File

/** Two views of a session's files: what it produced (the desktop's change
 *  ledger, newest first), and its workspace folder as the explorer shows
 *  it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FilesList(vm: AppViewModel, modifier: Modifier = Modifier) {
    LaunchedEffect(vm.selected?.id) { vm.loadFiles() }
    Column(modifier.fillMaxSize()) {
        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp).height(34.dp)) {
            SegmentedButton(selected = !vm.browsing, onClick = { vm.browsing = false },
                shape = SegmentedButtonDefaults.itemShape(0, 2)) { Text("Changes", style = MaterialTheme.typography.labelMedium) }
            SegmentedButton(selected = vm.browsing, onClick = { vm.browsing = true; if (vm.browsePath.isEmpty()) vm.browseTo(vm.browseRoot) },
                shape = SegmentedButtonDefaults.itemShape(1, 2)) { Text("Browse", style = MaterialTheme.typography.labelMedium) }
        }
        if (vm.browsing) BrowseList(vm, Modifier.fillMaxSize()) else ChangesList(vm, Modifier.fillMaxSize())
    }
}

@Composable
private fun BrowseList(vm: AppViewModel, modifier: Modifier = Modifier) {
    Column(modifier) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            if (vm.browsePath != vm.browseRoot) {
                IconButton(onClick = { vm.browseUp() }, modifier = Modifier.size(28.dp)) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Up", tint = Muted) }
                Spacer(Modifier.width(4.dp))
            }
            Text(vm.browsePath.removePrefix(vm.browseRoot).ifEmpty { folderName(vm.browseRoot) }.trimStart('/').ifEmpty { folderName(vm.browseRoot) },
                style = MaterialTheme.typography.labelMedium, color = Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        when {
            vm.browseLoading && vm.browseEntries.isEmpty() -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            vm.browseEntries.isEmpty() -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text("Empty folder", color = Muted) }
            else -> LazyColumn(Modifier.fillMaxSize()) {
                items(vm.browseEntries, key = { it.path }) { e ->
                    ListItem(
                        modifier = Modifier.clickable { vm.openBrowsed(e) },
                        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        leadingContent = {
                            if (vm.opening == e.path) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
                            else Icon(if (e.is_dir) Icons.Filled.Folder else iconFor(FileEntry(e.path, e.name, 0, 0, "").kind), null,
                                tint = if (e.is_dir) Accent else Muted)
                        },
                        headlineContent = { Text(e.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    )
                }
            }
        }
    }
}

@Composable
private fun ChangesList(vm: AppViewModel, modifier: Modifier = Modifier) {
    when {
        vm.loadingFiles && vm.files.isEmpty() -> Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        vm.files.isEmpty() -> Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text("Nothing produced yet — files this session writes show up here.", color = Muted, modifier = Modifier.padding(32.dp))
        }
        else -> {
            // What the agent produced leads; the folder's other recent
            // changes — builds, the person's own edits, whatever the walk
            // caught — stay behind a fold instead of drowning the list.
            val produced = vm.files.filter { it.via == "made" || it.via == "edited" || it.via == "wrote" }
            val other = vm.files - produced.toSet()
            var showOther by remember { mutableStateOf(produced.isEmpty()) }
            LazyColumn(modifier.fillMaxSize()) {
                items(produced, key = { "p:" + it.path }) { f -> FileRow(vm, f) }
                if (other.isNotEmpty()) {
                    item(key = "fold") {
                        ListItem(
                            modifier = Modifier.clickable { showOther = !showOther },
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                            headlineContent = {
                                Text(
                                    if (showOther) "Hide other changes in the folder"
                                    else "Other changes in the folder (${other.size})",
                                    color = Muted, style = MaterialTheme.typography.labelLarge,
                                )
                            },
                        )
                    }
                    if (showOther) items(other, key = { "o:" + it.path }) { f -> FileRow(vm, f) }
                }
            }
        }
    }
}

@Composable
private fun FileRow(vm: AppViewModel, f: FileEntry) {
    ListItem(
        modifier = Modifier.clickable { vm.open(f) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        leadingContent = {
            if (vm.opening == f.path) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
            else Icon(iconFor(f.kind), null, tint = if (f.kind == "image" || f.kind == "video") Accent else Muted)
        },
        headlineContent = { Text(f.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        supportingContent = {
            Text(
                "${relativeTime(f.modified)} · ${sizeLabel(f.bytes)} · " + when (f.via) {
                    "made" -> "made by the agent"; "edited" -> "edited by the agent"; "wrote" -> "written by the agent"
                    else -> folderName(f.path.substringBeforeLast('/'))
                },
                color = Muted, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
        },
    )
}

internal fun iconFor(kind: String) = when (kind) {
    "image" -> Icons.Filled.Image
    "video" -> Icons.Filled.Videocam
    "audio" -> Icons.Filled.Audiotrack
    "text" -> Icons.Filled.Description
    "pdf" -> Icons.Filled.PictureAsPdf
    else -> Icons.Filled.InsertDriveFile
}

fun sizeLabel(b: Long): String = when {
    b < 1024 -> "$b B"
    b < 1024 * 1024 -> "${b / 1024} KB"
    else -> "%.1f MB".format(b / 1048576.0)
}

/** One produced file, full screen: a picture, a video with controls, text
 *  to read — or a hand-off to whatever app knows the type. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FileViewer(vm: AppViewModel, entry: FileEntry, file: File, outer: PaddingValues) {
    val ctx = LocalContext.current
    val mime = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(entry.ext) ?: "*/*"
    val shareUri = { FileProvider.getUriForFile(ctx, "com.fivelime.aiterm.files", file) }
    val openWith = {
        ctx.startActivity(Intent.createChooser(Intent(Intent.ACTION_VIEW).setDataAndType(shareUri(), mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), entry.name))
    }
    val share = {
        ctx.startActivity(
            Intent.createChooser(
                Intent(Intent.ACTION_SEND).setType(mime).putExtra(Intent.EXTRA_STREAM, shareUri())
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION),
                entry.name,
            ),
        )
    }
    // Copy puts a content URI on the clipboard: paste lands the image in
    // anything that takes one (a chat, a doc, Gboard's clip tray).
    val copy = {
        val cm = ctx.getSystemService(android.content.ClipboardManager::class.java)
        cm.setPrimaryClip(android.content.ClipData.newUri(ctx.contentResolver, entry.name, shareUri()))
        vm.notice = "Copied"
    }
    // Into Photos (or Movies/Downloads by type), where the rest of the
    // phone expects saved things to be.
    val save = save@{
        if (android.os.Build.VERSION.SDK_INT < 29) { openWith(); return@save }
        val (collection, dir) = when (entry.kind) {
            "image" -> android.provider.MediaStore.Images.Media.EXTERNAL_CONTENT_URI to android.os.Environment.DIRECTORY_PICTURES
            "video" -> android.provider.MediaStore.Video.Media.EXTERNAL_CONTENT_URI to android.os.Environment.DIRECTORY_MOVIES
            else -> android.provider.MediaStore.Downloads.EXTERNAL_CONTENT_URI to android.os.Environment.DIRECTORY_DOWNLOADS
        }
        runCatching {
            val values = android.content.ContentValues().apply {
                put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, entry.name)
                put(android.provider.MediaStore.MediaColumns.MIME_TYPE, mime)
                put(android.provider.MediaStore.MediaColumns.RELATIVE_PATH, "$dir/AITerm")
            }
            val uri = ctx.contentResolver.insert(collection, values) ?: error("could not create entry")
            ctx.contentResolver.openOutputStream(uri)!!.use { out -> file.inputStream().use { it.copyTo(out) } }
            vm.notice = if (entry.kind == "image") "Saved to Photos" else "Saved to " + dir.lowercase().replaceFirstChar { it.uppercase() }
        }.onFailure { vm.notice = "Could not save: ${it.message}" }
    }
    Scaffold(
        modifier = Modifier.padding(outer),
        containerColor = Bg,
        topBar = {
            TopAppBar(
                navigationIcon = { IconButton(onClick = { vm.viewing = null }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
                title = { Text(entry.name, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMedium) },
                actions = {
                    IconButton(onClick = { save() }) { Icon(Icons.Filled.Download, "Save") }
                    if (entry.kind == "image") IconButton(onClick = copy) { Icon(Icons.Filled.ContentCopy, "Copy") }
                    IconButton(onClick = share) { Icon(Icons.Filled.Share, "Share") }
                    IconButton(onClick = openWith) { Icon(Icons.Filled.OpenInNew, "Open with…") }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Bg),
            )
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
            when (entry.kind) {
                "image" -> {
                    // Pinch to zoom, drag to pan, double-tap to toggle — a
                    // generated image is exactly the thing you lean into.
                    var scale by remember(file) { mutableStateOf(1f) }
                    var offset by remember(file) { mutableStateOf(Offset.Zero) }
                    AsyncImage(
                        model = file, contentDescription = entry.name,
                        modifier = Modifier.fillMaxSize()
                            .pointerInput(file) {
                                detectTapGestures(onDoubleTap = {
                                    scale = if (scale > 1f) 1f else 2.5f
                                    if (scale == 1f) offset = Offset.Zero
                                })
                            }
                            .pointerInput(file) {
                                detectTransformGestures { _, pan, zoom, _ ->
                                    scale = (scale * zoom).coerceIn(1f, 8f)
                                    offset = if (scale > 1f) offset + pan * scale else Offset.Zero
                                }
                            }
                            .graphicsLayer {
                                scaleX = scale; scaleY = scale
                                translationX = offset.x; translationY = offset.y
                            },
                    )
                }
                "video", "audio" -> AndroidView(
                    factory = { c ->
                        VideoView(c).apply {
                            setMediaController(MediaController(c).also { it.setAnchorView(this) })
                            setVideoPath(file.absolutePath)
                            setOnPreparedListener { it.isLooping = false; start() }
                        }
                    },
                    modifier = Modifier.fillMaxSize(),
                )
                "text" -> {
                    val text = remember(file) { runCatching { file.readText().take(200_000) }.getOrDefault("(could not read)") }
                    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 14.dp, vertical = 10.dp)) {
                        when (entry.ext) {
                            "md", "markdown" -> androidx.compose.foundation.text.selection.SelectionContainer { MarkdownText(text) }
                            "txt", "log", "csv" -> androidx.compose.foundation.text.selection.SelectionContainer {
                                Text(text, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                            }
                            else -> CodeText(text, entry.ext)
                        }
                    }
                }
                else -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Icon(iconFor(entry.kind), null, tint = Muted, modifier = Modifier.size(56.dp))
                    Spacer(Modifier.height(12.dp))
                    Text("${entry.ext.uppercase()} · ${sizeLabel(entry.bytes)}", color = Muted)
                    Spacer(Modifier.height(16.dp))
                    Button(onClick = openWith) { Text("Open with…") }
                }
            }
        }
    }
}
