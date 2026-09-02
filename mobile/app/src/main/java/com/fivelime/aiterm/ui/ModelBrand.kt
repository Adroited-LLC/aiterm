package com.fivelime.aiterm.ui

/**
 * Which mark a model wears — the pure lookup, no Compose, so a test can pin it.
 *
 * The rules are the desktop's (`src/brandMap.ts` → `brandForModel`), trimmed
 * to the brands the phone carries in `assets/icons`: LobeHub's keyword rules
 * first, because they know "o3" is OpenAI and "kimi" is Moonshot; then the
 * vendor prefix of a `vendor/model` id, which covers the fine-tuners the rules
 * never heard of; and last, for a CLI engine, the engine's own mark — Claude
 * Code's "opus" is a Claude model even though nothing in the id says so.
 * An API provider's model that resolves to nothing gets null, and the caller
 * draws a letter rather than the wrong company's logo.
 */
private val MODEL_RULES: List<Pair<String, List<String>>> = listOf(
    "openai" to listOf("gpt-3", "gpt-4", "gpt-5", "gpt-oss", "o1-", "^o1", "/o1", "o3-", "^o3", "/o3", "o4-", "^o4", "/o4",
        "text-embedding-", "tts-", "whisper-", "codex", "davinci", "babbage", "omni-moderation", "text-moderation",
        "text-ada", "computer-use", "^gpt-", "/gpt-", "openai"),
    "claude" to listOf("claude", "anthropic"),
    "aws" to listOf("titan"),
    "nousresearch" to listOf("deephermes", "hermes", "genstruct", "minos"),
    "nvidia" to listOf("nemotron", "openreasoning", "nemoretriever", "neva-", "nv-"),
    "meta" to listOf("llama", "/l3"),
    "gemini" to listOf("gemini"),
    "moonshot" to listOf("kimi", "moonshot"),
    "qwen" to listOf("qwen", "qwq", "qvq", "wanx", "wan\\d/", "wan\\d\\.\\d-", "tongyi", "gte-rerank"),
    "minimax" to listOf("minimax", "abab", "^image-"),
    "mistral" to listOf("mistral", "mixtral", "codestral", "mathstral", "/mn-", "pixtral", "ministral", "magistral", "devstral", "voxtral"),
    "perplexity" to listOf("pplx", "sonar"),
    "openrouter" to listOf("^openrouter"),
    "inception" to listOf("^mercury", "/mercury"),
    "cohere" to listOf("command"),
    "stepfun" to listOf("step"),
    "bytedance" to listOf("skylark", "seed-", "bytedance"),
    "microsoft" to listOf("wizardlm", "/phi-", "^phi-", "-phi-", "mai-", "microsoft"),
    "upstage" to listOf("^solar-", "/solar"),
    "grok" to listOf("^grok-", "/grok-"),
    "deepseek" to listOf("deepseek"),
    "liquid" to listOf("liquid", "lfm"),
    "ibm" to listOf("ibm", "granite"),
    // Companies the desktop's set files under a product; the phone's assets
    // are by company.
    "zhipu" to listOf("glm-", "zhipu", "z-ai"),
    "tencent" to listOf("hunyuan"),
    "baidu" to listOf("ernie"),
).map { (brand, keys) -> brand to keys }

private val MODEL_RE: List<Pair<String, List<Regex>>> =
    MODEL_RULES.map { (brand, keys) -> brand to keys.map { Regex(it, RegexOption.IGNORE_CASE) } }

/** The brand name a model id resolves to, or null. */
fun modelBrand(modelId: String): String? {
    val m = modelId.lowercase().trim()
    if (m.isEmpty()) return null
    for ((brand, res) in MODEL_RE) if (res.any { it.containsMatchIn(m) }) return brand
    val slash = m.indexOf('/')
    if (slash > 0) return m.substring(0, slash).removePrefix("~").takeIf { brandAsset(it) != null }
    return null
}

/** The asset file for a model offered by `agentId`, or null for a letter. */
fun modelAsset(agentId: String, modelId: String): String? {
    modelBrand(modelId)?.let { brandAsset(it) }?.let { return it }
    // A CLI engine's models are its own; an API provider's are anyone's.
    return if (agentId.startsWith("api:")) null else brandAsset(agentId)
}
