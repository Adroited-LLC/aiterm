/**
 * Which brand a thing on screen is — pure lookup, no files.
 *
 * `brand.ts` adds the SVG loading on top; this half is what the tests cover,
 * and it runs under plain Node (`npm run test:ui`) because nothing here needs
 * Vite. The data is what `scripts/sync-icons.mjs` writes from the LobeHub set:
 * `brands.json` (title, primary colour, group, whether a colour mark exists)
 * and `models.json` (LobeHub's own model-id → brand rules).
 */
import type { CSSProperties } from "react";
import brandsJson from "./assets/icons/brands.json" with { type: "json" };
import modelsJson from "./assets/icons/models.json" with { type: "json" };

export interface Brand {
  title: string;
  /** Primary colour as LobeHub records it — `#000`/`#fff` included, so
   *  `brandAccent` decides what a black-on-transparent mark is highlighted with. */
  color: string;
  group: "model" | "provider" | "application";
  /** A `<name>-color.svg` exists. */
  color_svg?: boolean;
}

export const BRANDS: Record<string, Brand> = brandsJson as Record<string, Brand>;

/** LobeHub's `modelMappings`: first rule whose keyword matches the lowercased
 *  model id wins, keywords being regex sources. */
const MODEL_RULES: { icon: string; keywords: string[] }[] = modelsJson;
const MODEL_RE = MODEL_RULES.map((r) => ({
  icon: r.icon,
  res: r.keywords.map((k) => new RegExp(k, "i")),
}));

export type Variant = "color" | "mono";

/** Which form to draw: colour when the brand has one, else mono. */
export function preferredVariant(name: string): Variant {
  return BRANDS[name]?.color_svg ? "color" : "mono";
}

/** Whether a brand of this name is in the set at all. */
export function hasBrand(name: string | null | undefined): boolean {
  return !!name && name in BRANDS;
}

// ---- colour ----

/** The brand's primary colour, as recorded. */
export function brandColor(name: string): string | undefined {
  return BRANDS[name]?.color;
}

/** Luminance of a `#rgb`/`#rrggbb`, 0..1, or null if it is not one. */
function luma(hex: string): number | null {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  let h = m[1];
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** A colour worth tinting UI with: the brand's primary unless it is black or
 *  white — the "colour" of a mark drawn in ink says nothing on a themed
 *  surface, and a tint of it would be a grey box. Null means "use the theme". */
/** Brands whose recorded primary is ink but whose colour mark has one
 *  obvious hue — the hue is the better tint. */
const ACCENT_OVERRIDE: Record<string, string> = {
  gemini: "#1C69FF",
};

export function brandAccent(name: string): string | null {
  const o = ACCENT_OVERRIDE[name];
  if (o) return o;
  const c = brandColor(name);
  if (!c) return null;
  const l = luma(c);
  if (l === null) return c;
  return l < 0.08 || l > 0.92 ? null : c;
}

// ---- resolving ----

/** Agent ids as the backends name them. */
const AGENT_BRAND: Record<string, string> = {
  claude: "claude",
  codex: "codex",
  openai: "openai",
  grok: "grok",
  opencode: "opencode",
  gemini: "gemini",
};

/** Per-engine hue from the theme for the marks whose own colour is ink: the
 *  same assignments `App.css` gives the mono icons, so a tinted badge and its
 *  icon agree. */
const AGENT_THEME_ACCENT: Record<string, string> = {
  codex: "var(--green)",
  openai: "var(--green)",
  grok: "var(--magenta)",
  opencode: "var(--cyan)",
  api: "var(--blue)",
};

export function brandForAgent(agent: string): string | null {
  const b = AGENT_BRAND[agent] ?? (hasBrand(agent) ? agent : null);
  return b && hasBrand(b) ? b : null;
}

/** Something to tint a session's badge or tab with: the brand's colour where
 *  it has one, else the theme's hue for that engine, else null. */
export function agentAccent(agent: string): string | null {
  const b = brandForAgent(agent);
  return (b && brandAccent(b)) ?? AGENT_THEME_ACCENT[agent] ?? null;
}

/** The class and `--brand` for an element tinted by `agentAccent` — a badge,
 *  a tab. `className` is "" or " branded" (leading space, for concatenation);
 *  the CSS for `.branded` reads `var(--brand)`. */
export function agentTint(agent: string | null | undefined): { className: string; style?: CSSProperties } {
  const a = agent ? agentAccent(agent) : null;
  return a ? { className: " branded", style: { "--brand": a } as CSSProperties } : { className: "" };
}

function norm(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]/g, "");
}

/** Names that do not normalise onto a brand: OpenRouter's host names, model
 *  vendor slugs, and a few companies the set files under a product. */
