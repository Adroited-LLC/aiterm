import assert from "node:assert/strict";
import test from "node:test";
import { attachmentPaths } from "./fileAttachments.ts";

test("escapes spaces and shell syntax without submitting the prompt", () => {
  assert.deepEqual(attachmentPaths(["/home/matt/report final.pdf", "/mnt/c/Users/John/a$(whoami);.txt"]),
    ["/home/matt/report\\ final.pdf", "/mnt/c/Users/John/a\\$\\(whoami\\)\\;.txt"]);
});
test("deduplicates selected files", () => {
  assert.deepEqual(attachmentPaths(["/tmp/log.txt", "/tmp/log.txt"]), ["/tmp/log.txt"]);
});
test("rejects filenames that could inject terminal controls or Enter", () => {
  for (const path of ["/tmp/a\ncommand", "/tmp/a\rcommand", "/tmp/a\x1b[201~", "/tmp/a\0", ""]) {
    assert.throws(() => attachmentPaths([path]));
  }
});
