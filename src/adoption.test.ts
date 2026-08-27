import test from "node:test";
import assert from "node:assert/strict";
import {
  nextAdoptionDelay, FAST_FOR, FAST_EVERY, SLOW_EVERY, GIVE_UP_AFTER,
} from "./adoption.ts";

test("the measured Codex case is still being watched", () => {
  // The bug this exists for: codex-cli 0.147.0 wrote its rollout 98.9s after
  // the session started, and the old flat 60s deadline had already given up —
  // so the placeholder row could never be collapsed and the conversation kept
  // two rows forever.
  assert.notEqual(nextAdoptionDelay(98_900), null, "must still be looking at 98.9s");
  assert.notEqual(nextAdoptionDelay(60_001), null, "the old deadline is not a deadline now");
});

test("it looks often while a transcript is imminent", () => {
  assert.equal(nextAdoptionDelay(0), FAST_EVERY);
  assert.equal(nextAdoptionDelay(FAST_FOR - 1), FAST_EVERY);
});

test("then it backs off instead of hammering the disk", () => {
  assert.equal(nextAdoptionDelay(FAST_FOR), SLOW_EVERY);
  assert.equal(nextAdoptionDelay(GIVE_UP_AFTER - 1), SLOW_EVERY);
});

test("it does stop eventually", () => {
  // An agent that has written nothing this long is one that does not write
  // transcripts at all. The placeholder stays, which still keeps the tab
  // reachable — the point it was there for.
  assert.equal(nextAdoptionDelay(GIVE_UP_AFTER), null);
  assert.equal(nextAdoptionDelay(GIVE_UP_AFTER * 2), null);
});

test("a clock that jumps backwards does not stop the search", () => {
  assert.equal(nextAdoptionDelay(-5_000), FAST_EVERY);
});
