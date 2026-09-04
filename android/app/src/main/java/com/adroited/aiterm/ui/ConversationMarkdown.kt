package com.adroited.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * A compact, dependency-free Markdown renderer for agent replies and README-like output.
 * Unsupported syntax remains visible instead of being silently discarded.
 */
@Composable
internal fun ConversationMarkdown(
    text: String,
    color: Color = MaterialTheme.colorScheme.onSurface,
) {
    val blocks = remember(text) { splitConversationBlocks(text) }
    Column {
        var previous: ConversationBlock? = null
        blocks.forEach { block ->
            val gap = gapBefore(previous, block)
            if (gap > 0.dp) Spacer(Modifier.height(gap))
            when (block) {
                is ConversationBlock.Code -> Text(
                    block.text,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 12.sp,
                    lineHeight = 17.sp,
                    color = color,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(6.dp))
                        .horizontalScroll(rememberScrollState())
                        .padding(10.dp),
                    softWrap = false,
                )

                is ConversationBlock.Quote -> Row(Modifier.height(IntrinsicSize.Min)) {
                    Box(
                        Modifier
                            .width(3.dp)
                            .fillMaxHeight()
                            .background(
                                MaterialTheme.colorScheme.outline.copy(alpha = 0.5f),
                                RoundedCornerShape(2.dp),
                            ),
                    )
                    Spacer(Modifier.width(10.dp))
                    Text(
                        conversationInline(block.text),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontStyle = FontStyle.Italic,
                        style = MaterialTheme.typography.bodyMedium,
                        lineHeight = 21.sp,
                    )
                }

                ConversationBlock.Rule -> HorizontalDivider(
                    color = MaterialTheme.colorScheme.outline.copy(alpha = 0.3f),
                )

                is ConversationBlock.Table -> MarkdownTable(block, color)

                is ConversationBlock.ListBlock -> Column(
                    verticalArrangement = Arrangement.spacedBy(3.dp),
                ) {
                    block.items.forEach { item ->
                        Row(Modifier.padding(start = (item.level * 16).dp)) {
                            Text(
                                item.marker,
                                color = color,
                                style = MaterialTheme.typography.bodyMedium,
                                lineHeight = 20.sp,
                                modifier = Modifier.width(if (item.marker.length > 1) 24.dp else 16.dp),
                            )
                            Text(
                                conversationInline(item.text),
                                color = color,
                                style = MaterialTheme.typography.bodyMedium,
                                lineHeight = 20.sp,
                                modifier = Modifier.weight(1f),
                            )
                        }
                    }
                }

                is ConversationBlock.Paragraph -> Text(
                    conversationInline(block.text),
                    color = color,
                    style = MaterialTheme.typography.bodyMedium,
                    fontWeight = if (block.heading > 0) FontWeight.SemiBold else null,
                    fontSize = when (block.heading) {
                        1 -> 20.sp
                        2 -> 18.sp
                        3 -> 16.sp
                        else -> 14.sp
                    },
                    lineHeight = when (block.heading) {
                        1 -> 27.sp
                        2 -> 25.sp
                        3 -> 22.sp
                        else -> 21.sp
                    },
                )
            }
            previous = block
        }
    }
}

private fun gapBefore(previous: ConversationBlock?, current: ConversationBlock): Dp {
    if (previous == null) return 0.dp
    if (current is ConversationBlock.Paragraph && current.heading > 0) {
        return if (current.heading == 1) 18.dp else 14.dp
    }
    if (previous is ConversationBlock.Paragraph && previous.heading > 0) return 6.dp
    if (previous is ConversationBlock.Rule || current is ConversationBlock.Rule) return 12.dp
    if (
        current is ConversationBlock.Code || previous is ConversationBlock.Code ||
        current is ConversationBlock.Table || previous is ConversationBlock.Table
    ) {
        return 10.dp
    }
    if (
        current is ConversationBlock.ListBlock &&
        previous is ConversationBlock.Paragraph &&
        previous.text.trimEnd().endsWith(":")
    ) {
        return 6.dp
    }
    return 10.dp
}

private sealed interface ConversationBlock {
    data class Paragraph(val text: String, val heading: Int = 0) : ConversationBlock
    data class ListBlock(val items: List<ConversationListItem>) : ConversationBlock
    data class Quote(val text: String) : ConversationBlock
    data class Code(val text: String) : ConversationBlock
    data class Table(
        val header: List<String>,
        val rows: List<List<String>>,
        val align: List<TextAlign>,
    ) : ConversationBlock

