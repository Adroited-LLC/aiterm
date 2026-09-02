import test from "node:test";
import assert from "node:assert/strict";
import {
  SETTINGS_MODAL_MIN_HEIGHT,
  SETTINGS_MODAL_MIN_WIDTH,
  settingsModalSize,
} from "./settingsModalSize.ts";

test("settings modal pointer resizing clamps to its minimum and the viewport", () => {
  assert.deepEqual(
    settingsModalSize({ width: 900, height: 700 }, -1_000, -1_000, { width: 1_920, height: 1_080 }),
    { width: SETTINGS_MODAL_MIN_WIDTH, height: SETTINGS_MODAL_MIN_HEIGHT },
  );
  assert.deepEqual(
    settingsModalSize({ width: 900, height: 700 }, 5_000, 5_000, { width: 1_000, height: 800 }),
    { width: 940, height: 736 },
  );
});
