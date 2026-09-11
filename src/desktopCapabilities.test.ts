import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

// Browser previews mock native IPC, so they cannot catch denied dialog calls.
// Pairing export/import must be authorized in every shipped desktop shell.
for (const shell of ["src-tauri", "windows"]) {
  test(`${shell} main window authorizes native pairing-file save and open`, () => {
    const capability = JSON.parse(readFileSync(new URL(`../${shell}/capabilities/default.json`, import.meta.url), "utf8"));
    assert.ok(capability.windows.includes("main"));
    const permissions = capability.permissions.map((p: string | { identifier: string }) => typeof p === "string" ? p : p.identifier);
    for (const command of ["dialog:allow-open", "dialog:allow-save"]) {
      assert.ok(permissions.includes(command), `${shell} denies ${command}; the native pairing file dialog cannot open`);
    }
  });
}