    data object Rule : ConversationBlock
}

private data class ConversationListItem(
    val level: Int,
    val marker: String,
    val text: String,
)

private val CONVERSATION_BULLET = Regex("^(\\s*)[-*+•]\\s+(.*)")
private val CONVERSATION_NUMBERED = Regex("^(\\s*)(\\d+)[.)]\\s+(.*)")
private val CONVERSATION_TABLE_SEPARATOR = Regex("^\\|?\\s*:?-+:?\\s*(\\|\\s*:?-+:?\\s*)*\\|?$")

private fun conversationListLevel(whitespace: String): Int =
    (whitespace.replace("\t", "    ").length + 3) / 4

private fun conversationTableCells(line: String): List<String> {
    val output = mutableListOf<String>()
    val current = StringBuilder()
    var index = 0
    val source = line.trim()
    while (index < source.length) {
        when {
            source[index] == '\\' && index + 1 < source.length && source[index + 1] == '|' -> {
                current.append('|')
                index++
            }

            source[index] == '|' -> {
                output += current.toString()
                current.clear()
            }

            else -> current.append(source[index])
        }
        index++
    }
    output += current.toString()
    if (source.startsWith("|") && output.isNotEmpty()) output.removeAt(0)
    if (source.endsWith("|") && !source.endsWith("\\|") && output.isNotEmpty()) {
        output.removeAt(output.lastIndex)
    }
    return output.map(String::trim)
}

private fun splitConversationBlocks(text: String): List<ConversationBlock> {
    val output = mutableListOf<ConversationBlock>()
    val paragraph = StringBuilder()
    val list = mutableListOf<ConversationListItem>()
    var code: StringBuilder? = null
    var table: MutableList<String>? = null

    fun flushList() {
        if (list.isNotEmpty()) output += ConversationBlock.ListBlock(list.toList())
        list.clear()
    }

    fun flushParagraph() {
        flushList()
        if (paragraph.isNotBlank()) {
            output += ConversationBlock.Paragraph(paragraph.toString().trimEnd())
        }
        paragraph.clear()
    }

    fun flushTable() {
        val source = table ?: return
        table = null
        val rows = source.map(::conversationTableCells)
        val separatorIndex = rows.indices.firstOrNull { index ->
            index > 0 && CONVERSATION_TABLE_SEPARATOR.matches(source[index].trim())
        }
        if (separatorIndex == null || rows[separatorIndex - 1].isEmpty()) {
            output += ConversationBlock.Code(source.joinToString("\n"))
            return
        }
        val header = rows[separatorIndex - 1]
        val align = rows[separatorIndex].map { cell ->
            val left = cell.startsWith(":")
            val right = cell.endsWith(":")
            when {
                left && right -> TextAlign.Center
                right -> TextAlign.End
                else -> TextAlign.Start
            }
        }
        val width = maxOf(header.size, align.size)
        if (width == 0) {
            output += ConversationBlock.Code(source.joinToString("\n"))
            return
        }
        fun pad(row: List<String>): List<String> =
            if (row.size >= width) row.take(width) else row + List(width - row.size) { "" }

        val body = rows
            .filterIndexed { index, _ -> index != separatorIndex && index != separatorIndex - 1 }
            .map(::pad)
        output += ConversationBlock.Table(
            header = pad(header),
            rows = body,
            align = (align + List(width) { TextAlign.Start }).take(width),
        )
    }

    for (raw in text.lines()) {
        val line = raw.trimEnd()
        if (line.trimStart().startsWith("```")) {
            flushTable()
            if (code == null) {
                flushParagraph()
                code = StringBuilder()
            } else {
                output += ConversationBlock.Code(code.toString().trimEnd())
                code = null
            }
            continue
        }
        if (code != null) {
            code.append(raw).append('\n')
            continue
        }
        if (line.trimStart().startsWith("|")) {
            flushParagraph()
            (table ?: mutableListOf<String>().also { table = it }).add(line.trim())
            continue
        }
        flushTable()
        val heading = Regex("^(#{1,6})\\s+(.*?)\\s*#*$").find(line)
        when {
            heading != null -> {
                flushParagraph()
                output += ConversationBlock.Paragraph(
                    heading.groupValues[2],
                    heading = heading.groupValues[1].length,
                )
            }

            Regex("^\\s*([-*_])\\s*\\1\\s*\\1[-*_\\s]*$").matches(line) -> {
                flushParagraph()
                output += ConversationBlock.Rule
            }

            line.startsWith("> ") || line == ">" -> {
                flushParagraph()
                output += ConversationBlock.Quote(line.removePrefix(">").trimStart())
            }

            line.isBlank() -> if (list.isEmpty()) flushParagraph() else if (paragraph.isNotEmpty()) flushParagraph()

            else -> {
                val bullet = CONVERSATION_BULLET.find(line)
                val numbered = CONVERSATION_NUMBERED.find(line)
                when {
                    bullet != null -> {
                        if (paragraph.isNotEmpty()) flushParagraph()
                        val level = conversationListLevel(bullet.groupValues[1])
                        list += ConversationListItem(
                            level,
                            if (level == 0) "•" else if (level == 1) "◦" else "▪",
                            bullet.groupValues[2],
                        )
                    }

                    numbered != null -> {
                        if (paragraph.isNotEmpty()) flushParagraph()
                        list += ConversationListItem(
                            conversationListLevel(numbered.groupValues[1]),
                            numbered.groupValues[2] + ".",
                            numbered.groupValues[3],
                        )
                    }

                    list.isNotEmpty() && raw.startsWith(" ") && paragraph.isEmpty() -> {
                        val last = list.removeAt(list.lastIndex)
                        list += last.copy(text = last.text + " " + line.trim())
                    }

                    else -> {
                        flushList()
                        if (paragraph.isNotEmpty()) paragraph.append('\n')
                        paragraph.append(line)
                    }
                }
            }
        }
    }
    code?.let { output += ConversationBlock.Code(it.toString().trimEnd()) }
    flushTable()
    flushParagraph()
    return output
}

