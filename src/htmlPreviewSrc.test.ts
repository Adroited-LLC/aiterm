import assert from "node:assert/strict";
import { test } from "node:test";
import { htmlPreviewSrc } from "./htmlPreviewSrc.ts";

const fileUrl = (path: string) => `asset://localhost/${encodeURIComponent(path)}`;
const diskPath = (url: URL) => decodeURIComponent(url.pathname.slice(1));

test("HTML siblings and parent resources resolve to their actual files", () => {
  const base = htmlPreviewSrc(fileUrl("/home/matt/project/pages/index.html")) + "?v=2";
  assert.equal(diskPath(new URL(base)), "/home/matt/project/pages/index.html");
  for (const [relative, expected] of [
    ["./app.js", "/home/matt/project/pages/app.js"],
    ["styles/main.css", "/home/matt/project/pages/styles/main.css"],
    ["../images/logo.png", "/home/matt/project/images/logo.png"],
  ]) assert.equal(diskPath(new URL(relative, base)), expected);
});

test("special filename characters remain escaped and round-trip correctly", () => {
  const path = "/home/matt/project #1/100% & café/%2F preview.html";
  const url = new URL(htmlPreviewSrc(fileUrl(path)));
  assert.equal(diskPath(url), path);
  assert.equal(url.hash, "");
  assert.equal(url.search, "");
  assert.equal(diskPath(new URL("image%20%231.png", url)), "/home/matt/project #1/100% & café/image #1.png");
});

test("Windows WSL URLs retain the UNC root and relative resource paths", () => {
  const root = "\\\\wsl.localhost\\Fedora";
  const url = htmlPreviewSrc(`http://asset.localhost/${encodeURIComponent(root)}/home/matt/project/index.html`);
  assert.equal(diskPath(new URL("./app.js", url)), root + "/home/matt/project/app.js");
});
