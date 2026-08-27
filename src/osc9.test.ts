import test from "node:test";
import assert from "node:assert/strict";
import { parseOsc9 } from "./osc9.ts";

// Run with: npm run test:ui
// Node strips the types; nothing here imports React or xterm, which is the
// reason the parser lives in its own module.

test("a plain body is the message", () => {
  assert.deepEqual(parseOsc9("Claude needs permission to run rm"), {
    kind: "message",
    message: "Claude needs permission to run rm",
  });
});

test("surrounding whitespace is trimmed", () => {
  assert.deepEqual(parseOsc9("  Waiting for input  "), {
    kind: "message",
    message: "Waiting for input",
  });
});

test("an empty body earns no badge", () => {
  assert.equal(parseOsc9(""), null);
  assert.equal(parseOsc9("   "), null);
});

test("9;4 state 1 carries a percentage", () => {
  assert.deepEqual(parseOsc9("4;1;50"), { kind: "progress", progress: { state: 1, pct: 50 } });
});

test("state 0 withdraws progress, which is not the same as 0%", () => {
  assert.deepEqual(parseOsc9("4;0"), { kind: "progress", progress: null });
});

test("indeterminate keeps no number even when one is sent", () => {
  assert.deepEqual(parseOsc9("4;3;40"), { kind: "progress", progress: { state: 3, pct: null } });
});

test("an error state keeps its number", () => {
  assert.deepEqual(parseOsc9("4;2;80"), { kind: "progress", progress: { state: 2, pct: 80 } });
});

test("a percentage above 100 is clamped rather than drawn off the end", () => {
  assert.deepEqual(parseOsc9("4;1;250"), { kind: "progress", progress: { state: 1, pct: 100 } });
});

test("a progress state the protocol never defined is treated as text", () => {
  assert.deepEqual(parseOsc9("4;9"), { kind: "message", message: "4;9" });
});

test("a message that merely starts with a digit is still a message", () => {
  assert.deepEqual(parseOsc9("3 files changed"), {
    kind: "message",
    message: "3 files changed",
  });
});

test("a message beginning '4;' but not progress-shaped stays a message", () => {
  assert.deepEqual(parseOsc9("4; files left"), { kind: "message", message: "4; files left" });
});
