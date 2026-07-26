/**
 * Reading claude's TUI off the screen, so aiterm can put a real dialog in front
 * of it.
 *
 * This is deliberately narrow. It does not try to understand the terminal in
 * general — it looks for a handful of screens we know how to present better,
 * and recognises nothing else. Every detector must be strict enough that a
 * near-miss returns null: showing no dialog costs nothing, while showing the
 * wrong one would send keystrokes into a screen that is not what we think it
 * is.
 *
 * The screen text comes from xterm's own buffer, which has already parsed the
 * escape codes. We are reading rendered characters, not guessing at a byte
 * stream.
 */

/** The visible viewport, one string per row, right-trimmed. */
export type Screen = string[];

export interface ModelOption {
  /** The number claude prints, 1-based, as shown. */
  number: number;
  /** "Default (recommended)", "Opus (1M context)", "Sonnet"… */
  name: string;
  /** "Sonnet 5 · Efficient for routine tasks" */
  description: string;
  /** Marked ✔ — what the session is on right now. */
  current: boolean;
}

export interface ModelPicker {
  kind: "model-picker";
  options: ModelOption[];
  /** Index into `options` of the ❯ row. */
  highlighted: number;
  /** "Low effort" etc, the ○ line. null when absent. */
  effort: string | null;
}

/** Row shape: optional ❯, then "N. Name", then two-plus spaces, then prose. */
const ROW = /^\s*(❯)?\s*(\d+)\.\s+(.+?)\s{2,}(.+?)\s*$/;

/**
 * claude's `/model` picker, or null.
 *
 * Anchored on two things at once: the "Select model" heading and the footer
 * that names its own keys. Requiring the footer matters — it is what tells us
 * `s` means session-only on *this* build. If claude ever renames that key, the
 * detector stops matching and the user simply gets the normal terminal back,
 * rather than us pressing a key that now does something else.
 */
export function detectModelPicker(screen: Screen): ModelPicker | null {
  const hasHeading = screen.some((l) => l.trim() === "Select model");
  const footer = screen.find(
    (l) => l.includes("to use this session only") && l.includes("Esc to cancel"),
  );
  if (!hasHeading || !footer) return null;

  const options: ModelOption[] = [];
  let highlighted = -1;
  for (const line of screen) {
    const m = ROW.exec(line);
    if (!m) continue;
    const [, caret, num, rawName, description] = m;
    if (caret) highlighted = options.length;
    options.push({
      number: Number(num),
      name: rawName.replace(/\s*✔\s*$/, "").trim(),
      description: description.trim(),
      current: /✔/.test(rawName),
    });
  }

  // Numbers must be the complete run 1..n. A partial read — mid-repaint, or a
  // scrolled viewport — would otherwise look like a shorter list, and every
  // index we then computed would be wrong.
  if (options.length < 2 || highlighted < 0) return null;
  if (options.some((o, i) => o.number !== i + 1)) return null;

  const effortLine = screen.find((l) => /^\s*[○●]\s+.*effort/i.test(l));
  const effort = effortLine
    ? effortLine.replace(/^\s*[○●]\s+/, "").replace(/\s*[←→/]+\s*to adjust.*$/i, "").trim()
    : null;

  return { kind: "model-picker", options, highlighted, effort };
}

export interface PermissionOption {
  number: number;
  label: string;
}

export interface PermissionRequest {
  kind: "permission";
  /** "Bash command", "Edit file", … — what claude is asking about. */
  title: string;
  /** The command or diff lines it printed underneath. */
  detail: string[];
  options: PermissionOption[];
  highlighted: number;
}

const OPTION = /^\s*(❯)?\s*(\d+)\.\s+(.+?)\s*$/;
const RULE = /^[\s─━]{10,}$/;

/**
 * claude's tool-permission prompt, or null.
 *
 * Anchored on the question and the footer together. The stakes here are higher
 * than for the model picker — mistaking another screen for this one and sending
 * a key would approve something nobody asked to approve — so the shape has to
 * match exactly or we show nothing.
 */
