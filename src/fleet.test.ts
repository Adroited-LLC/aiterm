import test from "node:test";
import assert from "node:assert/strict";
import { buildFleet, elapsed, runningDetail } from "./fleet.ts";
import type { Session, SpineOverview } from "./ipc.ts";

// Run with: npm run test:ui
// Types only from `ipc`, so nothing here loads the Tauri bridge.

const session = (s: Partial<Session>): Session => ({
  id: "s1",
  agent: "claude",
  title: "A session",
  project_path: "/home/j/proj",
  group_path: "/home/j/proj",
  branch: null,
  forked: false,
  background: false,
  fork_parent: null,
  last_active: 1000,
  ...s,
});

const ov = (o: Partial<SpineOverview>): SpineOverview => ({
  session_id: "s1",
  agent: "claude",
  phase: "idle",
  detail: "",
  turn_open: false,
  turn_started_ts: null,
  last_text: null,
  last_tool: null,
  ...o,
});

const empty = { live: new Set<string>(), attention: new Set<string>(), busy: new Set<string>() };

test("the spine's phase decides the group", () => {
  const sessions = [
    session({ id: "a", last_active: 3 }),
    session({ id: "b", last_active: 2 }),
    session({ id: "c", last_active: 1 }),
  ];
  const f = buildFleet({
    sessions,
    overview: new Map([
      ["a", ov({ session_id: "a", phase: "working" })],
      ["b", ov({ session_id: "b", phase: "needs_you", detail: "permission: Edit foo.rs" })],
    ]),
    ...empty,
  });
  assert.deepEqual(f.needsYou.map((r) => r.session.id), ["b"]);
  assert.deepEqual(f.running.map((r) => r.session.id), ["a"]);
  assert.deepEqual(f.recent.flatMap((g) => g.rows.map((r) => r.session.id)), ["c"]);
  assert.equal(f.needsYou[0].detail, "permission: Edit foo.rs");
});

test("with no spine log the tabs decide, so the board is never empty", () => {
  const f = buildFleet({
    sessions: [session({ id: "a" }), session({ id: "b" }), session({ id: "c" })],
    overview: new Map(),
    live: new Set(["a"]),
    attention: new Set(["a"]),
    busy: new Set(["b"]),
  });
  assert.deepEqual(f.needsYou.map((r) => r.session.id), ["a"]);
  assert.deepEqual(f.running.map((r) => r.session.id), ["b"]);
  assert.equal(f.needsYou[0].tailed, false);
  assert.equal(f.needsYou[0].live, true);
  assert.equal(f.recent[0].rows[0].session.id, "c");
});

test("the spine outranks the tabs when it has an opinion", () => {
  const f = buildFleet({
    sessions: [session({ id: "a" })],
    overview: new Map([["a", ov({ session_id: "a", phase: "idle" })]]),
    ...empty,
    attention: new Set(["a"]),
  });
  assert.equal(f.needsYou.length, 0);
  assert.equal(f.idleTotal, 1);
});

test("recent groups by project, newest project first, and caps", () => {
  const sessions = [
    session({ id: "a", last_active: 5, group_path: "/p/one" }),
    session({ id: "b", last_active: 4, group_path: "/p/two" }),
    session({ id: "c", last_active: 3, group_path: "/p/one" }),
    session({ id: "d", last_active: 2, group_path: "/p/two" }),
  ];
  const f = buildFleet({ sessions, overview: new Map(), ...empty, cap: 3 });
  assert.deepEqual(f.recent.map((g) => g.path), ["/p/one", "/p/two"]);
  assert.deepEqual(f.recent[0].rows.map((r) => r.session.id), ["a", "c"]);
  assert.deepEqual(f.recent[1].rows.map((r) => r.session.id), ["b"]);
  assert.equal(f.idleTotal, 4);
  assert.equal(f.idleShown, 3);
});

test("a running row prefers the tool it is in, then its last line", () => {
  assert.equal(
    runningDetail(ov({ last_tool: { title: "Bash npm test", status: "running" }, last_text: "one moment" })),
    "Bash npm test",
  );
  assert.equal(
    runningDetail(ov({ last_tool: { title: "Read foo.rs", status: "completed" }, last_text: "one moment" })),
    "one moment",
  );
  assert.equal(runningDetail(ov({ detail: "running Bash" })), "running Bash");
  assert.equal(runningDetail(ov({})), "");
});

test("elapsed counts only from an open turn", () => {
  assert.equal(elapsed(null, 10_000), "");
  assert.equal(elapsed(9_000, 10_000), "1s");
  assert.equal(elapsed(10_000 - 62_000, 10_000), "1m 02s");
  assert.equal(elapsed(10_000 - 3_723_000, 10_000), "1h 02m");
});
