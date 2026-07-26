/** App-wide appearance settings: theme, fonts, per-panel sizing. */

export interface PanelScales {
  sessions: number;
  explorer: number;
  git: number;
  agent: number;
}

export interface AppSettings {
  themeId: string;
  /** Accent override; null = theme default. */
  accent: string | null;
  /** UI font family for panels; "" = system default stack. */
  uiFont: string;
  /** Terminal font family; "" = default mono stack. */
  termFont: string;
  /** Terminal base font size in px (global zoom multiplies this). */
  termFontSize: number;
  panelScale: PanelScales;
}

export const DEFAULT_SETTINGS: AppSettings = {
  themeId: "warp-dark",
  accent: null,
  uiFont: "",
  termFont: "",
  termFontSize: 13,
  panelScale: { sessions: 1, explorer: 1, git: 1, agent: 1 },
};

const SETTINGS_KEY = "aiterm.settings";

export function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (!raw) return DEFAULT_SETTINGS;
    const parsed = JSON.parse(raw);
    return {
      ...DEFAULT_SETTINGS,
      ...parsed,
      panelScale: { ...DEFAULT_SETTINGS.panelScale, ...(parsed.panelScale ?? {}) },
    };
  } catch {
    return DEFAULT_SETTINGS;
  }
}

export function saveSettings(s: AppSettings) {
  localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
}

/* ---------- themes ---------- */

export interface Theme {
  id: string;
  name: string;
  vars: {
    bg: string; bgPanel: string; bgRaised: string; bgHover: string; bgActive: string;
    border: string; text: string; textDim: string; textFaint: string;
    accent: string; green: string; red: string; yellow: string; blue: string; cyan: string;
  };
  term: {
    red: string; green: string; yellow: string; blue: string;
    magenta: string; cyan: string; selection: string;
  };
}

export const THEMES: Theme[] = [
  {
    id: "warp-dark",
    name: "Warp Dark",
    vars: {
      bg: "#121317", bgPanel: "#16171d", bgRaised: "#1c1e26", bgHover: "#22242e",
      bgActive: "#2a2d39", border: "#262833", text: "#d8dae5", textDim: "#8b8fa3",
      textFaint: "#5c6070", accent: "#da7756", green: "#98c379", red: "#e06c75",
      yellow: "#e5c07b", blue: "#61afef", cyan: "#56b6c2",
    },
    term: {
      red: "#e06c75", green: "#98c379", yellow: "#e5c07b", blue: "#61afef",
      magenta: "#c678dd", cyan: "#56b6c2", selection: "#33364180",
    },
  },
  {
    id: "one-dark",
    name: "One Dark",
    vars: {
      bg: "#21252b", bgPanel: "#282c34", bgRaised: "#2c313a", bgHover: "#323842",
      bgActive: "#3a404b", border: "#3a3f4b", text: "#abb2bf", textDim: "#7f848e",
      textFaint: "#5c6370", accent: "#61afef", green: "#98c379", red: "#e06c75",
      yellow: "#e5c07b", blue: "#61afef", cyan: "#56b6c2",
    },
    term: {
      red: "#e06c75", green: "#98c379", yellow: "#e5c07b", blue: "#61afef",
      magenta: "#c678dd", cyan: "#56b6c2", selection: "#3e445180",
    },
  },
  {
    id: "dracula",
    name: "Dracula",
    vars: {
      bg: "#1e1f29", bgPanel: "#282a36", bgRaised: "#343746", bgHover: "#3c3f51",
      bgActive: "#44475a", border: "#3c3f51", text: "#f8f8f2", textDim: "#a3aed3",
      textFaint: "#6272a4", accent: "#bd93f9", green: "#50fa7b", red: "#ff5555",
      yellow: "#f1fa8c", blue: "#8be9fd", cyan: "#8be9fd",
    },
    term: {
      red: "#ff5555", green: "#50fa7b", yellow: "#f1fa8c", blue: "#6272a4",
      magenta: "#ff79c6", cyan: "#8be9fd", selection: "#44475a99",
    },
  },
  {
    id: "gruvbox",
    name: "Gruvbox",
    vars: {
      bg: "#1d2021", bgPanel: "#282828", bgRaised: "#32302f", bgHover: "#3c3836",
      bgActive: "#504945", border: "#3c3836", text: "#ebdbb2", textDim: "#a89984",
      textFaint: "#7c6f64", accent: "#fe8019", green: "#b8bb26", red: "#fb4934",
      yellow: "#fabd2f", blue: "#83a598", cyan: "#8ec07c",
    },
    term: {
      red: "#fb4934", green: "#b8bb26", yellow: "#fabd2f", blue: "#83a598",
      magenta: "#d3869b", cyan: "#8ec07c", selection: "#50494580",
    },
  },
  {
    id: "nord",
    name: "Nord",
    vars: {
      bg: "#232831", bgPanel: "#2e3440", bgRaised: "#3b4252", bgHover: "#434c5e",
      bgActive: "#4c566a", border: "#3b4252", text: "#d8dee9", textDim: "#9aa4b5",
      textFaint: "#616e88", accent: "#88c0d0", green: "#a3be8c", red: "#bf616a",
      yellow: "#ebcb8b", blue: "#81a1c1", cyan: "#88c0d0",
    },
    term: {
      red: "#bf616a", green: "#a3be8c", yellow: "#ebcb8b", blue: "#81a1c1",
      magenta: "#b48ead", cyan: "#88c0d0", selection: "#4c566a80",
    },
  },
  {
    id: "tokyo-night",
    name: "Tokyo Night",
    vars: {
      bg: "#16161e", bgPanel: "#1a1b26", bgRaised: "#24283b", bgHover: "#292e42",
      bgActive: "#343a55", border: "#2a2f45", text: "#c0caf5", textDim: "#787c99",
      textFaint: "#565a6e", accent: "#7aa2f7", green: "#9ece6a", red: "#f7768e",
      yellow: "#e0af68", blue: "#7aa2f7", cyan: "#7dcfff",
    },
    term: {
      red: "#f7768e", green: "#9ece6a", yellow: "#e0af68", blue: "#7aa2f7",
      magenta: "#bb9af7", cyan: "#7dcfff", selection: "#343a5580",
    },
  },
  {
    id: "carbon",
    name: "Carbon",
    vars: {
      bg: "#000000", bgPanel: "#0f0f0f", bgRaised: "#181818", bgHover: "#1f1f1f",
      bgActive: "#2a2a2a", border: "#222222", text: "#e4e4e4", textDim: "#9a9a9a",
      textFaint: "#666666", accent: "#e8e8e8", green: "#0dbc79", red: "#f14c4c",
      yellow: "#cca700", blue: "#3b8eea", cyan: "#29b8db",
    },
    term: {
      red: "#cd3131", green: "#0dbc79", yellow: "#e5e510", blue: "#2472c8",
      magenta: "#bc3fbc", cyan: "#11a8cd", selection: "#ffffff22",
    },
  },
  {
    id: "solarized",
    name: "Solarized",
    vars: {
      bg: "#001a21", bgPanel: "#002b36", bgRaised: "#073642", bgHover: "#0a4252",
      bgActive: "#10505f", border: "#0a3d4a", text: "#adbcbc", textDim: "#839496",
      textFaint: "#586e75", accent: "#268bd2", green: "#859900", red: "#dc322f",
      yellow: "#b58900", blue: "#268bd2", cyan: "#2aa198",
    },
    term: {
      red: "#dc322f", green: "#859900", yellow: "#b58900", blue: "#268bd2",
      magenta: "#d33682", cyan: "#2aa198", selection: "#07364299",
    },
  },
];

