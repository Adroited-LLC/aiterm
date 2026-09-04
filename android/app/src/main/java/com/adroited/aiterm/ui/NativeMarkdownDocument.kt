package com.adroited.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.adroited.aiterm.remote.RemoteMarkdownBlock
import com.adroited.aiterm.remote.RemoteMarkdownDocument
import com.adroited.aiterm.remote.RemoteMarkdownSpan

/** Native Compose rendering of the bounded document produced by Rust/comrak. */
@Composable
internal fun NativeMarkdownDocument(document: RemoteMarkdownDocument, modifier: Modifier = Modifier) {
    SelectionContainer {
        LazyColumn(
            modifier = modifier,
            contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(11.dp),
        ) {
            itemsIndexed(document.blocks) { _, block -> NativeMarkdownBlock(block) }
        }
    }
}

@Composable
private fun NativeMarkdownBlock(block: RemoteMarkdownBlock) {
    when (block.kind) {
        "heading" -> Text(
            nativeMarkdownSpans(block.spans),
            fontWeight = FontWeight.SemiBold,
            fontSize = when (block.level) { 1 -> 24.sp; 2 -> 20.sp; 3 -> 17.sp; else -> 15.sp },
            lineHeight = when (block.level) { 1 -> 31.sp; 2 -> 27.sp; else -> 23.sp },
        )
        "paragraph" -> Text(nativeMarkdownSpans(block.spans), style = MaterialTheme.typography.bodyMedium, lineHeight = 21.sp)
        "quote" -> Row {
            Box(Modifier.width(3.dp).height(48.dp).background(MaterialTheme.colorScheme.primary.copy(alpha = .55f), RoundedCornerShape(2.dp)))
            Spacer(Modifier.width(10.dp))
            Text(
                nativeMarkdownSpans(block.spans),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontStyle = FontStyle.Italic,
                modifier = Modifier.weight(1f),
            )
        }
        "code" -> Column(
            Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(8.dp)).padding(10.dp),
        ) {
            block.language?.let { Text(it, color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelSmall) }
            Text(
                block.spans.joinToString("") { it.text },
                fontFamily = FontFamily.Monospace,
                fontSize = 12.sp,
                lineHeight = 17.sp,
                softWrap = false,
                modifier = Modifier.horizontalScroll(rememberScrollState()),
            )
        }
        "rule" -> HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        "list_item" -> Row(Modifier.padding(start = (block.depth * 16).dp)) {
            Text(
                when {
                    block.checked == true -> "☑"
                    block.checked == false -> "☐"
                    block.ordered -> "${block.number ?: 1}."
                    else -> "•"
                },
                color = if (block.checked == true) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.width(if (block.ordered) 30.dp else 22.dp),
            )
            Text(nativeMarkdownSpans(block.spans), modifier = Modifier.weight(1f), lineHeight = 21.sp)
        }
        "table" -> NativeMarkdownTable(block)
        else -> Text(block.spans.joinToString("") { it.text })
    }
}

@Composable
private fun NativeMarkdownTable(block: RemoteMarkdownBlock) {
    val columnCount = block.rows.maxOfOrNull { it.cells.size } ?: return
    val surface = MaterialTheme.colorScheme.surfaceVariant
    Column(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).background(surface.copy(alpha = .35f), RoundedCornerShape(8.dp)),
    ) {
        block.rows.forEachIndexed { rowIndex, row ->
            Row(Modifier.background(if (row.header) surface else Color.Transparent)) {
                repeat(columnCount) { column ->
                    Text(
                        nativeMarkdownSpans(row.cells.getOrNull(column).orEmpty()),
                        fontWeight = if (row.header) FontWeight.SemiBold else null,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.width(160.dp).padding(horizontal = 10.dp, vertical = 8.dp),
                    )
                }
            }
            if (rowIndex != block.rows.lastIndex) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = .55f))
        }
    }
}

@Composable
private fun nativeMarkdownSpans(spans: List<RemoteMarkdownSpan>): AnnotatedString {
    val codeBackground = MaterialTheme.colorScheme.surfaceVariant
    val linkColor = MaterialTheme.colorScheme.primary
    return remember(spans, codeBackground, linkColor) {
        buildAnnotatedString {
            spans.forEach { span ->
                val style = SpanStyle(
                    fontWeight = if (span.bold) FontWeight.Bold else null,
                    fontStyle = if (span.italic) FontStyle.Italic else null,
                    textDecoration = if (span.strike) TextDecoration.LineThrough else null,
                    fontFamily = if (span.code) FontFamily.Monospace else null,
                    background = if (span.code) codeBackground else Color.Unspecified,
                )
                val label = if (span.image != null) "🖼 ${span.text.ifBlank { span.image }}" else span.text
                val target = span.href ?: span.image
                if (target != null && (target.startsWith("https://") || target.startsWith("http://"))) {
                    withLink(
                        LinkAnnotation.Url(
                            target,
                            TextLinkStyles(style.merge(SpanStyle(color = linkColor, textDecoration = TextDecoration.Underline))),
                        ),
                    ) { append(label) }
                } else {
                    pushStyle(style)
                    append(label)
                    pop()
                }
            }
        }
    }
}
