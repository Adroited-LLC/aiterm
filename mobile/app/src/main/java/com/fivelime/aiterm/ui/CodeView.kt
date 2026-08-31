package com.fivelime.aiterm.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.sp

/** A source file, readable: keywords, strings, comments and numbers in
 *  color. Regex-token highlighting — not a parser, and enough. */
@Composable
fun CodeText(text: String, ext: String) {
    val colored = remember(text, ext) { highlight(text, ext) }
    SelectionContainer {
        Text(
            colored, fontFamily = FontFamily.Monospace, fontSize = 12.sp, lineHeight = 17.sp,
            modifier = Modifier.horizontalScroll(rememberScrollState()),
            softWrap = false,
        )
    }
}

private val HASH_COMMENT_EXTS = setOf("py", "sh", "bash", "zsh", "yaml", "yml", "toml", "rb", "env", "ini", "cfg", "conf")
private val C_KEYWORDS = (
    "fun val var class object interface if else when for while return import package private public internal " +
    "override suspend data let const function export default new this null true false async await try catch " +
    "finally throw match impl struct enum trait pub use mut static typeof extends implements interface type " +
    "void int long float double boolean string String remember by lazy init"
).split(' ').toSet()
private val PY_KEYWORDS = (
    "def class if elif else for while return import from as with try except finally raise lambda None True " +
    "False and or not in is pass yield async await global nonlocal print self"
).split(' ').toSet()

private fun highlight(text: String, ext: String): AnnotatedString {
    val hash = ext.lowercase() in HASH_COMMENT_EXTS
    val keywords = if (hash) PY_KEYWORDS else C_KEYWORDS
    val comment = if (hash) "(#.*)" else "(//.*|/\\*[\\s\\S]*?\\*/)"
    val re = Regex(
        comment +
            "|(\"(?:\\\\.|[^\"\\\\\\n])*\"|'(?:\\\\.|[^'\\\\\\n])*'|`(?:\\\\.|[^`\\\\])*`)" +
            "|\\b(\\d+(?:\\.\\d+)?[fLu]?)\\b" +
            "|\\b([A-Za-z_][A-Za-z0-9_]*)\\b",
    )
    // Cap the work: a huge file gets its head highlighted and the rest plain.
    val body = if (text.length > 120_000) text.take(120_000) else text
    return buildAnnotatedString {
        var i = 0
        for (m in re.findAll(body)) {
            if (m.range.first > i) append(body.substring(i, m.range.first))
            val t = m.value
            when {
                m.groups[1] != null -> withStyle(SpanStyle(color = Muted)) { append(t) }
                m.groups[2] != null -> withStyle(SpanStyle(color = Green)) { append(t) }
                m.groups[3] != null -> withStyle(SpanStyle(color = Amber)) { append(t) }
                t in keywords -> withStyle(SpanStyle(color = Accent)) { append(t) }
                else -> append(t)
            }
            i = m.range.last + 1
        }
        if (i < body.length) append(body.substring(i))
        if (body.length < text.length) append("\n… (truncated)")
    }
}
