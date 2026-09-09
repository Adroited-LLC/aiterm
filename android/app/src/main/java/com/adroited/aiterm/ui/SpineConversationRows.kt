package com.adroited.aiterm.ui

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Build
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Language
import androidx.compose.material.icons.filled.Psychology
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.ToolCategory
import com.adroited.aiterm.remote.ToolStatus
import kotlinx.coroutines.withTimeoutOrNull

internal const val MESSAGE_ACTION_HOLD_MILLIS = 800L

/**
 * Observes a deliberate stationary hold without consuming the pointer stream.
 * Native text selection and list scrolling therefore keep their normal Android gestures.
 */
internal fun Modifier.stationaryMessageHold(onHold: () -> Unit): Modifier = pointerInput(onHold) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        val origin = down.position
        val cancelledBeforeTimeout = withTimeoutOrNull(MESSAGE_ACTION_HOLD_MILLIS) {
            while (true) {
                val change = awaitPointerEvent(PointerEventPass.Initial)
                    .changes.firstOrNull { it.id == down.id }
                    ?: return@withTimeoutOrNull true
                if (!change.pressed || (change.position - origin).getDistance() > viewConfiguration.touchSlop) {
                    return@withTimeoutOrNull true
                }
            }
        }
        if (cancelledBeforeTimeout == null) onHold()
    }
}

internal sealed interface SpineTimelineItem {
    val key: String

    data class Row(val item: Item) : SpineTimelineItem {
        override val key: String = item.key
    }

    data class Activity(val items: List<Item>) : SpineTimelineItem {
        override val key: String = "activity:${items.first().key}"
    }
}

/** Keep human and agent turns full-size while folding consecutive machine work into one row. */
internal fun spineTimeline(items: List<Item>): List<SpineTimelineItem> {
    val result = mutableListOf<SpineTimelineItem>()
    val activity = mutableListOf<Item>()
    fun flushActivity() {
        if (activity.isNotEmpty()) result += SpineTimelineItem.Activity(activity.toList())
        activity.clear()
    }
    items.forEach { item ->
        if (item is Item.Tool || (item is Item.AgentText && parseSubagentMessage(item.text) != null)) {
            activity += item
        } else {
            flushActivity()
            result += SpineTimelineItem.Row(item)
        }
    }
    flushActivity()
    return result
}

@Composable
internal fun SpineTimelineRow(row: SpineTimelineItem, onLongPress: (SpineTimelineItem) -> Unit = {}) {
    when (row) {
        is SpineTimelineItem.Row -> SpineItemRow(row.item, onLongPress = { onLongPress(row) })
        is SpineTimelineItem.Activity -> SpineActivityGroup(row, onLongPress)
    }
}

@Composable
private fun SpineItemRow(item: Item, onLongPress: () -> Unit) {
    when (item) {
        is Item.User -> SpineUserBubble(item, onLongPress)
        is Item.AgentText -> SpineAgentBlock(item, onLongPress)
        is Item.Thought -> SpineThoughtBlock(item, onLongPress)
        is Item.Tool -> SpineToolCard(item)
        is Item.TurnEnd -> HorizontalDivider(
            Modifier.padding(vertical = 4.dp),
            color = MaterialTheme.colorScheme.outline.copy(
                alpha = if (item.reason == "completed") 0.14f else 0.32f,
            ),
        )
    }
}

