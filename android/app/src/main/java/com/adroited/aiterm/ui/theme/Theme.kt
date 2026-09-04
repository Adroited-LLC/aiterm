package com.adroited.aiterm.ui.theme

import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import coil3.request.ImageRequest
import coil3.svg.SvgDecoder

/** A complete semantic palette, shared by Material and custom conversation elements. */
data class AitermPalette(
    val background: Color,
    val surface: Color,
    val surfaceRaised: Color,
    val accent: Color,
    val muted: Color,
    val warning: Color,
    val success: Color,
    val error: Color,
    val onSurface: Color,
    val onSurfaceVariant: Color,
    val primaryContainer: Color,
    val onPrimaryContainer: Color,
    val outline: Color,
    val light: Boolean = false,
)

val AitermPalettes: Map<String, AitermPalette> = linkedMapOf(
    "dark" to AitermPalette(
        background = Color(0xFF0B1220), surface = Color(0xFF121B2D), surfaceRaised = Color(0xFF1A2540),
        accent = Color(0xFF7DD3FC), muted = Color(0xFF8A97AD), warning = Color(0xFFF6C453),
        success = Color(0xFF5EE0A0), error = Color(0xFFFF8A80),
        onSurface = Color(0xFFE6EAF2), onSurfaceVariant = Color(0xFFD5DBE7),
        primaryContainer = Color(0xFF1E3A5F), onPrimaryContainer = Color(0xFFE6F4FF),
        outline = Color(0xFF2A3650),
    ),
    "black" to AitermPalette(
        background = Color.Black, surface = Color(0xFF121212), surfaceRaised = Color(0xFF1C1C1C),
        accent = Color(0xFF7DD3FC), muted = Color(0xFF8F98A6), warning = Color(0xFFF6C453),
        success = Color(0xFF5EE0A0), error = Color(0xFFFF8A80),
        onSurface = Color(0xFFEDEFF3), onSurfaceVariant = Color(0xFFD8DCE3),
        primaryContainer = Color(0xFF16324D), onPrimaryContainer = Color(0xFFE6F4FF),
        outline = Color(0xFF2B2B2B),
    ),
    "nord" to AitermPalette(
        background = Color(0xFF232831), surface = Color(0xFF2E3440), surfaceRaised = Color(0xFF3B4252),
        accent = Color(0xFF88C0D0), muted = Color(0xFF9AA4B5), warning = Color(0xFFEBCB8B),
        success = Color(0xFFA3BE8C), error = Color(0xFFBF616A),
        onSurface = Color(0xFFD8DEE9), onSurfaceVariant = Color(0xFFCDD3E0),
        primaryContainer = Color(0xFF434C5E), onPrimaryContainer = Color(0xFFE5E9F0),
        outline = Color(0xFF3B4252),
    ),
    "light" to AitermPalette(
        background = Color(0xFFF4F6FA), surface = Color(0xFFE7ECF4), surfaceRaised = Color(0xFFD9E1EE),
        accent = Color(0xFF0369A1), muted = Color(0xFF5B6B82), warning = Color(0xFFA97B0F),
        success = Color(0xFF1F8A57), error = Color(0xFFC94040),
        onSurface = Color(0xFF17202E), onSurfaceVariant = Color(0xFF334054),
        primaryContainer = Color(0xFFCBE4F7), onPrimaryContainer = Color(0xFF0A2E48),
        outline = Color(0xFFB7C2D4), light = true,
    ),
)

// Null deliberately means "follow the phone" so existing installs retain their current behavior.
private val selectedPaletteName = mutableStateOf<String?>(null)

/** Select a named palette, or pass null to follow the system light/dark setting. */
fun setAitermPalette(name: String?) {
    selectedPaletteName.value = name?.takeIf(AitermPalettes::containsKey)
}