private val CONVERSATION_CELL_MAX = 220.dp
private val CONVERSATION_CELL_MIN = 56.dp

@Composable
private fun MarkdownTable(table: ConversationBlock.Table, color: Color) {
    val columns = table.header.size
    val rows = listOf(table.header) + table.rows
    val stripe = MaterialTheme.colorScheme.surfaceVariant
    Box(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())) {
        Layout(
            content = {
                rows.forEachIndexed { rowIndex, row ->
                    row.forEachIndexed { columnIndex, cell ->
                        Box(
                            Modifier
                                .background(
                                    when {
                                        rowIndex == 0 -> stripe
                                        rowIndex % 2 == 0 -> stripe.copy(alpha = 0.35f)
                                        else -> Color.Transparent
                                    },
                                )
                                .padding(horizontal = 10.dp, vertical = 7.dp),
                        ) {
                            Text(
                                conversationInline(cell),
                                color = color,
                                style = MaterialTheme.typography.bodySmall,
                                lineHeight = 18.sp,
                                fontWeight = if (rowIndex == 0) FontWeight.SemiBold else null,
                                textAlign = table.align.getOrNull(columnIndex) ?: TextAlign.Start,
                                modifier = Modifier.fillMaxWidth(),
                            )
                        }
                    }
                }
            },
            modifier = Modifier.background(stripe.copy(alpha = 0.25f), RoundedCornerShape(8.dp)),
        ) { measurables, _ ->
            val maximum = CONVERSATION_CELL_MAX.roundToPx()
            val minimum = CONVERSATION_CELL_MIN.roundToPx()
            val widths = IntArray(columns)
            measurables.forEachIndexed { index, measurable ->
                val column = index % columns
                widths[column] = maxOf(
                    widths[column],
                    measurable.maxIntrinsicWidth(Constraints.Infinity),
                )
            }
            for (column in 0 until columns) widths[column] = widths[column].coerceIn(minimum, maximum)
            val heights = IntArray(rows.size)
            measurables.forEachIndexed { index, measurable ->
                val row = index / columns
                heights[row] = maxOf(
                    heights[row],
                    measurable.minIntrinsicHeight(widths[index % columns]),
                )
            }
            val placeables = measurables.mapIndexed { index, measurable ->
                measurable.measure(Constraints.fixed(widths[index % columns], heights[index / columns]))
            }
            layout(widths.sum(), heights.sum()) {
                var y = 0
                for (row in rows.indices) {
                    var x = 0
                    for (column in 0 until columns) {
                        placeables[row * columns + column].place(x, y)
                        x += widths[column]
                    }
                    y += heights[row]
                }
            }
        }
    }
}

private val CONVERSATION_HTML_BREAK = Regex("<br\\s*/?>", RegexOption.IGNORE_CASE)
private val CONVERSATION_HTML_MARK =
    Regex("</?(b|strong|i|em|code|s|del|strike|u|sub|sup)>", RegexOption.IGNORE_CASE)