@Composable
@OptIn(ExperimentalFoundationApi::class)
private fun SpineUserBubble(item: Item.User, onLongPress: () -> Unit) {
    val content = remember(item.text) { splitConversationAttachments(item.text) }
    if (content.text.isBlank() && content.imagePaths.isEmpty()) return
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        Column(
            Modifier.widthIn(max = 330.dp)
                .stationaryMessageHold(onLongPress)
                .background(
                    MaterialTheme.colorScheme.primaryContainer,
                    RoundedCornerShape(18.dp, 18.dp, 5.dp, 18.dp),
                )
                .padding(horizontal = 13.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (content.text.isNotBlank()) {
                ConversationMarkdown(content.text, MaterialTheme.colorScheme.onPrimaryContainer)
            }
            content.imagePaths.forEach { path ->
                Text(
                    "Image · ${path.substringAfterLast('/').ifBlank { path }}",
                    color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.82f),
                    fontFamily = FontFamily.Monospace,
                    style = MaterialTheme.typography.labelSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
@OptIn(ExperimentalFoundationApi::class)
private fun SpineAgentBlock(item: Item.AgentText, onLongPress: () -> Unit) {
    val subagent = remember(item.text) { parseSubagentMessage(item.text) }
    if (subagent != null) {
        SpineSubagentCard(item.id, subagent, onLongPress)
        return
    }
    Column(
        Modifier.fillMaxWidth().stationaryMessageHold(onLongPress).padding(end = 14.dp),
    ) {
        AssistantMarkdown(item.text)
        if (!item.done) SpineCaret()
    }
}

@Composable
private fun SpineSubagentCard(id: String, message: SubagentMessage, onLongPress: () -> Unit) {
    var expanded by rememberSaveable(id) { mutableStateOf(false) }
    val hasPayload = message.payload.isNotBlank()
    Column(
        Modifier.fillMaxWidth()
            .stationaryMessageHold(onLongPress)
            .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.42f), RoundedCornerShape(10.dp)),
    ) {
        Row(
            Modifier.fillMaxWidth()
                .then(if (hasPayload) Modifier.clickable { expanded = !expanded } else Modifier)
                .semantics { if (hasPayload) stateDescription = if (expanded) "Expanded" else "Collapsed" }
                .padding(horizontal = 11.dp, vertical = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.Psychology, null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(15.dp))
            Spacer(Modifier.width(7.dp))
            Text(
                message.headline,
                color = MaterialTheme.colorScheme.primary,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.labelMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            if (hasPayload) {
                Text(if (expanded) "⌃" else "⌄", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        if (expanded && hasPayload) {
            HorizontalDivider(color = MaterialTheme.colorScheme.outline.copy(alpha = 0.35f))
            Box(Modifier.padding(horizontal = 11.dp, vertical = 9.dp)) {
                ConversationMarkdown(message.payload)
            }
        }
    }
}

@Composable
private fun SpineCaret() {
    val transition = rememberInfiniteTransition(label = "streaming-caret")
    val alpha by transition.animateFloat(
        initialValue = 0.18f,
        targetValue = 0.95f,
        animationSpec = infiniteRepeatable(tween(650), RepeatMode.Reverse),
        label = "streaming-caret-alpha",
    )
    Box(
        Modifier.padding(top = 3.dp).size(width = 7.dp, height = 14.dp)
            .background(MaterialTheme.colorScheme.primary.copy(alpha = alpha), RoundedCornerShape(2.dp)),
    )
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun SpineThoughtBlock(item: Item.Thought, onLongPress: () -> Unit) {
    var expanded by rememberSaveable(item.id) { mutableStateOf(false) }
    val modifier = Modifier.fillMaxWidth()
        .stationaryMessageHold(onLongPress)
        .clickable { expanded = !expanded }
        .padding(horizontal = 4.dp, vertical = 2.dp)
    if (expanded) {
        Box(modifier) {
            ConversationMarkdown(item.text, MaterialTheme.colorScheme.onSurfaceVariant)
        }
    } else {
        Text(
            conversationMarkdownPlain(item.text).replace('\n', ' '),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontStyle = FontStyle.Italic,
            style = MaterialTheme.typography.bodySmall,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = modifier,
        )
    }
}

@Composable
@OptIn(ExperimentalFoundationApi::class)
private fun SpineActivityGroup(activity: SpineTimelineItem.Activity, onLongPress: (SpineTimelineItem) -> Unit) {
    var expanded by rememberSaveable(activity.key) { mutableStateOf(false) }
    val active = activity.items.count { it is Item.Tool && !it.status.settled }
    Column(
        Modifier.fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.42f), RoundedCornerShape(10.dp)),
    ) {
        Row(
            Modifier.fillMaxWidth()
                .stationaryMessageHold { onLongPress(activity) }
                .clickable { expanded = !expanded }
                .semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" }
                .padding(horizontal = 11.dp, vertical = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.Build, null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(15.dp))
            Spacer(Modifier.width(7.dp))
            Text(
                "Activity · ${activity.items.size} ${if (activity.items.size == 1) "step" else "steps"}",
                color = MaterialTheme.colorScheme.primary,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.labelMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(7.dp))
            if (active > 0) {
                Text(
                    "$active running",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.labelSmall,
                )
                Spacer(Modifier.width(7.dp))
            }
            Text(
                if (expanded) "⌃" else "⌄",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (expanded) {
            HorizontalDivider(color = MaterialTheme.colorScheme.outline.copy(alpha = 0.35f))
            Column(
                Modifier.padding(start = 8.dp, top = 7.dp, end = 7.dp, bottom = 7.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                activity.items.forEach { item ->
                    key(item.key) {
                        SpineItemRow(item, onLongPress = { onLongPress(SpineTimelineItem.Row(item)) })
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun SpineToolCard(item: Item.Tool) {
    var expanded by rememberSaveable(item.id) { mutableStateOf(false) }
    val output = item.output?.takeIf(String::isNotBlank)
    Column(
        Modifier.fillMaxWidth()
            .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.72f), RoundedCornerShape(8.dp))
            .combinedClickable(
                onClick = { if (output != null || item.input.isNotBlank()) expanded = !expanded },
                onLongClick = { expanded = true },
            )
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(toolIcon(item.category), null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(15.dp))
            Spacer(Modifier.width(7.dp))
            Text(
                item.title.ifBlank { item.tool.ifBlank { "Tool" } },
                color = MaterialTheme.colorScheme.primary,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.labelMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(8.dp))
            ToolStatusMark(item.status)
        }
        if (item.input.isNotBlank()) {
            Text(
                item.input,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.labelSmall,
                maxLines = if (expanded) 8 else 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 4.dp),
            )
        }
        if (expanded && output != null) {
            Text(
                output,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(top = 7.dp),
            )
        }
    }
}

private fun toolIcon(category: ToolCategory): ImageVector = when (category) {
    ToolCategory.Read -> Icons.Filled.Description
    ToolCategory.Edit -> Icons.Filled.Edit
    ToolCategory.Execute -> Icons.Filled.Terminal
    ToolCategory.Search -> Icons.Filled.Search
    ToolCategory.Fetch -> Icons.Filled.Language
    ToolCategory.Think -> Icons.Filled.Psychology
    ToolCategory.Other -> Icons.Filled.Build
}

@Composable
private fun ToolStatusMark(status: ToolStatus) {
    when (status) {
        ToolStatus.Pending, ToolStatus.Running -> {
            val transition = rememberInfiniteTransition(label = "tool-status")
            val alpha by transition.animateFloat(
                initialValue = 0.25f,
                targetValue = 1f,
                animationSpec = infiniteRepeatable(tween(700), RepeatMode.Reverse),
                label = "tool-status-alpha",
            )
            StatusDot(MaterialTheme.colorScheme.primary.copy(alpha = alpha))
        }
        ToolStatus.Completed -> Icon(
            Icons.Filled.Check,
            "done",
            tint = MaterialTheme.colorScheme.tertiary,
            modifier = Modifier.size(15.dp),
        )
        ToolStatus.Failed -> Icon(
            Icons.Filled.Close,
            "failed",
            tint = MaterialTheme.colorScheme.error,
            modifier = Modifier.size(15.dp),
        )
        ToolStatus.Cancelled -> StatusDot(MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun StatusDot(color: Color) {
    Box(Modifier.size(8.dp).background(color, RoundedCornerShape(50)))
}
