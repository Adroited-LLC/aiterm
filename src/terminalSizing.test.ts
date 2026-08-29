import test from "node:test";
import assert from "node:assert/strict";
import { projectTerminalGrid } from "./terminalSizing.ts";

test("a remote focus owner keeps xterm on Rust's canonical grid", () => {
  const projected = projectTerminalGrid(
    "remote",
    { cols: 47, rows: 11 },
    { cols: 118, rows: 36 },
  );

  assert.deepEqual(projected, {
    size: { cols: 47, rows: 11 },
    resizeBackend: false,
  });
});

test("the desktop owner may fit locally and publish the resulting grid", () => {
  const projected = projectTerminalGrid(
    "desktop",
    { cols: 47, rows: 11 },
    { cols: 118, rows: 36 },
  );

  assert.deepEqual(projected, {
    size: { cols: 118, rows: 36 },
    resizeBackend: true,
  });
});
