package com.fivelime.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import coil3.request.ImageRequest
import coil3.svg.SvgDecoder
import com.fivelime.aiterm.SessionState

/** One theme's worth of color. The app reads `Bg`, `Accent` and friends
 *  everywhere; those are state-backed getters below, so switching palettes
 *  recomposes the whole UI in place. */
data class Palette(
    val bg: Color, val surface1: Color, val surface2: Color,
    val accent: Color, val muted: Color, val amber: Color, val green: Color, val red: Color,
    val onSurface: Color, val onSurfaceVariant: Color,
    val primaryContainer: Color, val onPrimaryContainer: Color, val outline: Color,
    val light: Boolean = false,
)

val Palettes: Map<String, Palette> = linkedMapOf(
    "dark" to Palette(
        bg = Color(0xFF0B1220), surface1 = Color(0xFF121B2D), surface2 = Color(0xFF1A2540),
        accent = Color(0xFF7DD3FC), muted = Color(0xFF8A97AD), amber = Color(0xFFF6C453),
        green = Color(0xFF5EE0A0), red = Color(0xFFFF8A80),
        onSurface = Color(0xFFE6EAF2), onSurfaceVariant = Color(0xFFD5DBE7),
        primaryContainer = Color(0xFF1E3A5F), onPrimaryContainer = Color(0xFFE6F4FF),
        outline = Color(0xFF2A3650),
    ),
    "black" to Palette(
        bg = Color(0xFF000000), surface1 = Color(0xFF121212), surface2 = Color(0xFF1C1C1C),
        accent = Color(0xFF7DD3FC), muted = Color(0xFF8F98A6), amber = Color(0xFFF6C453),
        green = Color(0xFF5EE0A0), red = Color(0xFFFF8A80),
        onSurface = Color(0xFFEDEFF3), onSurfaceVariant = Color(0xFFD8DCE3),
        primaryContainer = Color(0xFF16324D), onPrimaryContainer = Color(0xFFE6F4FF),
        outline = Color(0xFF2B2B2B),
    ),
    // The desktop's Nord, translated: same polar-night grounds, frost accent.
    "nord" to Palette(
        bg = Color(0xFF232831), surface1 = Color(0xFF2E3440), surface2 = Color(0xFF3B4252),
        accent = Color(0xFF88C0D0), muted = Color(0xFF9AA4B5), amber = Color(0xFFEBCB8B),
        green = Color(0xFFA3BE8C), red = Color(0xFFBF616A),
        onSurface = Color(0xFFD8DEE9), onSurfaceVariant = Color(0xFFCDD3E0),
        primaryContainer = Color(0xFF434C5E), onPrimaryContainer = Color(0xFFE5E9F0),
        outline = Color(0xFF3B4252),
    ),
    "light" to Palette(
        bg = Color(0xFFF4F6FA), surface1 = Color(0xFFE7ECF4), surface2 = Color(0xFFD9E1EE),
        accent = Color(0xFF0369A1), muted = Color(0xFF5B6B82), amber = Color(0xFFA97B0F),
        green = Color(0xFF1F8A57), red = Color(0xFFC94040),
        onSurface = Color(0xFF17202E), onSurfaceVariant = Color(0xFF334054),
        primaryContainer = Color(0xFFCBE4F7), onPrimaryContainer = Color(0xFF0A2E48),
        outline = Color(0xFFB7C2D4),
        light = true,
    ),
)

private val themeState = androidx.compose.runtime.mutableStateOf(Palettes.getValue("dark"))
fun setPalette(name: String) { themeState.value = Palettes[name] ?: Palettes.getValue("dark") }

val Bg: Color get() = themeState.value.bg
val Surface1: Color get() = themeState.value.surface1
val Surface2: Color get() = themeState.value.surface2
val Accent: Color get() = themeState.value.accent
val Muted: Color get() = themeState.value.muted
val Amber: Color get() = themeState.value.amber
val Green: Color get() = themeState.value.green
val Red: Color get() = themeState.value.red

fun stateColor(s: SessionState): Color = when (s) {
    SessionState.Working -> Amber
    SessionState.NeedsYou -> Red
    SessionState.OnDesktop -> Green
    SessionState.Running -> Accent
    SessionState.Idle -> Muted
}

fun stateLabel(s: SessionState): String? = when (s) {
    SessionState.Working -> "working"
    SessionState.NeedsYou -> "needs you"
    SessionState.OnDesktop -> "on desktop"
    SessionState.Running -> "running"
    SessionState.Idle -> null
}

