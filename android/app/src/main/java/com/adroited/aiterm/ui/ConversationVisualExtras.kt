package com.adroited.aiterm.ui

import android.content.ClipData
import android.content.Intent
import android.widget.Toast
import android.graphics.BitmapFactory
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.Image
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Code
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Image
import androidx.compose.material.icons.filled.InsertDriveFile
import androidx.compose.material.icons.filled.Replay
import androidx.compose.material.icons.filled.SelectAll
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.produceState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.remote.RemoteSessionChange
import com.adroited.aiterm.ui.theme.AgentIcon
import kotlinx.coroutines.launch

internal fun timelineText(row: SpineTimelineItem): String = when (row) {
    is SpineTimelineItem.Row -> when (val item = row.item) {
        is Item.User -> splitConversationAttachments(item.text).text
        is Item.AgentText -> item.text
        is Item.Thought -> item.text
        is Item.Tool -> listOf(item.title.ifBlank { item.tool }, item.input, item.output.orEmpty())
            .filter(String::isNotBlank).joinToString("\n\n")
        is Item.TurnEnd -> ""
    }
    is SpineTimelineItem.Tools -> row.tools.joinToString("\n\n") { tool ->
        listOf(tool.title.ifBlank { tool.tool }, tool.input, tool.output.orEmpty())
            .filter(String::isNotBlank).joinToString("\n")
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ConversationMessageSheet(
    row: SpineTimelineItem,
    promptAbove: String?,
    onDismiss: () -> Unit,
    onEdit: (String) -> Unit,
    onSendAgain: (String) -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    var selecting by remember(row.key) { mutableStateOf(false) }
    val raw = remember(row) { timelineText(row) }
    val plain = remember(raw) { conversationMarkdownPlain(raw) }
    val item = (row as? SpineTimelineItem.Row)?.item

    fun copy(value: String, label: String) {
        clipboard.setText(AnnotatedString(value))
        Toast.makeText(context, "Copied $label", Toast.LENGTH_SHORT).show()
        onDismiss()
    }
    fun share() {
        context.startActivity(
            Intent.createChooser(
                Intent(Intent.ACTION_SEND).apply {
                    type = "text/plain"
                    putExtra(Intent.EXTRA_TEXT, raw)
                },
                null,
            ),
        )
        onDismiss()
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(bottom = 10.dp)) {
            if (selecting) {
                Text("Select text", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(20.dp))
                SelectionContainer {
                    Text(
                        raw,
                        modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
                        fontFamily = if (item is Item.Tool || row is SpineTimelineItem.Tools) FontFamily.Monospace else null,
                    )
                }
                return@Column
            }
            Text(
                plain.replace('\n', ' '),
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
            )
            if (item is Item.User) {
                MessageAction("Edit and send again", Icons.Filled.Edit) { onEdit(raw); onDismiss() }
                MessageAction("Send again", Icons.Filled.Replay) { onSendAgain(raw); onDismiss() }
            }
            if (item is Item.AgentText && promptAbove != null) {
                MessageAction("Ask again", Icons.Filled.Replay) { onSendAgain(promptAbove); onDismiss() }
            }
            MessageAction("Copy text", Icons.Filled.ContentCopy) { copy(plain, "text") }
            MessageAction("Copy as markdown", Icons.Filled.Code) { copy(raw, "markdown") }
            MessageAction("Select text", Icons.Filled.SelectAll) { selecting = true }
            MessageAction("Share", Icons.Filled.Share, ::share)
        }
    }
}

@Composable
private fun MessageAction(label: String, icon: ImageVector, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(label) },
        leadingContent = { Icon(icon, null, tint = MaterialTheme.colorScheme.primary) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier.clickable(onClick = onClick),
    )
}

@Composable
internal fun ConversationCrewStrip(
    current: RemoteSession,
    sessions: List<RemoteSession>,
    broughtIn: Map<String, String>,
    activity: Map<String, String>,
    onSelect: (RemoteSession) -> Unit,
) {
    val parentId = broughtIn[current.id] ?: current.id
    val members = remember(sessions, broughtIn, parentId) {
        (listOf(parentId) + broughtIn.filterValues { it == parentId }.keys)
            .distinct().mapNotNull { id -> sessions.firstOrNull { it.id == id } }
    }
    if (members.size < 2) return
    Column(Modifier.fillMaxWidth()) {
        val attention = members.count { activity[it.id] == "attention" }
        val active = members.count { activity[it.id] == "output" }
        Text(
            buildString {
                append("CREW · ${members.size} AGENTS")
                if (active > 0) append(" · $active WORKING")
            },
            color = MaterialTheme.colorScheme.primary,
            style = MaterialTheme.typography.labelSmall,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 2.dp),
        )
        if (attention > 0) {
            Text(
                "$attention crew member${if (attention == 1) "" else "s"} need${if (attention == 1) "s" else ""} you",
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.labelMedium,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
            )
        }
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 10.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            members.forEach { member ->
                val selected = member.id == current.id
                val state = activity[member.id]
                Row(
                    Modifier.background(
                        if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.16f)
                        else MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.62f),
                        RoundedCornerShape(18.dp),
                    ).clickable(enabled = !selected) { onSelect(member) }
                        .padding(horizontal = 10.dp, vertical = 7.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    AgentIcon(member.agent, size = 15.dp)
                    Spacer(Modifier.width(6.dp))
                    Text(
                        member.title.ifBlank { member.agent.replaceFirstChar(Char::uppercase) },
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.widthIn(max = 160.dp),
                        color = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
                        style = MaterialTheme.typography.labelMedium,
                    )
                    if (state == "attention" || state == "output") {
                        Spacer(Modifier.width(6.dp))
                        Box(
                            Modifier.size(7.dp).background(
                                if (state == "attention") MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
                                CircleShape,
                            ),
                        )
                    }
                }
            }
        }
    }
}

