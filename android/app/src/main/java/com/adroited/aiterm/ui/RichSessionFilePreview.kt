package com.adroited.aiterm.ui

import android.content.ClipData
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color as AndroidColor
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import android.webkit.MimeTypeMap
import android.widget.MediaController
import android.widget.Toast
import android.widget.VideoView
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.OpenInNew
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.FileProvider
import com.adroited.aiterm.remote.RemoteSessionChange
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@Composable
internal fun RichSessionFilePreviewBody(
    target: RemoteSessionChange,
    loading: Boolean,
    preview: RemoteSessionFilePreview?,
    error: String?,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val cached = remember(preview) {
        preview?.let {
            val extension = target.name.substringAfterLast('.', "bin").replace(Regex("[^a-zA-Z0-9]"), "")
            val dir = File(context.cacheDir, "session-previews").apply { mkdirs() }
            File(dir, "${target.path.hashCode()}.$extension").apply { writeBytes(it.data) }
        }
    }
    DisposableEffect(cached) { onDispose { cached?.delete() } }
    val mime = preview?.mime?.takeIf { it.isNotBlank() }
        ?: MimeTypeMap.getSingleton().getMimeTypeFromExtension(target.name.substringAfterLast('.', ""))
        ?: "application/octet-stream"

    fun uri() = cached?.let { FileProvider.getUriForFile(context, "${context.packageName}.terminal-images", it) }
    fun open() = uri()?.let {
        context.startActivity(Intent.createChooser(Intent(Intent.ACTION_VIEW).setDataAndType(it, mime)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), target.name))
    }
    fun share() = uri()?.let {
        context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType(mime)
            .putExtra(Intent.EXTRA_STREAM, it).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), target.name))
    }
    fun copy() = uri()?.let {
        context.getSystemService(android.content.ClipboardManager::class.java)
            .setPrimaryClip(ClipData.newUri(context.contentResolver, target.name, it))
        Toast.makeText(context, "Copied file", Toast.LENGTH_SHORT).show()
    }

    Column(modifier) {
        if (preview != null && cached != null && !preview.truncated) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                IconButton(onClick = ::copy) { Icon(Icons.Filled.ContentCopy, "Copy file") }
                IconButton(onClick = ::share) { Icon(Icons.Filled.Share, "Share") }
                IconButton(onClick = ::open) { Icon(Icons.Filled.OpenInNew, "Open with") }
            }
        }
        Box(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
            when {
                loading -> CircularProgressIndicator()
                error != null -> Text(error, color = MaterialTheme.colorScheme.error)
                preview == null -> Text("No preview available", color = MaterialTheme.colorScheme.onSurfaceVariant)
                preview.truncated -> Text("This file is larger than the 8 MB phone preview limit. Open it from the desktop for the complete file.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                mime.startsWith("image/") -> ZoomableImage(preview.data, target.name)
                mime == "application/pdf" || target.name.endsWith(".pdf", true) -> PdfPreview(cached!!)
                mime.startsWith("video/") || mime.startsWith("audio/") -> MediaPreview(cached!!)
                mime.startsWith("text/") || target.name.substringAfterLast('.', "").lowercase() in textExtensions -> {
                    val text = preview.data.toString(Charsets.UTF_8)
                    SelectionContainer {
                        Text(
                            highlightedCode(text, target.name.substringAfterLast('.', "")),
                            fontFamily = FontFamily.Monospace,
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(10.dp),
                        )
                    }
                }
                else -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(target.name, fontWeight = FontWeight.SemiBold)
                    Spacer(Modifier.height(8.dp))
                    Text("Use Open with to view this file.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

private val textExtensions = setOf("txt", "md", "markdown", "log", "csv", "json", "toml", "yaml", "yml", "xml", "kt", "kts", "java", "rs", "go", "py", "js", "jsx", "ts", "tsx", "html", "css", "sh", "sql")

@Composable
private fun ZoomableImage(data: ByteArray, label: String) {
    val bitmap = remember(data) { BitmapFactory.decodeByteArray(data, 0, data.size)?.asImageBitmap() }
    var scale by remember(data) { mutableStateOf(1f) }
    var offset by remember(data) { mutableStateOf(Offset.Zero) }
    if (bitmap == null) { Text("Could not decode image"); return }
    Image(
        bitmap = bitmap,
        contentDescription = label,
        contentScale = ContentScale.Fit,
        modifier = Modifier.fillMaxSize()
            .pointerInput(data) { detectTapGestures(onDoubleTap = { scale = if (scale > 1f) 1f else 2.5f; if (scale == 1f) offset = Offset.Zero }) }
            .pointerInput(data) { detectTransformGestures { _, pan, zoom, _ -> scale = (scale * zoom).coerceIn(1f, 8f); offset = if (scale > 1f) offset + pan else Offset.Zero } }
            .graphicsLayer { scaleX = scale; scaleY = scale; translationX = offset.x; translationY = offset.y },
    )
}

@Composable
private fun MediaPreview(file: File) {
    AndroidView(
        factory = { context -> VideoView(context).apply {
            setMediaController(MediaController(context).also { it.setAnchorView(this) })
            setVideoPath(file.absolutePath)
            setOnPreparedListener { start() }
        } },
        modifier = Modifier.fillMaxSize(),
    )
}

@Composable
private fun PdfPreview(file: File) {
    val renderer = remember(file) { runCatching { PdfRenderer(ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)) }.getOrNull() }
    DisposableEffect(renderer) { onDispose { renderer?.close() } }
    if (renderer == null) { Text("Could not open PDF", color = MaterialTheme.colorScheme.error); return }
    val width = with(LocalDensity.current) { LocalConfiguration.current.screenWidthDp.dp.roundToPx() }.coerceAtMost(1800)
    val pages = remember(renderer) { (0 until renderer.pageCount).toList() }
    val lock = remember(renderer) { Any() }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        items(pages, key = { it }) { index -> PdfPage(renderer, index, width, lock) }
    }
}

@Composable
private fun PdfPage(renderer: PdfRenderer, index: Int, width: Int, lock: Any) {
    var bitmap by remember(renderer, index) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(renderer, index, width) {
        bitmap = withContext(Dispatchers.IO) { synchronized(lock) {
            renderer.openPage(index).use { page ->
                val height = (width * page.height.toFloat() / page.width).toInt().coerceAtLeast(1)
                Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888).also { result ->
                    result.eraseColor(AndroidColor.WHITE)
                    page.render(result, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                }
            }
        } }
    }
    bitmap?.let { Image(it.asImageBitmap(), "Page ${index + 1}", modifier = Modifier.fillMaxWidth(), contentScale = ContentScale.FillWidth) }
        ?: Box(Modifier.fillMaxWidth().height(360.dp).background(MaterialTheme.colorScheme.surfaceVariant), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp)) }
}

@Composable
private fun highlightedCode(text: String, extension: String): AnnotatedString {
    val keyword = MaterialTheme.colorScheme.primary
    val stringColor = MaterialTheme.colorScheme.tertiary
    return remember(text, extension, keyword, stringColor) {
        buildAnnotatedString {
            append(text)
            Regex("\\b(class|fun|val|var|if|else|when|for|while|return|import|package|struct|enum|fn|let|const|async|await|true|false|null)\\b")
                .findAll(text).forEach { addStyle(SpanStyle(color = keyword, fontWeight = FontWeight.SemiBold), it.range.first, it.range.last + 1) }
            Regex("\"(?:\\\\.|[^\"\\\\])*\"").findAll(text)
                .forEach { addStyle(SpanStyle(color = stringColor), it.range.first, it.range.last + 1) }
        }
    }
}