/** The desktop's brand marks, from its own icon set. Unknown engines get a letter. */
fun brandAsset(id: String): String? = when (id.lowercase()) {
    "claude", "anthropic" -> "claude-color.svg"
    "codex" -> "openai.svg"
    "openai" -> "openai.svg"
    "grok" -> "grok.svg"
    "xai" -> "xai.svg"
    "opencode" -> "opencode.svg"
    "antigravity" -> "antigravity.svg"
    "gemini", "google" -> "gemini-color.svg"
    "anthropic" -> "claude-color.svg"
    "amazon" -> "aws-color.svg"
    "meta", "meta-llama" -> "meta-color.svg"
    "mistralai", "mistral" -> "mistral-color.svg"
    "moonshotai", "moonshot" -> "moonshot.svg"
    "qwen" -> "qwen-color.svg"
    "deepseek" -> "deepseek-color.svg"
    "microsoft" -> "microsoft-color.svg"
    "nvidia" -> "nvidia-color.svg"
    "cohere" -> "cohere-color.svg"
    "perplexity" -> "perplexity-color.svg"
    "minimax" -> "minimax-color.svg"
    "baidu" -> "baidu-color.svg"
    "bytedance", "bytedance-seed" -> "bytedance-color.svg"
    "z-ai", "zhipu" -> "zhipu-color.svg"
    "tencent" -> "tencent-color.svg"
    "ibm-granite", "ibm" -> "ibm.svg"
    "liquid" -> "liquid.svg"
    "nousresearch" -> "nousresearch.svg"
    "upstage" -> "upstage-color.svg"
    "stepfun" -> "stepfun-color.svg"
    "inception" -> "inception.svg"
    // OpenRouter's mark is OpenRouter's only. The chat console ("chat"/"api")
    // used to borrow it, which dressed a LOCAL model's session in another
    // company's logo; a brandless id falls through to the letter avatar.
    "openrouter" -> "openrouter-color.svg"
    else -> null
}

/** Marks drawn in one colour are black in the set; on our dark ground they
 *  need to be white. The colour marks are left alone. */
private fun isMono(asset: String) = !asset.contains("-color")

@Composable
fun AgentIcon(id: String, size: Dp = 28.dp) {
    val asset = brandAsset(id)
    // A mark that cannot be drawn — no asset for the id, or a load that
    // failed — is a lettered avatar, never an empty gap in the row.
    val failed = androidx.compose.runtime.remember(id) { androidx.compose.runtime.mutableStateOf(false) }
    if (asset == null || failed.value) {
        Box(Modifier.size(size).background(agentColor(id).copy(alpha = 0.18f), CircleShape), contentAlignment = Alignment.Center) {
            Text(id.take(1).uppercase(), color = agentColor(id), fontWeight = FontWeight.Bold)
        }
        return
    }
    val ctx = LocalContext.current
    AsyncImage(
        model = ImageRequest.Builder(ctx).data("file:///android_asset/icons/$asset").decoderFactory(SvgDecoder.Factory()).build(),
        contentDescription = id,
        modifier = Modifier.size(size),
        colorFilter = if (isMono(asset)) ColorFilter.tint(themeState.value.onSurface) else null,
        onError = { failed.value = true },
    )
}

@Composable
fun AitermTheme(content: @Composable () -> Unit) {
    val p = themeState.value
    val base = if (p.light) lightColorScheme() else darkColorScheme()
    val scheme = base.copy(
        primary = p.accent,
        onPrimary = p.bg,
        primaryContainer = p.primaryContainer,
        onPrimaryContainer = p.onPrimaryContainer,
        background = p.bg,
        onBackground = p.onSurface,
        surface = p.bg,
        onSurface = p.onSurface,
        surfaceVariant = p.surface1,
        onSurfaceVariant = p.onSurfaceVariant,
        surfaceContainer = p.surface1,
        surfaceContainerHigh = p.surface2,
        outline = p.outline,
        error = p.red,
    )
    MaterialTheme(colorScheme = scheme, content = content)
}

/** One colour per engine, so a row reads at a glance. Unknown engines get grey. */
fun agentColor(agent: String): Color = when (agent.lowercase()) {
    "claude" -> Color(0xFFE8956B)
    "codex" -> Color(0xFF8AB4F8)
    "grok" -> Color(0xFFB0BEC5)
    "opencode" -> Color(0xFFB39DDB)
    // Google's: a blue, like the desktop's var(--blue) for the same mark.
    "antigravity" -> Color(0xFF64B5F6)
    "chat" -> Color(0xFF80CBC4)
    else -> Muted
}

/** The zone absolute dates are written in; null = the phone's own. Set
 *  from settings — someone reading a desktop in another time zone may want
 *  its clock, not theirs. */
var displayZone: java.util.TimeZone? = null

fun relativeTime(lastActive: Long, now: Long = System.currentTimeMillis()): String {
    val ms = if (lastActive > 100_000_000_000L) lastActive else lastActive * 1000
    val d = (now - ms) / 1000
    return when {
        d < 60 -> "just now"
        d < 3600 -> "${d / 60}m ago"
        d < 86_400 -> "${d / 3600}h ago"
        d < 7 * 86_400 -> "${d / 86_400}d ago"
        else -> java.text.SimpleDateFormat("MMM d", java.util.Locale.getDefault())
            .apply { displayZone?.let { timeZone = it } }
            .format(java.util.Date(ms))
    }
}

/** Tap anything that isn't a control and the keyboard goes away — nobody
 *  should need the system back button to put the IME down. */
@Composable
fun Modifier.dismissKeyboardOnTap(): Modifier {
    val focus = androidx.compose.ui.platform.LocalFocusManager.current
    val kb = androidx.compose.ui.platform.LocalSoftwareKeyboardController.current
    return this.then(
        Modifier.pointerInput(Unit) {
            detectTapGestures(onTap = { focus.clearFocus(); kb?.hide() })
        },
    )
}

/** The vendor logo a model id implies: "anthropic/claude-sonnet-5" wears
 *  Claude's mark, "x-ai/grok-4.6" Grok's. No slash → the engine's own. */
fun modelBrand(modelId: String, fallback: String): String {
    val vendor = modelId.substringBefore('/', "").removePrefix("~")
    return vendor.ifEmpty { fallback }
}

fun folderName(path: String): String = path.trimEnd('/').substringAfterLast('/').ifEmpty { path }