@Composable
internal fun GeneratedFilesRail(
    files: List<RemoteSessionChange>,
    onOpen: (RemoteSessionChange) -> Unit,
    onShowAll: () -> Unit,
    loadThumbnail: suspend (RemoteSessionChange) -> ByteArray?,
) {
    val visible = files.filter { it.kind != "deleted" }.take(8)
    if (visible.isEmpty()) return
    Column(Modifier.fillMaxWidth().padding(top = 2.dp)) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("MADE IN THIS SESSION", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.weight(1f))
            Text("All files", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.clickable(onClick = onShowAll).padding(6.dp))
        }
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(7.dp),
        ) {
            visible.forEach { file ->
                GeneratedFileCard(file, onOpen, loadThumbnail)
            }
        }
    }
}

@Composable
private fun GeneratedFileCard(
    file: RemoteSessionChange,
    onOpen: (RemoteSessionChange) -> Unit,
    loadThumbnail: suspend (RemoteSessionChange) -> ByteArray?,
) {
    val image = file.name.substringAfterLast('.', "").lowercase() in setOf("png", "jpg", "jpeg", "webp", "gif")
    val thumbnail by produceState<ByteArray?>(initialValue = null, key1 = file.path, key2 = file.at) {
        if (image && file.bytes <= 2 * 1024 * 1024) value = loadThumbnail(file)
    }
    val bitmap = remember(thumbnail) { thumbnail?.let { BitmapFactory.decodeByteArray(it, 0, it.size) }?.asImageBitmap() }
    Column(
        Modifier.width(116.dp).background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = .5f), RoundedCornerShape(12.dp))
            .clickable { onOpen(file) }.padding(8.dp),
    ) {
        if (bitmap != null) {
            Image(bitmap!!, file.name, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxWidth().height(64.dp).background(MaterialTheme.colorScheme.surface))
        } else {
            Icon(if (image) Icons.Filled.Image else Icons.Filled.InsertDriveFile, null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(26.dp))
        }
        Spacer(Modifier.height(7.dp))
        Text(file.name, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Medium)
        Text(sessionFileSizeLabel(file.bytes), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
internal fun NeedsYouQuickKeys(onKey: (String) -> Unit) {
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 5.dp),
        horizontalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        listOf("Esc" to "\u001b", "Enter" to "\r", "y" to "y", "n" to "n").forEach { (label, value) ->
            Text(
                label,
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.background(MaterialTheme.colorScheme.primary.copy(alpha = .12f), RoundedCornerShape(9.dp))
                    .clickable { onKey(value) }.padding(horizontal = 13.dp, vertical = 8.dp),
            )
        }
    }
}

@Composable
internal fun conversationScrollIndicator(state: LazyListState): Modifier {
    val alpha by animateFloatAsState(
        targetValue = if (state.isScrollInProgress) .72f else 0f,
        animationSpec = tween(if (state.isScrollInProgress) 80 else 650),
        label = "conversation-scrollbar",
    )
    val color = MaterialTheme.colorScheme.primary
    return Modifier.drawWithContent {
        drawContent()
        val info = state.layoutInfo
        if (alpha <= 0f || info.totalItemsCount == 0) return@drawWithContent
        val visible = info.visibleItemsInfo.size.coerceAtLeast(1)
        val fraction = (visible.toFloat() / info.totalItemsCount).coerceIn(.08f, 1f)
        val height = size.height * fraction
        val first = info.visibleItemsInfo.firstOrNull()?.index ?: 0
        val top = (size.height - height) * (first.toFloat() / (info.totalItemsCount - visible).coerceAtLeast(1))
        drawRoundRect(color.copy(alpha = alpha), topLeft = Offset(size.width - 3.dp.toPx(), top),
            size = androidx.compose.ui.geometry.Size(2.dp.toPx(), height), cornerRadius = CornerRadius(2.dp.toPx()))
    }
}