private val BaseTypography = Typography()
private val AitermTypography = Typography(
    headlineMedium = BaseTypography.headlineMedium.copy(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
    ),
    headlineSmall = BaseTypography.headlineSmall.copy(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
    ),
    bodyLarge = BaseTypography.bodyLarge.copy(fontFamily = FontFamily.SansSerif),
    bodyMedium = BaseTypography.bodyMedium.copy(fontFamily = FontFamily.SansSerif),
    labelLarge = BaseTypography.labelLarge.copy(fontFamily = FontFamily.SansSerif),
    labelMedium = BaseTypography.labelMedium.copy(
        fontFamily = FontFamily.Monospace,
        fontWeight = FontWeight.Medium,
    ),
)

@Composable
fun AitermTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    val selected = selectedPaletteName.value
    val palette = selected?.let(AitermPalettes::get)
        ?: AitermPalettes.getValue(if (darkTheme) "dark" else "light")
    val base = if (palette.light) lightColorScheme() else darkColorScheme()
    val colors = base.copy(
        primary = palette.accent,
        onPrimary = palette.background,
        secondary = palette.onSurfaceVariant,
        tertiary = palette.success,
        error = palette.error,
        background = palette.background,
        onBackground = palette.onSurface,
        surface = palette.background,
        onSurface = palette.onSurface,
        surfaceVariant = palette.surface,
        onSurfaceVariant = palette.onSurfaceVariant,
        surfaceContainer = palette.surface,
        surfaceContainerHigh = palette.surfaceRaised,
        primaryContainer = palette.primaryContainer,
        onPrimaryContainer = palette.onPrimaryContainer,
        outline = palette.outline,
    )
    MaterialTheme(colorScheme = colors, typography = AitermTypography, content = content)
}

/** Maps agents and model vendors onto the desktop's bundled brand marks. */
fun brandAsset(id: String): String? = when (id.lowercase()) {
    "claude", "anthropic" -> "claude-color.svg"
    "codex", "openai" -> "openai.svg"
    "grok", "xai" -> "grok.svg"
    "opencode" -> "opencode.svg"
    "antigravity", "gemini", "google" -> "gemini-color.svg"
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
    "openrouter" -> "openrouter-color.svg"
    else -> null
}

private fun isMonochromeBrand(asset: String): Boolean = !asset.contains("-color")

/** A brand mark with a deterministic letter avatar fallback for unknown or failed assets. */
@Composable
fun AgentIcon(
    id: String,
    modifier: Modifier = Modifier,
    size: Dp = 28.dp,
) {
    val normalizedId = id.substringAfterLast('/').ifBlank { id }
    val asset = brandAsset(modelBrand(id, normalizedId))
    var failed by remember(id, asset) { mutableStateOf(false) }
    if (asset == null || failed) {
        val color = agentColor(normalizedId)
        Box(
            modifier.size(size).background(color.copy(alpha = 0.18f), CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(normalizedId.take(1).uppercase(), color = color, fontWeight = FontWeight.Bold)
        }
        return
    }
    val context = LocalContext.current
    AsyncImage(
        model = ImageRequest.Builder(context)
            .data("file:///android_asset/icons/$asset")
            .decoderFactory(SvgDecoder.Factory())
            .build(),
        contentDescription = normalizedId,
        modifier = modifier.size(size),
        colorFilter = if (isMonochromeBrand(asset)) {
            ColorFilter.tint(MaterialTheme.colorScheme.onSurface)
        } else {
            null
        },
        onError = { failed = true },
    )
}

/** One stable accent per built-in engine; unknown engines use a neutral grey. */
fun agentColor(agent: String): Color = when (agent.lowercase()) {
    "claude", "anthropic" -> Color(0xFFE8956B)
    "codex", "openai" -> Color(0xFF8AB4F8)
    "grok", "xai" -> Color(0xFFB0BEC5)
    "opencode" -> Color(0xFFB39DDB)
    "antigravity", "gemini", "google" -> Color(0xFF64B5F6)
    "chat", "api" -> Color(0xFF80CBC4)
    else -> Color(0xFF8A97AD)
}

/** The vendor logo implied by `vendor/model`; no vendor falls back to the engine. */
fun modelBrand(modelId: String, fallback: String): String {
    val vendor = modelId.substringBefore('/', "").removePrefix("~")
    return vendor.ifEmpty { fallback }
}
