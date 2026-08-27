import test from "node:test";
import assert from "node:assert/strict";
import { stableOrder } from "./order.ts";

const ids = (list: { id: string }[]) => list.map((s) => s.id);
const rows = (...id: string[]) => id.map((i) => ({ id: i }));

test("a row does not move when its session is merely written to", () => {
  // What the sidebar saw last, and what the next scan returns: identical
  // sessions, reordered because `b` just wrote a line and is now newest.
  const remembered = ["a", "b", "c"];
  const rescanned = rows("b", "a", "c");
  assert.deepEqual(
    ids(stableOrder(rescanned, (s) => s.id, remembered)),
    ["a", "b", "c"],
    "the remembered order wins over the new recency sort",
  );
});

test("a session that did not exist last time arrives at the top", () => {
  // `new` is genuinely new, so recency decides where it lands — that is the
  // one case where the list should change shape.
  const remembered = ["a", "b"];
  const rescanned = rows("new", "b", "a");
  assert.deepEqual(ids(stableOrder(rescanned, (s) => s.id, remembered)), ["new", "a", "b"]);
});

test("several new sessions keep the order they arrived in", () => {
  const remembered = ["a"];
  const rescanned = rows("n2", "n1", "a");
  assert.deepEqual(ids(stableOrder(rescanned, (s) => s.id, remembered)), ["n2", "n1", "a"]);
});

test("a session that disappeared is simply gone", () => {
  const remembered = ["a", "b", "c"];
  const rescanned = rows("c", "a");
  assert.deepEqual(ids(stableOrder(rescanned, (s) => s.id, remembered)), ["a", "c"]);
});

test("nothing remembered yet means the incoming order stands", () => {
  const rescanned = rows("b", "a");
  assert.deepEqual(ids(stableOrder(rescanned, (s) => s.id, undefined)), ["b", "a"]);
  assert.deepEqual(ids(stableOrder(rescanned, (s) => s.id, [])), ["b", "a"]);
});

test("applying it twice changes nothing", () => {
  // It runs during render, so a double render must not shuffle the list.
  const remembered = ["a", "b", "c"];
  const once = stableOrder(rows("b", "c", "a"), (s) => s.id, remembered);
  const twice = stableOrder(once, (s) => s.id, ids(once));
  assert.deepEqual(ids(twice), ids(once));
});