private val CONVERSATION_HTML_ENTITY = Regex("&(amp|lt|gt|quot|apos|nbsp|#39|#x27);")

internal fun conversationHtmlToMarkdown(text: String): String {
    if ('<' !in text && '&' !in text) return text
    return text
        .replace(CONVERSATION_HTML_BREAK, "\n")
        .replace(CONVERSATION_HTML_MARK) { match ->
            when (match.groupValues[1].lowercase()) {
                "b", "strong" -> "**"
                "i", "em" -> "*"
                "code" -> "`"
                "s", "del", "strike" -> "~~"
                else -> ""
            }
        }
        .replace(CONVERSATION_HTML_ENTITY) { match ->
            when (match.groupValues[1]) {
                "amp" -> "&"
                "lt" -> "<"
                "gt" -> ">"
                "quot" -> "\""
                "apos", "#39", "#x27" -> "'"
                "nbsp" -> "\u00A0"
                else -> match.value
            }
        }
}

private val CONVERSATION_INLINE = Regex(
    "(`[^`]+`)" +
        "|(\\*\\*\\*[^*]+\\*\\*\\*)" +
        "|(\\*\\*[^*]+\\*\\*)" +
        "|(__[^_]+__)" +
        "|(\\*[^*\\s][^*]*\\*)" +
        "|((?<![\\w])_[^_\\s][^_]*_(?![\\w]))" +
        "|(~~[^~]+~~)" +
        "|(\\[[^\\]]+]\\([^)\\s]+\\))" +
        "|(https?://[^\\s<>\"]+)",
)

internal fun conversationMarkdownPlain(text: String): String = conversationHtmlToMarkdown(text)
    .replace(Regex("(?m)^#{1,6}\\s+"), "")
    .replace(Regex("(?m)^\\s*[-*+]\\s+"), "• ")
    .replace(Regex("\\*{1,3}([^*]+)\\*{1,3}"), "$1")
    .replace(Regex("__([^_]+)__"), "$1")
    .replace(Regex("`([^`]+)`"), "$1")
    .replace(Regex("~~([^~]+)~~"), "$1")
    .trim()

@Composable
private fun conversationInline(raw: String): AnnotatedString {
    val codeBackground = MaterialTheme.colorScheme.surfaceVariant
    val linkColor = MaterialTheme.colorScheme.primary
    return buildAnnotatedString {
        val text = conversationHtmlToMarkdown(raw)
        var index = 0
        for (match in CONVERSATION_INLINE.findAll(text)) {
            if (match.range.first < index) continue
            append(text.substring(index, match.range.first))
            val token = match.value
            when {
                token.startsWith("`") -> withStyle(
                    SpanStyle(
                        fontFamily = FontFamily.Monospace,
                        background = codeBackground,
                        fontSize = 13.sp,
                    ),
                ) { append(token.trim('`')) }

                token.startsWith("***") -> withStyle(
                    SpanStyle(fontWeight = FontWeight.Bold, fontStyle = FontStyle.Italic),
                ) { append(token.removeSurrounding("***")) }

                token.startsWith("**") -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) {
                    append(token.removeSurrounding("**"))
                }

                token.startsWith("__") -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) {
                    append(token.removeSurrounding("__"))
                }

                token.startsWith("~~") -> withStyle(
                    SpanStyle(textDecoration = TextDecoration.LineThrough),
                ) { append(token.removeSurrounding("~~")) }

                token.startsWith("[") -> {
                    val label = token.substringAfter('[').substringBefore(']')
                    val url = token.substringAfter('(').substringBeforeLast(')')
                    withLink(
                        LinkAnnotation.Url(
                            url,
                            TextLinkStyles(
                                SpanStyle(
                                    color = linkColor,
                                    textDecoration = TextDecoration.Underline,
                                ),
                            ),
                        ),
                    ) { append(label) }
                }

                token.startsWith("http") -> {
                    val url = token.trimEnd('.', ',', ')', ']', ';')
                    withLink(
                        LinkAnnotation.Url(
                            url,
                            TextLinkStyles(
                                SpanStyle(
                                    color = linkColor,
                                    textDecoration = TextDecoration.Underline,
                                ),
                            ),
                        ),
                    ) { append(url) }
                    if (url.length < token.length) append(token.substring(url.length))
                }

                token.startsWith("_") -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) {
                    append(token.removeSurrounding("_"))
                }

                else -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) {
                    append(token.removeSurrounding("*"))
                }
            }
            index = match.range.last + 1
        }
        append(text.substring(index))
    }
}