export function detectPermission(screen: Screen): PermissionRequest | null {
  // The wording is per-tool — "Do you want to proceed?" for a Bash command,
  // "Do you want to create hello.txt?" for a Write, and so on. The shape is
  // what is stable, so anchor on that rather than on any one tool's sentence.
  const q = screen.findIndex((l) => /^\s*Do you want to .+\?\s*$/.test(l));
  if (q < 0) return null;
  const footerAt = screen.findIndex(
    (l, i) => i > q && l.includes("Esc to cancel"),
  );
  if (footerAt < 0) return null;

  const options: PermissionOption[] = [];
  let highlighted = -1;
  for (let i = q + 1; i < footerAt; i++) {
    const m = OPTION.exec(screen[i]);
    if (!m) continue;
    if (m[1]) highlighted = options.length;
    options.push({ number: Number(m[2]), label: m[3].trim() });
  }
  if (options.length < 2 || highlighted < 0) return null;
  if (options.some((o, i) => o.number !== i + 1)) return null;

  // Walk back to the rule above the question; what sits between is what claude
  // is asking about, with the first line as its heading.
  let top = q - 1;
  while (top >= 0 && !RULE.test(screen[top])) top--;
  const block = screen
    .slice(top + 1, q)
    .map((l) => l.trim())
    .filter(Boolean);
  if (!block.length) return null;

  return {
    kind: "permission",
    title: block[0],
    detail: block.slice(1),
    options,
    highlighted,
  };
}

export type Detected = ModelPicker | PermissionRequest;

/** Every detector we know. Order matters only for overlapping screens. */
export function detect(screen: Screen): Detected | null {
  return detectPermission(screen) ?? detectModelPicker(screen);
}

/**
 * Did the last command settle the model for this session only?
 *
 * Read back after driving the picker, so a click that silently did the wrong
 * thing is reported instead of assumed. claude prints one of:
 *   "Set model to X for this session only"
 *   "Set model to X and saved as your default for new sessions"
 *   "Kept model as X"
 */
export function readModelOutcome(
  screen: Screen,
): { text: string; sessionOnly: boolean } | null {
  for (let i = screen.length - 1; i >= 0; i--) {
    const line = screen[i];
    if (/(Set|Kept) model/.test(line)) {
      const text = line.replace(/^[\s⎿·>]*/, "").trim();
      return { text, sessionOnly: /for this session only/.test(text) };
    }
  }
  return null;
}

/**
 * The permission mode claude's status line is advertising.
 *
 * Read rather than tracked, so a shift+tab typed straight into the terminal
 * stays in step with the pill for free — and so a mode we cannot set is still
 * reported honestly.
 *
 * shift+tab cycles four of them. Bypass only joins the loop when the session
 * was launched with `--allow-dangerously-skip-permissions`, which enables it
 * without selecting it — verified by cycling a live session both ways:
 *
 *   without:  manual → accept edits → plan → manual
 *   with:     manual → accept edits → plan → bypass permissions → manual
 *
 * `auto` is outside the loop entirely: it is granted at launch, and once left
 * it cannot be returned to without a new session.
 */
/**
 * Every mode the status line can show. `auto` is here but not in CYCLE_MODES:
 * shift+tab cannot reach it, so it can be displayed and left, never chosen.
 * Leaving it out of this list entirely was worse — the mode went unrecognised
 * and the pill hid itself just when it had the most to say.
 */
export const PERMISSION_MODES = [
  "auto",
  "manual",
  "accept edits",
  "plan",
  "bypass permissions",
] as const;

export type PermissionMode = (typeof PERMISSION_MODES)[number];

/**
 * What shift+tab actually cycles through, in order. The menu offers exactly
 * this and nothing else: a control the CLI does not have is a lie, however
 * convenient. Leaving auto means you cannot return to it without restarting
 * the session — worth saying out loud rather than hiding.
 */
export const CYCLE_MODES: readonly PermissionMode[] = [
  "manual",
  "accept edits",
  "plan",
  "bypass permissions",
];

export function detectPermissionMode(screen: Screen): PermissionMode | null {
  // Search upward: the status line is the last thing drawn, and the same words
  // can appear in conversation text further up.
  for (let i = screen.length - 1; i >= 0; i--) {
    const line = screen[i];
    if (/bypass permissions on/i.test(line)) return "bypass permissions";
    if (/accept edits on/i.test(line)) return "accept edits";
    if (/plan mode on/i.test(line)) return "plan";
    if (/auto mode on/i.test(line)) return "auto";
    if (/manual mode on/i.test(line)) return "manual";
  }
  return null;
}
