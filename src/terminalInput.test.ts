import assert from "node:assert/strict";
import test from "node:test";
import { TerminalInputLine } from "./terminalInput.ts";

test("reports an exactly typed slash command on Enter", () => {
  const input = new TerminalInputLine();
  input.write("/clear");
  assert.equal(input.write("\r"), "/clear");
});

test("allows an ordinary backspace correction", () => {
  const input = new TerminalInputLine();
  input.write("/clearx");
  input.write("\x7f");
  assert.equal(input.write("\r"), "/clear");
});

test("refuses to identify a command after cursor editing", () => {
  const input = new TerminalInputLine();
  input.write("/clearX");
  input.write("\x1b[D");
  input.write("\x7f");
  assert.equal(input.write("\r"), null);
});
