import test from "node:test";
import assert from "node:assert/strict";
import { reconcileTabs } from "./tabModel.ts";

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