const NAME_ALIAS: Record<string, string> = {
  // Vendor slugs in `vendor/model` ids.
  metallama: "meta",
  mistralai: "mistral",
  moonshotai: "moonshot",
  xai: "xai",
  zai: "zai",
  allenai: "ai2",
  ibmgranite: "ibm",
  arceeai: "arcee",
  rekaai: "reka",
  bytedanceseed: "bytedance",
  xiaomi: "xiaomimimo",
  "01ai": "yi",
  thudm: "chatglm",
  cognitivecomputations: "dolphin",
  amazon: "aws",
  // OpenRouter host names.
  googlevertex: "vertexai",
  googleaistudio: "aistudio",
  amazonbedrock: "bedrock",
  claudeplatformonaws: "bedrock",
  amazonnova: "nova",
  nebiusaistudio: "nebius",
  novitaai: "novita",
  klusterai: "kluster",
  inferencenet: "inference",
  siliconflow: "siliconcloud",
  voyageaibymongodb: "voyage",
  blackforestlabs: "bfl",
  inferactvllm: "vllm",
  akashml: "akashchat",
  mancer2: "mancer",
  azureai: "azure",
  // Hosts by URL label.
  googleapis: "gemini",
  aliyuncs: "qwen",
  bigmodel: "zhipu",
  volces: "doubao",
  lingyiwanwu: "yi",
  amazonaws: "bedrock",
  baidubce: "wenxin",
  x: "xai",
  z: "zai",
};

const SUFFIXES = ["aistudio", "ai", "labs", "inc", "research", "studio", "cloud", "api"];

/** Brand names long enough that "starts with" is not a coincidence, longest
 *  first so `alibabacloud` wins over `alibaba`. */
const PREFIXABLE = Object.keys(BRANDS).filter((b) => b.length >= 5).sort((a, b) => b.length - a.length);

/** A provider or vendor by its name — "DeepInfra", "Moonshot AI", "x-ai". */
export function brandForName(name: string | null | undefined): string | null {
  if (!name) return null;
  const n = norm(name);
  if (!n) return null;
  if (hasBrand(n)) return n;
  const a = NAME_ALIAS[n];
  if (a && hasBrand(a)) return a;
  for (const s of SUFFIXES) {
    if (n.length > s.length + 2 && n.endsWith(s)) {
      const t = n.slice(0, -s.length);
      if (hasBrand(t)) return t;
      const ta = NAME_ALIAS[t];
      if (ta && hasBrand(ta)) return ta;
    }
  }
  for (const b of PREFIXABLE) if (n.startsWith(b)) return b;
  return null;
}

/** A model by its id, as OpenRouter ("anthropic/claude-sonnet-4"), a bare
 *  OpenAI-compatible host ("gpt-4o") or a CLI ("claude-opus-4-1") names it.
 *  LobeHub's rules first — they know "o3" is OpenAI and "glm-4v" is not GLM —
 *  then the vendor prefix, which covers the fine-tuners the rules never heard
 *  of. Null when neither knows it, and the caller draws nothing rather than a
 *  wrong mark. */
export function brandForModel(id: string | null | undefined): string | null {
  if (!id) return null;
  const m = id.toLowerCase();
  for (const r of MODEL_RE) if (r.res.some((re) => re.test(m))) return r.icon;
  const slash = m.indexOf("/");
  if (slash > 0) return brandForName(m.slice(0, slash).replace(/^~/, ""));
  return null;
}

/** An API host by its base URL: "https://api.groq.com/openai/v1" → groq. The
 *  registrable label of the host is a brand name often enough that the alias
 *  table only has to cover the ones that are not (googleapis, aliyuncs). */
export function brandForUrl(url: string | null | undefined): string | null {
  if (!url) return null;
  let host: string;
  try { host = new URL(url).hostname.toLowerCase(); } catch { return null; }
  if (host === "localhost" || host === "127.0.0.1" || host === "::1") {
    try {
      const port = new URL(url).port;
      if (port === "11434") return "ollama";
      if (port === "1234") return "lmstudio";
    } catch { /* fall through */ }
    return null;
  }
  const labels = host.split(".").filter(Boolean);
  // Public suffixes with two labels (co.uk, com.cn): step past them.
  const two = new Set(["co", "com", "org", "net", "ac", "gov"]);
  let i = labels.length - 2;
  if (i > 0 && two.has(labels[i]) && labels[i + 1].length === 2) i--;
  if (i < 0) return null;
  // Try the registrable label, then each subdomain label inward — "openrouter"
  // in api.openrouter.ai, "bedrock-runtime" in bedrock-runtime.us-east-1.amazonaws.com.
  const tries = [labels[i], ...labels.slice(0, i).reverse()];
  for (const t of tries) {
    const b = brandForName(t.split("-")[0]) ?? brandForName(t);
    if (b) return b;
  }
  return null;
}

/** A usage source (`UsageSource.id` + `name`): the Claude plan is the Claude
 *  mark, the CLIs their own, and an API provider whatever its name resolves to. */
export function brandForUsageSource(id: string, name: string): string | null {
  if (id === "anthropic") return "claude";
  if (id === "codex" || id === "grok") return id;
  if (id.startsWith("provider:")) return brandForName(name) ?? brandForName(id.slice(9));
  return brandForName(id) ?? brandForName(name);
}
