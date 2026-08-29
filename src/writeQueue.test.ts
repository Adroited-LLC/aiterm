import test from "node:test";
import assert from "node:assert/strict";
import { makeWriteQueue } from "./writeQueue.ts";

/** A send that resolves only when the test says so, recording every call. */
function fakeSend() {
  const calls: { id: number; data: string }[] = [];
  const gates: (() => void)[] = [];
  const send = (id: number, data: string) => {
    calls.push({ id, data });
    return new Promise<void>((resolve) => gates.push(resolve));
  };
  return { calls, gates, send, releaseAll: () => gates.splice(0).forEach((g) => g()) };
}

test("only one write per pty is ever in flight", async () => {
  // The reason this exists: two concurrent terminal-write tasks have no guaranteed
  // order on the async runtime, so the queue must never hand over two at once.
  const f = fakeSend();
  const write = makeWriteQueue(f.send);
  write(1, "a");
  write(1, "b");
  write(1, "c");
  assert.equal(f.calls.length, 1, "the second and third wait their turn");
  assert.deepEqual(f.calls[0], { id: 1, data: "a" });
});

test("what was typed during a send goes out next, in order", async () => {
  const f = fakeSend();
  const write = makeWriteQueue(f.send);
  write(1, "a");
  write(1, "b");
  write(1, "c");
  f.gates.shift()!(); // "a" completes
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(f.calls.length, 2);
  assert.equal(f.calls[1].data, "bc", "b and c merged, and kept their order");
});

test("a burst crosses the boundary once", async () => {
  const f = fakeSend();
  const write = makeWriteQueue(f.send);
  for (const ch of "hello") write(2, ch);
  f.gates.shift()!();
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(f.calls.map((c) => c.data), ["h", "ello"]);
});

test("two ptys do not block each other", async () => {
  const f = fakeSend();
  const write = makeWriteQueue(f.send);
  write(1, "x");
  write(2, "y");
  assert.deepEqual(f.calls, [{ id: 1, data: "x" }, { id: 2, data: "y" }]);
});

test("the queue keeps working after a send fails", async () => {
  let attempt = 0;
  const seen: string[] = [];
  const write = makeWriteQueue(async (_id, data) => {
    seen.push(data);
    if (++attempt === 1) throw new Error("pty went away");
  });
  await write(1, "a").catch(() => {});
  await write(1, "b");
  assert.deepEqual(seen, ["a", "b"], "a failure does not wedge the pty forever");
});

test("an empty write is not a round trip", async () => {
  const f = fakeSend();
  const write = makeWriteQueue(f.send);
  await write(1, "");
  assert.equal(f.calls.length, 0);
});

test("equivalent object identities share one ordered stream", async () => {
  type Target = { tabId: string; attachmentId: string };
  const calls: { target: Target; data: string }[] = [];
  const gates: (() => void)[] = [];
  const write = makeWriteQueue<Target>(
    (target, data) => {
      calls.push({ target, data });
      return new Promise<void>((resolve) => gates.push(resolve));
    },
    (target) => `${target.tabId}\0${target.attachmentId}`,
  );

  const first = { tabId: "tab-a", attachmentId: "desktop-a" };
  const second = { tabId: "tab-a", attachmentId: "desktop-a" };
  write(first, "a");
  write(second, "b");
  assert.deepEqual(calls.map(({ data }) => data), ["a"]);

  gates.shift()!();
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(calls.map(({ target, data }) => ({ target, data })), [
    { target: { tabId: "tab-a", attachmentId: "desktop-a" }, data: "a" },
    { target: { tabId: "tab-a", attachmentId: "desktop-a" }, data: "b" },
  ]);
  assert.equal(calls[1].target, second, "the queued write retains the latest full target");
});

test("different attachment identities drain independently", () => {
  type Target = { tabId: string; attachmentId: string };
  const calls: { target: Target; data: string }[] = [];
  const write = makeWriteQueue<Target>(
    (target, data) => {
      calls.push({ target, data });
      return new Promise<void>(() => {});
    },
    (target) => `${target.tabId}\0${target.attachmentId}`,
  );

  write({ tabId: "tab-a", attachmentId: "desktop-a" }, "left");
  write({ tabId: "tab-a", attachmentId: "desktop-b" }, "right");

  assert.deepEqual(calls.map(({ target, data }) => ({ target, data })), [
    { target: { tabId: "tab-a", attachmentId: "desktop-a" }, data: "left" },
    { target: { tabId: "tab-a", attachmentId: "desktop-b" }, data: "right" },
  ]);
});
