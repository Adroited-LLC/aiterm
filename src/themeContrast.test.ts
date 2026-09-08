import test from "node:test";
import assert from "node:assert/strict";
import { contrastRatio, onAccent, readableText } from "./themeContrast.ts";

test("contrast preserves readable colours and lifts quiet text across all surfaces", () => {
  assert.equal(contrastRatio("#000000", "#ffffff"), 21);
  assert.equal(readableText("#d8dae5", ["#16171d"]), "#d8dae5");
  const surfaces = ["#121317", "#16171d", "#1c1e26", "#22242e", "#2a2d39"];
  const lifted = readableText("#5c6070", surfaces);
  for (const surface of surfaces) assert.ok(contrastRatio(lifted, surface) >= 4.5);
});
test("filled accent controls choose readable ink for bright and dark accents", () => {
  for (const colour of ["#ffffff", "#000000", "#e8e8e8", "#268bd2", "#da7756", "#fe8019"]) {
    assert.ok(contrastRatio(colour, onAccent(colour)) >= 4.5, colour);
  }
});