export const ACCENT_SWATCHES = [
  "#e8e8e8", "#9aa4b5", "#da7756", "#fe8019", "#e5c07b", "#cca700", "#98c379",
  "#50fa7b", "#2aa198", "#56b6c2", "#61afef", "#7aa2f7", "#bd93f9", "#c678dd",
  "#ff79c6", "#e06c75",
];

export function themeById(id: string): Theme {
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}

const UI_FALLBACK = `-apple-system, "Segoe UI", "Inter", system-ui, sans-serif`;
const MONO_FALLBACK = `"JetBrainsMono Nerd Font", "JetBrains Mono", "Fira Code", monospace`;

/** Push the current theme + fonts into CSS custom properties on :root. */
export function applySettings(s: AppSettings) {
  const t = themeById(s.themeId);
  const r = document.documentElement.style;
  r.setProperty("--bg", t.vars.bg);
  r.setProperty("--bg-panel", t.vars.bgPanel);
  r.setProperty("--bg-raised", t.vars.bgRaised);
  r.setProperty("--bg-hover", t.vars.bgHover);
  r.setProperty("--bg-active", t.vars.bgActive);
  r.setProperty("--border", t.vars.border);
  r.setProperty("--text", t.vars.text);
  r.setProperty("--text-dim", t.vars.textDim);
  r.setProperty("--text-faint", t.vars.textFaint);
  r.setProperty("--accent", s.accent ?? t.vars.accent);
  r.setProperty("--green", t.vars.green);
  r.setProperty("--red", t.vars.red);
  r.setProperty("--yellow", t.vars.yellow);
  r.setProperty("--blue", t.vars.blue);
  r.setProperty("--cyan", t.vars.cyan);
  r.setProperty("--font-ui", s.uiFont ? `"${s.uiFont}", ${UI_FALLBACK}` : UI_FALLBACK);
  r.setProperty("--font-mono", s.termFont ? `"${s.termFont}", ${MONO_FALLBACK}` : MONO_FALLBACK);
}

export function termFontFamily(s: AppSettings): string {
  return s.termFont ? `"${s.termFont}", ${MONO_FALLBACK}` : MONO_FALLBACK;
}

/** xterm theme derived from the app theme. */
export function termTheme(s: AppSettings) {
  const t = themeById(s.themeId);
  return {
    background: t.vars.bg,
    foreground: t.vars.text,
    cursor: s.accent ?? t.vars.accent,
    selectionBackground: t.term.selection,
    black: t.vars.bgRaised,
    red: t.term.red,
    green: t.term.green,
    yellow: t.term.yellow,
    blue: t.term.blue,
    magenta: t.term.magenta,
    cyan: t.term.cyan,
    white: t.vars.text,
    brightBlack: t.vars.textFaint,
  };
}

/* ---------- installed-font detection ---------- */

/**
 * Font discovery lives in the backend now — see `src-tauri/src/fonts.rs`.
 *
 * This used to be a canvas width-probe against a hardcoded candidate list:
 * render a sample string in `"Name", monospace` and in plain `monospace`, and
 * call the font installed when the widths differed. It worked, but it could
 * only ever find fonts someone had thought to list, so the picker quietly
 * offered a fraction of what was actually on the machine. fontconfig knows
 * the real answer — `listFonts()` in ipc.ts asks it.
 */
