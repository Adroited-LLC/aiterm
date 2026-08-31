package com.fivelime.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Enough markdown to read an agent's answer or a README: headings by
 *  level, bullets, fenced code, quotes, rules — and inline code, bold,
 *  italics, strikethrough and tappable links. Anything else is shown as
 *  written. */
@Composable
fun MarkdownText(text: String, color: Color = MaterialTheme.colorScheme.onSurface) {
    val blocks = remember(text) { splitBlocks(text) }
    Column {
        blocks.forEach { b ->
            when (b) {
                is Block.Code -> {
                    Text(
                        b.text, fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = color,
                        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp)
                            .background(Bg, RoundedCornerShape(6.dp)).horizontalScroll(rememberScrollState())
                            .padding(8.dp),
                        softWrap = false,
                    )
                }
                is Block.Quote -> Row(Modifier.padding(vertical = 2.dp).height(IntrinsicSize.Min)) {
                    Box(Modifier.width(3.dp).fillMaxHeight().background(Muted.copy(alpha = 0.5f), RoundedCornerShape(2.dp)))
                    Spacer(Modifier.width(8.dp))
                    Text(inline(b.text), color = Muted, fontStyle = FontStyle.Italic, style = MaterialTheme.typography.bodyMedium)
                }
                is Block.Rule -> HorizontalDivider(Modifier.padding(vertical = 6.dp), color = Muted.copy(alpha = 0.3f))
                is Block.Para -> Text(
                    inline(b.text), color = color, style = MaterialTheme.typography.bodyMedium,
                    fontWeight = if (b.heading > 0) FontWeight.SemiBold else null,
                    fontSize = when (b.heading) {
                        1 -> 19.sp
                        2 -> 17.sp
                        3 -> 15.sp
                        else -> 14.sp
                    },
                    modifier = Modifier.padding(vertical = if (b.heading > 0) 4.dp else 2.dp),
                )
            }
        }
    }
}

private sealed class Block {
    data class Para(val text: String, val heading: Int = 0) : Block()
    data class Quote(val text: String) : Block()
    data class Code(val text: String) : Block()
    data object Rule : Block()
}

private fun splitBlocks(text: String): List<Block> {
    val out = mutableListOf<Block>()
    val para = StringBuilder()
    var code: StringBuilder? = null
    var table: StringBuilder? = null
    fun flush() { if (para.isNotBlank()) out += Block.Para(para.toString().trimEnd()); para.clear() }
    fun flushTable() { table?.let { out += Block.Code(it.toString().trimEnd()) }; table = null }
    for (raw in text.lines()) {
        val line = raw.trimEnd()
        if (line.trimStart().startsWith("```")) {
            flushTable()
            if (code == null) { flush(); code = StringBuilder() } else { out += Block.Code(code.toString().trimEnd()); code = null }
            continue
        }
        if (code != null) { code.append(raw).append('\n'); continue }
        // A table row: pipes only align in monospace, so table blocks are
        // shown like code — scrollable sideways, columns intact.
        if (line.trimStart().startsWith("|")) {
            flush()
            if (!Regex("^\\|[-\\s|:]+\\|?$").matches(line.trim())) {
                (table ?: StringBuilder().also { table = it }).append(line.trim()).append('\n')
            }
            continue
        }
        flushTable()
        val h = Regex("^(#{1,6})\\s+(.*)").find(line)
        when {
            h != null -> { flush(); out += Block.Para(h.groupValues[2], heading = h.groupValues[1].length) }
            Regex("^\\s*([-*_])\\s*\\1\\s*\\1[-*_\\s]*$").matches(line) -> { flush(); out += Block.Rule }
            line.startsWith("> ") || line == ">" -> { flush(); out += Block.Quote(line.removePrefix(">").trimStart()) }
            line.isBlank() -> flush()
            else -> {
                val bullet = Regex("^(\\s*)[-*]\\s+(.*)").find(line)
                val num = Regex("^(\\s*)(\\d+)[.)]\\s+(.*)").find(line)
                val shown = when {
                    bullet != null -> bullet.groupValues[1] + "• " + bullet.groupValues[2]
                    num != null -> num.groupValues[1] + num.groupValues[2] + ". " + num.groupValues[3]
                    else -> line
                }
                if (para.isNotEmpty()) para.append('\n')
                para.append(shown)
            }
        }
    }
    code?.let { out += Block.Code(it.toString().trimEnd()) }
    flushTable()
    flush()
    return out
}

/** Inline spans: `code`, ***both***, **bold**, *italic*, ~~gone~~, and
 *  [links](https://…) that open in the browser. */
private val INLINE = Regex(
    "(`[^`]+`)" +
        "|(\\*\\*\\*[^*]+\\*\\*\\*)" +
        "|(\\*\\*[^*]+\\*\\*)" +
        "|(\\*[^*\\s][^*]*\\*)" +
        "|(~~[^~]+~~)" +
        "|(\\[[^\\]]+]\\([^)\\s]+\\))" +
        "|(https?://[^\\s<>\"]+)",
)

private fun inline(s: String): AnnotatedString = buildAnnotatedString {
    var i = 0
    for (m in INLINE.findAll(s)) {
        if (m.range.first < i) continue
        append(s.substring(i, m.range.first))
        val t = m.value
        when {
            t.startsWith("`") -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = Bg, fontSize = 13.sp)) { append(t.trim('`')) }
            t.startsWith("***") -> withStyle(SpanStyle(fontWeight = FontWeight.Bold, fontStyle = FontStyle.Italic)) { append(t.removeSurrounding("***")) }
            t.startsWith("**") -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(t.removeSurrounding("**")) }
            t.startsWith("~~") -> withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) { append(t.removeSurrounding("~~")) }
            t.startsWith("[") -> {
                val label = t.substringAfter('[').substringBefore(']')
                val url = t.substringAfter('(').substringBeforeLast(')')
                withLink(
                    LinkAnnotation.Url(url, TextLinkStyles(SpanStyle(color = Accent, textDecoration = TextDecoration.Underline))),
                ) { append(label) }
            }
            t.startsWith("http") -> {
                val url = t.trimEnd('.', ',', ')', ']', ';')
                withLink(
                    LinkAnnotation.Url(url, TextLinkStyles(SpanStyle(color = Accent, textDecoration = TextDecoration.Underline))),
                ) { append(url) }
                if (url.length < t.length) append(t.substring(url.length))
            }
            else -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(t.removeSurrounding("*")) }
        }
        i = m.range.last + 1
    }
    append(s.substring(i))
}
