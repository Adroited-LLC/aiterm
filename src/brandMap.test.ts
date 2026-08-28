import test from "node:test";
import assert from "node:assert/strict";
import {
  BRANDS, agentAccent, agentTint, brandAccent, brandForAgent, brandForModel, brandForName,
  brandForUrl, brandForUsageSource, hasBrand, preferredVariant,
} from "./brandMap.ts";

test("every engine has a mark; the non-vendors do not", () => {
  for (const a of ["claude", "codex", "grok", "opencode"]) assert.ok(brandForAgent(a), a);
  assert.equal(brandForAgent("api"), null);
  assert.equal(brandForAgent("shell"), null);
});

test("a colour mark is preferred where one exists", () => {
  assert.equal(preferredVariant("claude"), "color");
  assert.equal(preferredVariant("openai"), "mono");
  assert.equal(hasBrand("no-such-brand"), false);
  assert.equal(hasBrand(null), false);
});

test("ink is not an accent; the theme steps in for those engines", () => {
  assert.equal(brandAccent("claude"), BRANDS.claude.color);
  assert.equal(brandAccent("openai"), null); // #000
  assert.equal(brandAccent("codex"), null); // #fff
  assert.equal(agentAccent("codex"), "var(--green)");
  assert.equal(agentAccent("grok"), "var(--magenta)");
  assert.equal(agentAccent("claude"), BRANDS.claude.color);
  assert.deepEqual(agentTint("shell"), { className: "" });
  assert.equal(agentTint("claude").className, " branded");
  assert.equal(agentTint(null).className, "");
});

test("model ids, as OpenRouter, a bare host and a CLI name them", () => {
  const cases: [string, string | null][] = [
    ["anthropic/claude-sonnet-4", "claude"],
    ["claude-opus-4-1", "claude"],
    ["openai/gpt-4o", "openai"],
    ["gpt-5", "openai"],
    ["openai/o3-mini", "openai"],
    ["google/gemini-2.5-pro", "gemini"],
    ["x-ai/grok-4", "grok"],
    ["meta-llama/llama-3.3-70b-instruct", "meta"],
    ["mistralai/mistral-large", "mistral"],
    ["deepseek/deepseek-r1", "deepseek"],
    ["qwen/qwen3-235b-a22b", "qwen"],
    ["moonshotai/kimi-k2", "moonshot"],
    ["z-ai/glm-4.5", "zai"],
    ["amazon/nova-pro-v1", "nova"],
    ["openrouter/auto", "openrouter"],
    // An unknown fine-tuner draws nothing rather than a wrong mark.
    ["thedrummer/unslopnemo-12b", null],
    ["", null],
  ];
  for (const [id, want] of cases) assert.equal(brandForModel(id), want, id);
  assert.equal(brandForModel(null), null);
});

test("provider names, as OpenRouter's directory spells them", () => {
  const cases: [string, string | null][] = [
    ["DeepInfra", "deepinfra"],
    ["Moonshot AI", "moonshot"],
    ["Google Vertex", "vertexai"],
    ["Amazon Bedrock", "bedrock"],
    ["xAI", "xai"],
    ["Z.AI", "zai"],
    ["SiliconFlow", "siliconcloud"],
    ["Arcee AI", "arcee"],
    ["Together", "together"],
    ["Chutes", null],
    ["", null],
  ];
  for (const [name, want] of cases) assert.equal(brandForName(name), want, name);
});

test("base urls resolve by host label", () => {
  const cases: [string, string | null][] = [
    ["https://openrouter.ai/api/v1", "openrouter"],
    ["https://api.openai.com/v1", "openai"],
    ["https://api.anthropic.com", "anthropic"],
    ["https://generativelanguage.googleapis.com/v1beta/openai", "gemini"],
    ["https://api.x.ai/v1", "xai"],
    ["https://api.z.ai/api/paas/v4", "zai"],
    ["https://api.groq.com/openai/v1", "groq"],
    ["https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen"],
    ["https://myco.openai.azure.com", "azure"],
    ["https://bedrock-runtime.us-east-1.amazonaws.com", "bedrock"],
    ["http://localhost:11434/v1", "ollama"],
    ["http://192.168.0.53:8080/v1", null],
    ["not a url", null],
  ];
  for (const [url, want] of cases) assert.equal(brandForUrl(url), want, url);
});

test("usage sources", () => {
  assert.equal(brandForUsageSource("anthropic", "Claude"), "claude");
  assert.equal(brandForUsageSource("codex", "Codex"), "codex");
  assert.equal(brandForUsageSource("grok", "Grok"), "grok");
  assert.equal(brandForUsageSource("provider:abc", "OpenRouter"), "openrouter");
});
