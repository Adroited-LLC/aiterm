import test from "node:test";
import assert from "node:assert/strict";
import {
  applyTabRegistryEvent, createTabExitCatchUp, reconcileTabs,
} from "./tabModel.ts";

test("remote open and requested close reconcile the desktop roster without an ended tab", () => {
  let projection = {
    revision: 0 as number | null,
    tabs: [] as Array<{ id: string; title: string }>,
  };
  let applied = applyTabRegistryEvent(projection, {
    change: "opened",
    revision: 1,
    tabId: "tab-phone",
    tab: { id: "tab-phone", title: "Phone tab" },
  });
  assert.equal(applied.needsSnapshot, false);
  assert.deepEqual(applied.projection.tabs, [{ id: "tab-phone", title: "Phone tab" }]);

  projection = applied.projection;
  applied = applyTabRegistryEvent(projection, {
    change: "removed",
    revision: 2,
    tabId: "tab-phone",
    requested: true,
  });
  assert.equal(applied.needsSnapshot, false);
  assert.deepEqual(applied.projection.tabs, []);
  assert.deepEqual(applied.removed, { tabId: "tab-phone", requested: true });
});

test("a desktop registry revision gap requests snapshot recovery without applying partial state", () => {
  const before = {
    revision: 4 as number | null,
    tabs: [{ id: "tab-a", title: "before", local: "keep" }],
  };
  const applied = applyTabRegistryEvent(before, {
    change: "changed",
    revision: 6,
    tabId: "tab-a",
    tab: { id: "tab-a", title: "after" },
  });

  assert.equal(applied.needsSnapshot, true);
  assert.deepEqual(applied.projection, before);
});

test("Rust descriptors replace metadata without changing active tab identity", () => {
  const before = [{ id: "tab-a", title: "repo", slotId: "old" }];
  const after = reconcileTabs(before, [{ id: "tab-a", title: "repo", slotId: "new" }]);

  assert.equal(after[0].id, "tab-a");
  assert.equal(after[0].slotId, "new");
});

test("a closed Rust tab is removed from the renderer projection", () => {
  assert.deepEqual(
    reconcileTabs(
      [{ id: "tab-a", title: "a", slotId: "a" }, { id: "tab-b", title: "b", slotId: "b" }],
      [{ id: "tab-b", title: "b", slotId: "b" }],
    ),
    [{ id: "tab-b", title: "b", slotId: "b" }],
  );
});

test("an exit before the component listener is recovered from authoritative state once", () => {
  const ended: Array<[string, number | null, string | null]> = [];
  const catchUp = createTabExitCatchUp("tab-a", (...exit) => ended.push(exit));

  catchUp.reconcile([{
    id: "tab-a",
    state: "exited",
    exit: { code: 23, signal: null },
  }]);
  catchUp.reconcile([{
    id: "tab-a",
    state: "exited",
    exit: { code: 23, signal: null },
  }]);

  assert.deepEqual(ended, [["tab-a", 23, null]]);
});

test("an exit between listener setup and rejected attach is not duplicated by catch-up", () => {
  const ended: Array<[string, number | null, string | null]> = [];
  const catchUp = createTabExitCatchUp("tab-a", (...exit) => ended.push(exit));

  catchUp.event({ tabId: "tab-a", code: null, signal: "Killed" });
  catchUp.reconcile([{
    id: "tab-a",
    state: "exited",
    exit: { code: null, signal: "Killed" },
  }]);

  assert.deepEqual(ended, [["tab-a", null, "Killed"]]);
});

test("a prepare-exit attach rejection stays subscribed for the eventual final exit", () => {
  const ended: Array<[string, number | null, string | null]> = [];
  const catchUp = createTabExitCatchUp("tab-a", (...exit) => ended.push(exit));

  catchUp.reconcile([{ id: "tab-a", state: "running" }]);
  assert.deepEqual(ended, []);

  catchUp.event({ tabId: "tab-a", code: 17, signal: "Terminated" });
  catchUp.reconcile([{
    id: "tab-a",
    state: "exited",
    exit: { code: 17, signal: "Terminated" },
  }]);

  assert.deepEqual(ended, [["tab-a", 17, "Terminated"]]);
});

test("disposed and foreign-tab callbacks cannot end a replacement tab", () => {
  const ended: Array<[string, number | null, string | null]> = [];
  const catchUp = createTabExitCatchUp("tab-old", (...exit) => ended.push(exit));

  catchUp.event({ tabId: "tab-replacement", code: 9, signal: null });
  catchUp.dispose();
  catchUp.event({ tabId: "tab-old", code: 9, signal: null });
  catchUp.reconcile([{
    id: "tab-old",
    state: "exited",
    exit: { code: 9, signal: null },
  }]);

  assert.deepEqual(ended, []);
});
