package com.fivelime.aiterm

import com.fivelime.aiterm.ui.modelAsset
import com.fivelime.aiterm.ui.modelBrand
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Which mark a model in the new-session picker wears. */
class ModelBrandTest {
    @Test fun aCliEnginesBareModelWearsTheEnginesMark() {
        assertEquals("claude-color.svg", modelAsset("claude", "opus"))
        assertEquals("claude-color.svg", modelAsset("claude", "fable"))
        assertEquals("openai.svg", modelAsset("codex", "gpt-5.6-sol"))
        assertEquals("grok.svg", modelAsset("grok", "grok-4-fast"))
        assertEquals("gemini-color.svg", modelAsset("antigravity", "gemini-3-pro"))
    }

    @Test fun theRulesKnowTheNamesThatDoNotSayTheirVendor() {
        assertEquals("openai", modelBrand("o3-mini"))
        assertEquals("moonshot", modelBrand("kimi-k2"))
        assertEquals("meta", modelBrand("llama-3.3-70b"))
        assertEquals("mistral", modelBrand("codestral-latest"))
        assertEquals("nousresearch", modelBrand("hermes-4-405b"))
        assertEquals("zhipu", modelBrand("glm-4.5"))
    }

    @Test fun anOpenRouterIdResolvesByRuleThenByVendor() {
        assertEquals("claude-color.svg", modelAsset("api:openrouter", "anthropic/claude-sonnet-4"))
        assertEquals("openai.svg", modelAsset("api:openrouter", "openai/gpt-4o"))
        assertEquals("deepseek-color.svg", modelAsset("api:openrouter", "deepseek/deepseek-r1"))
        assertEquals("grok.svg", modelAsset("api:openrouter", "x-ai/grok-code-fast-1"))
        assertEquals("xai.svg", modelAsset("api:openrouter", "x-ai/some-new-thing"))
        // A vendor the rules never heard of, but the assets have.
        assertEquals("meta-color.svg", modelAsset("api:openrouter", "meta-llama/some-finetune"))
        assertEquals("qwen-color.svg", modelAsset("api:openrouter", "qwen/qwen3-coder"))
    }

    @Test fun anApiModelNobodyKnowsGetsNoMark() {
        assertNull(modelBrand("big-pickle"))
        assertNull(modelAsset("api:openrouter", "big-pickle"))
        assertNull(modelAsset("api:openrouter", "unknownvendor/some-model"))
    }
}
