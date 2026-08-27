import test from "node:test";
import assert from "node:assert/strict";
import { followRekey } from "./selection.ts";

// The reported bug, 2026-08-04: from an opcode session, ← into claude's agents
// screen and picking an agent forked the conversation. The tab re-keyed
// (262513cc -> 9873e983) and the live-row stripe moved, but the click
// selection stayed on the conversation left behind — which wears the loudest
// highlight of the three, so the wrong row read as current.
test("selection follows the tab when it re-keys to a new conversation", () => {
  const after = followRekey(new Set(["262513cc"]), "262513cc", "9873e983");
  assert.deepEqual([...after], ["9873e983"]);
});

test("a selection that never held the old session is left alone", () => {
  // Rows ctrl-clicked for a drag are the user's, not the tab's.
  const picked = new Set(["aaa", "bbb"]);
  assert.deepEqual([...followRekey(picked, "262513cc", "9873e983")], ["aaa", "bbb"]);
});

test("other selected rows survive the move", () => {
  const after = followRekey(new Set(["aaa", "262513cc"]), "262513cc", "9873e983");
  assert.deepEqual([...after].sort(), ["9873e983", "aaa"]);
});

test("it moves rather than adds, so the next drag is not quietly bigger", () => {
  const after = followRekey(new Set(["262513cc"]), "262513cc", "9873e983");
  assert.equal(after.size, 1);
  assert.ok(!after.has("262513cc"));
});

test("a re-key to the same id changes nothing", () => {
  assert.deepEqual([...followRekey(new Set(["a"]), "a", "a")], ["a"]);
});

test("a missing side is not a move", () => {
  // Switching to a tab with no session slot, or from none, must not
  // manufacture a selection out of null.
  assert.deepEqual([...followRekey(new Set(["a"]), null, "b")], ["a"]);
  assert.deepEqual([...followRekey(new Set(["a"]), "a", null)], ["a"]);
});

test("the input set is never mutated", () => {
  const original = new Set(["262513cc"]);
  followRekey(original, "262513cc", "9873e983");
  assert.deepEqual([...original], ["262513cc"]);
});
