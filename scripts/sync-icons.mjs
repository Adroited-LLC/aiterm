// Bring the LobeHub brand set (https://lobehub.com/icons) into the app.
//
//   node scripts/sync-icons.mjs          # svg + metadata (what the app uses)
//   node scripts/sync-icons.mjs --png    # also the light/dark PNG sets
//
// SVG marks come from @lobehub/icons-static-svg: the mono one (`<name>.svg`,
// drawn in currentColor so it takes the theme's text colour — that is the
// light/dark handling) and the colour one (`<name>-color.svg`) where the brand
// has one. Both go to src/assets/icons/all, so the app ships its own copies and
// the build never reaches into node_modules for them. Wordmarks (`-text`,
// `-brand`) are left out: nothing here shows them.
//
// A handful go to src/assets/icons/hot as well: the engines aiterm launches and
// the marks on screen from the first paint. `src/brand.ts` loads `hot` eagerly
// and everything else on demand, so a session list full of Claude icons draws
// at once while the 300th OpenRouter vendor costs nothing until a row with it
// is on screen.
//
// Metadata comes from @lobehub/icons (the React package — a separate version
// line from the static one, pinned below), fetched from jsdelivr rather than
// installed: it drags antd in for the sake of two data files:
//   brands.json  — from its `toc`: title, primary colour, group, which variants
//                  exist, for every brand.
//   models.json  — its `modelMappings`: the keyword → brand table behind
//                  `<ModelIcon model="gpt-4o">`, so a model id resolves here
//                  exactly as it does on lobehub.com.
//
// --png copies @lobehub/icons-static-png's light/ and dark/ sets into
// src/assets/icons/png. The app does not read them (SVG + currentColor covers
// both modes); they are for anything that cannot run CSS.
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const svgPkg = join(root, "node_modules", "@lobehub", "icons-static-svg");
const out = join(root, "src", "assets", "icons");

/** On screen before anything is clicked: engine tabs, session rows, usage chips. */
const HOT = [
  "claude", "claudecode", "anthropic", "codex", "openai", "grok", "xai",
  "opencode", "gemini", "geminicli", "openrouter",
];

if (!existsSync(svgPkg)) {
  console.error("@lobehub/icons-static-svg is not installed — run npm install first");
  process.exit(1);
}
const version = JSON.parse(readFileSync(join(svgPkg, "package.json"), "utf8")).version;

// ---- SVGs ----
const wantPng = process.argv.includes("--png");
for (const d of ["all", "hot"]) {
  rmSync(join(out, d), { recursive: true, force: true });
  mkdirSync(join(out, d), { recursive: true });
}
const src = join(svgPkg, "icons");
let n = 0;
const names = new Set();
const hasColor = new Set();
for (const f of readdirSync(src)) {
  if (!f.endsWith(".svg") || /-(text|text-cn|brand)(-color)?\.svg$/.test(f)) continue;
  const name = f.replace(/-color\.svg$|\.svg$/, "");
  names.add(name);
  if (f.endsWith("-color.svg")) hasColor.add(name);
  copyFileSync(join(src, f), join(out, HOT.includes(name) ? "hot" : "all", f));
  n++;
}
for (const h of HOT) if (!names.has(h)) console.warn(`hot brand has no icon: ${h}`);
console.log(`${n} svg (${names.size} brands, ${hasColor.size} in colour) from @lobehub/icons-static-svg ${version}`);

// ---- metadata ----
/** The React package's version to read `toc` and `modelMappings` from. Bump
 *  alongside @lobehub/icons-static-svg in package.json. */
const ICONS_VERSION = "5.16.0";
const cdn = `https://cdn.jsdelivr.net/npm/@lobehub/icons@${ICONS_VERSION}`;
async function get(path) {
  for (let tries = 0; ; tries++) {
    try {
      const res = await fetch(`${cdn}/${path}`);
      if (res.ok) return await res.text();
      if (tries >= 4) throw new Error(`${res.status} for ${path}`);
    } catch (e) {
      if (tries >= 4) throw e;
    }
    await new Promise((r) => setTimeout(r, 500 * (tries + 1)));
  }
}

// `toc` is a JSON array of every brand with its colour and variant flags. The
// React package's ids are PascalCase (OpenAI, XAI); lowercased they are the
// SVG names. Colours are recorded as given — #000 and #fff included, since a
// brand whose mark is black is a fact the UI decides what to do with.
const toc = JSON.parse(await get("es/toc.json"));
const brands = {};
for (const t of toc) {
  const id = t.id.toLowerCase();
  if (!names.has(id)) continue;
  brands[id] = {
    title: t.title,
    color: t.color,
    group: t.group,
    ...(hasColor.has(id) ? { color_svg: true } : {}),
  };
}
for (const b of names) if (!brands[b]) console.warn(`no toc entry for ${b}`);
writeFileSync(
  join(out, "brands.json"),
  JSON.stringify(Object.fromEntries(Object.keys(brands).sort().map((k) => [k, brands[k]])), null, 2) + "\n",
);
console.log(`${Object.keys(brands).length} brands → brands.json`);

// `modelMappings` is `[{ Icon: OpenAI, keywords: ['gpt-4', ...], props }]`,
// tested first-match-wins with `new RegExp(keyword, 'i')` against the
// lowercased model id. Only the icon and the keywords matter here; `props`
// picks a colour variant of the same mark that the static set does not have.
const cfg = await get("es/features/modelConfig.js");
const block = cfg.slice(cfg.indexOf("modelMappings = ["));
const models = [];
for (const m of block.matchAll(/Icon:\s*(\w+),\s*keywords:\s*\[([^\]]*)\]/g)) {
  const icon = m[1].toLowerCase();
  const keywords = [...m[2].matchAll(/'((?:[^'\\]|\\.)*)'/g)].map((k) => k[1].replace(/\\\\/g, "\\"));
  if (!names.has(icon)) { console.warn(`model mapping to unknown icon ${icon}`); continue; }
  models.push({ icon, keywords });
}
writeFileSync(join(out, "models.json"), JSON.stringify(models) + "\n");
console.log(`${models.length} model rules → models.json`);

// ---- PNG, on request ----
if (wantPng) {
  const pngPkg = join(root, "node_modules", "@lobehub", "icons-static-png");
  if (!existsSync(pngPkg)) {
    console.error("@lobehub/icons-static-png is not installed");
    process.exit(1);
  }
  let p = 0;
  for (const mode of ["light", "dark"]) {
    const d = join(out, "png", mode);
    rmSync(d, { recursive: true, force: true });
    mkdirSync(d, { recursive: true });
    for (const f of readdirSync(join(pngPkg, mode))) {
      if (!f.endsWith(".png") || /-(text|text-cn|brand|brand-color)\.png$/.test(f)) continue;
      copyFileSync(join(pngPkg, mode, f), join(d, f));
      p++;
    }
  }
  console.log(`${p} png → src/assets/icons/png/{light,dark}`);
}
