/**
 * Brand marks for every engine, model vendor and API host aiterm names.
 *
 * The set is LobeHub's (https://lobehub.com/icons), vendored into
 * `assets/icons` by `scripts/sync-icons.mjs` — see that script for what is
 * copied and why. This module is the one place that turns a thing on screen
 * (an agent id, a model id, a provider's name or URL) into a brand name, and
 * the one place that hands out the SVG and the brand's colour for it. Nothing
 * else knows the file layout.
 *
 * Two loading tiers, because the set is ~550 files and 2.5 MB:
 *
 * - `assets/icons/hot` is bundled eagerly — the engines and the handful of
 *   marks on screen from the first paint, so a session list never draws blank
 *   badges that pop in a frame later.
 * - `assets/icons/all` is a lazy glob: each mark is its own chunk, fetched
 *   the first time a row needs it and cached after. The 300th OpenRouter
 *   vendor costs nothing until a model of theirs is on screen.
 *
 * Every mark has a mono form drawn in `currentColor`, so it takes the theme's
 * text colour like any glyph — that is the light/dark handling. Brands with a
 * colour form use it by default: a coloured mark reads at a glance where the
 * grey one is just another shape.
 */
import { preferredVariant, type Variant } from "./brandMap";

export * from "./brandMap";

// ---- files ----

const hot = import.meta.glob("./assets/icons/hot/*.svg", {
  query: "?raw", import: "default", eager: true,
}) as Record<string, string>;
const lazy = import.meta.glob("./assets/icons/all/*.svg", {
  query: "?raw", import: "default",
}) as Record<string, () => Promise<string>>;

const loaded = new Map<string, string>();
const inflight = new Map<string, Promise<string | undefined>>();

function path(dir: "hot" | "all", name: string, variant: Variant) {
  return `./assets/icons/${dir}/${name}${variant === "color" ? "-color" : ""}.svg`;
}

/** The SVG markup if it is already in memory, else undefined — call
 *  `loadSvg` and re-render when it resolves. */
export function svgFor(name: string, variant: Variant = preferredVariant(name)): string | undefined {
  return hot[path("hot", name, variant)] ?? loaded.get(path("all", name, variant));
}

/** Fetch a mark's chunk. Resolves to undefined for a name/variant with no file. */
export function loadSvg(name: string, variant: Variant = preferredVariant(name)): Promise<string | undefined> {
  const have = svgFor(name, variant);
  if (have !== undefined) return Promise.resolve(have);
  const key = path("all", name, variant);
  const importer = lazy[key];
  if (!importer) return Promise.resolve(undefined);
  let p = inflight.get(key);
  if (!p) {
    p = importer().then((svg) => { loaded.set(key, svg); inflight.delete(key); return svg; });
    inflight.set(key, p);
  }
  return p;
}
