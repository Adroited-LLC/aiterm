import test from "node:test";
import assert from "node:assert/strict";
import { makePtyExitBuffer } from "./ptyExitBuffer.ts";
import type { PtyExit } from "./ptyExitBuffer.ts";

const exit = (id: number): PtyExit => ({ id, code: 0, signal: null });

test("an exit emitted before ptySpawn resolves is delivered after its id binds", () => {
  const received: PtyExit[] = [];
  const buffer = makePtyExitBuffer((event) => received.push(event));

  buffer.receive(exit(41));
  buffer.bind(41);
  buffer.flush();

  assert.deepEqual(received, [exit(41)]);
});

test("early exits for other PTYs do not hide this PTY's exit", () => {
  const received: PtyExit[] = [];
  const buffer = makePtyExitBuffer((event) => received.push(event));

  buffer.receive(exit(7));
  buffer.receive(exit(42));
  buffer.bind(42);
  buffer.flush();

  assert.deepEqual(received, [exit(42)]);
});

test("a PTY exit is delivered exactly once and never after disposal", () => {
  const received: PtyExit[] = [];
  const buffer = makePtyExitBuffer((event) => received.push(event));

  buffer.bind(9);
  buffer.flush();
  buffer.receive(exit(9));
  buffer.receive(exit(9));
  buffer.dispose();
  buffer.receive(exit(9));

  assert.deepEqual(received, [exit(9)]);
});
