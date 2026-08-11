import test from "node:test";
import assert from "node:assert/strict";
import { groupActivity, slugOf, totalOf } from "./activity.ts";
import type { ActivityRow } from "./ipc.ts";

// Run with: npm run test:ui
// `ActivityRow` is a type-only import, so nothing here loads the Tauri bridge
// — Node strips the import along with the annotations.

const row = (r: Partial<ActivityRow>): ActivityRow => ({
  date: "2026-08-10",
  model: "z-ai/glm-5.2",
  provider_name: "Novita",
  requests: 1,
  prompt_tokens: 0,
  completion_tokens: 0,
  usage: 0,
  ...r,
});

test("a host's days are summed into one line", () => {
  const [h] = groupActivity([
    row({ date: "2026-08-09", requests: 2, prompt_tokens: 100, completion_tokens: 20, usage: 0.1 }),
    row({ date: "2026-08-10", requests: 3, prompt_tokens: 300, completion_tokens: 80, usage: 0.21 }),
  ]);
  assert.equal(h.name, "Novita");
  assert.equal(h.requests, 5);
  assert.equal(h.tokens, 500);
  assert.equal(+h.usage.toFixed(2), 0.31);
});

test("each host keeps its own models underneath it", () => {
  const hosts = groupActivity([
    row({ provider_name: "Baidu", model: "a", usage: 0.3 }),
    row({ provider_name: "Baidu", model: "b", usage: 0.1 }),
    row({ provider_name: "Novita", model: "a", usage: 0.05 }),
  ]);
  assert.deepEqual(hosts.map((h) => h.name), ["Baidu", "Novita"]);
  assert.deepEqual(hosts[0].models.map((m) => m.model), ["a", "b"]);
  assert.equal(hosts[0].usage, 0.4);
  assert.deepEqual(hosts[1].models.map((m) => m.model), ["a"]);
});

test("the biggest bill comes first, and free traffic sorts by volume", () => {
  const hosts = groupActivity([
    row({ provider_name: "Cheap", usage: 0.01 }),
    row({ provider_name: "Free A", requests: 4, usage: 0 }),
    row({ provider_name: "Free B", requests: 9, usage: 0 }),
    row({ provider_name: "Dear", usage: 4.2 }),
  ]);
  assert.deepEqual(hosts.map((h) => h.name), ["Dear", "Cheap", "Free B", "Free A"]);
});

// The whole point of the flag: activity says "Baidu", a policy says "baidu".
test("a display name normalises to the slug a policy is keyed by", () => {
  assert.equal(slugOf("Baidu"), "baidu");
  assert.equal(slugOf("Together.ai"), "together-ai");
  assert.equal(slugOf("Amazon Bedrock"), "amazon-bedrock");
  assert.equal(slugOf("Z.AI"), "z-ai");
  // Nothing usable, and nothing that could match a real slug either — but it
  // must not come back with dashes that would match the wrong thing.
  assert.equal(slugOf("!!"), "");
});

test("the window's total is every host added up", () => {
  const t = totalOf(groupActivity([
    row({ provider_name: "A", requests: 2, prompt_tokens: 10, usage: 1.5 }),
    row({ provider_name: "B", requests: 3, completion_tokens: 5, usage: 0.5 }),
  ]));
  assert.deepEqual(t, { requests: 5, tokens: 15, usage: 2 });
});

test("nothing to report is an empty list, not a zero row", () => {
  assert.deepEqual(groupActivity([]), []);
  assert.deepEqual(totalOf([]), { requests: 0, tokens: 0, usage: 0 });
});
