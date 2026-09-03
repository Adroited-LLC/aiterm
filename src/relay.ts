/**
 * Bringing a second agent into a session, and letting the two talk.
 *
 * Not a review after the fact: a relay. The second agent opens in a tab of
 * its own, is told where the first agent's transcript is on disk, reads it
 * itself, and writes to that agent directly. When its reply lands, aiterm
 * types it — whole — into the first agent's terminal; when the first
 * answers, that goes back. Rounds are how many times the second agent
 * speaks; its last message goes to the first agent as something to take
 * into account and carry on from, not something to answer.
 *
 * What either agent is told comes from Settings → Bring in; nothing here
 * restricts what they may do, and the second agent launches in whatever
 * mode its engine is set to.
 *
 * "The reply landed" is read from the transcript (one more assistant
 * message than before) together with the terminal having gone quiet: the
 * message is written when the turn ends, and a moment of silence after it
 * keeps a tool-using turn from being relayed mid-way. The text relayed is
 * the whole last message, read back from the conversation, not the
 * clipped preview the detail card shows.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { sessionConversation, sessionDetail as getSessionDetail, sessionTranscriptPath } from "./ipc";
import type { TermHandle, TermTab } from "./components/TerminalView";
import type { StartChoice } from "./components/StartControls";
import { BRING_IN_DEFAULTS, BringInPrompts } from "./settings";

export interface RelayState {
  aKey: string;
  bKey: string;
  aName: string;
  bName: string;
  round: number;
  rounds: number;
  /** Whose reply is being waited on. */
  phase: "opening" | "waitB" | "waitA" | "done" | "stopped" | "error";
  note: string;
}

/** How long the terminal must be silent after the transcript grew before
 *  the reply counts as finished. */
const QUIET_MS = 3_500;
const POLL_MS = 2_000;
/** How much of a conversation is read back to find the last message whole. */
const READ_BACK_CHARS = 400_000;

export function engineName(agentId: string | undefined, model?: string | null): string {
  const base = agentId === "claude" ? "Claude Code" : agentId === "codex" ? "Codex" : agentId === "grok" ? "Grok" : agentId === "antigravity" ? "Antigravity" : agentId ?? "the other agent";
  return model ? `${base} (${model})` : base;
}

/** A prompt with its placeholders filled; an unknown one is left as is. */
export function fill(template: string, vars: Record<string, string>): string {
  return template.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? vars[k] : m));
}

/** The prompt in force: the person's edit when they made one, else ours. */
export function promptOr(custom: string, fallback: string): string {
  return custom.trim() ? custom : fallback;
}

export function useRelay(io: {
  prompts: () => BringInPrompts;
  tabs: () => TermTab[];
  handle: (key: string) => TermHandle | undefined;
  /** ms since this tab last produced output. */
  quietFor: (key: string) => number;
  /** Whether the tab is reporting progress (a turn in flight). */
  busy: (key: string) => boolean;
  /** Start a session; resolves with the tab opened. */
  open: (cwd: string, choice: StartChoice, prompt: string, extra: { parentKey: string; title: string; permissionFlags?: string }) => Promise<{ key: string; sessionId?: string } | null>;
}) {
  const [relay, setRelay] = useState<RelayState | null>(null);
  const timer = useRef<number | null>(null);
  const gen = useRef(0);
  const ioRef = useRef(io);
  ioRef.current = io;

  const stop = useCallback((why: RelayState["phase"] = "stopped", note = "") => {
    gen.current++;
    if (timer.current !== null) { clearTimeout(timer.current); timer.current = null; }
    setRelay((r) => (r ? { ...r, phase: why, note: note || r.note } : r));
  }, []);

  /** The whole of a session's last assistant message. */
  const lastMessage = async (sessionId: string, fallback: string): Promise<string> => {
    try {
      const turns = await sessionConversation(sessionId, READ_BACK_CHARS);
      for (let i = turns.length - 1; i >= 0; i--) {
        if (turns[i][0] === "assistant" && turns[i][1].trim()) return turns[i][1].trim();
      }
    } catch { /* the preview is better than nothing */ }
    return fallback;
  };

  /** Wait for one more assistant message in a tab's session, then a quiet
   *  moment; resolve with its whole text. Rejects when the tab is gone. */
  const awaitReply = useCallback((key: string, baseline: number, myGen: number): Promise<string> =>
    new Promise((resolve, reject) => {
      const tick = async () => {
        if (myGen !== gen.current) return; // stopped or superseded
        const tab = ioRef.current.tabs().find((t) => t.key === key);
        if (!tab) { reject(new Error("that tab was closed")); return; }
        try {
          if (tab.sessionId) {
            const d = await getSessionDetail(tab.sessionId);
            const n = d?.assistant_messages ?? 0;
            const preview = d?.last_assistant?.trim() ?? "";
            if (n > baseline && preview && !ioRef.current.busy(key) && ioRef.current.quietFor(key) > QUIET_MS) {
              resolve(await lastMessage(tab.sessionId, preview));
              return;
            }
          }
        } catch { /* a transient read failure is just a missed tick */ }
        timer.current = window.setTimeout(tick, POLL_MS);
      };
      timer.current = window.setTimeout(tick, POLL_MS);
    }), []);

  const countOf = async (key: string): Promise<number> => {
    const tab = ioRef.current.tabs().find((t) => t.key === key);
    if (!tab?.sessionId) return 0;
    try { return (await getSessionDetail(tab.sessionId))?.assistant_messages ?? 0; } catch { return 0; }
  };

  const pathOf = async (sessionId: string | undefined): Promise<string> => {
    if (!sessionId) return "";
    try { return await sessionTranscriptPath(sessionId); } catch { return ""; }
  };

  const start = useCallback(async (opts: { aKey: string; choice: StartChoice; focus: string; rounds: number; auto?: boolean }) => {
    const a = ioRef.current.tabs().find((t) => t.key === opts.aKey);
    if (!a?.sessionId || !a.cwd) return;
    gen.current++;
    const myGen = gen.current;
    const rounds = Math.max(1, Math.floor(opts.rounds));
    const aName = engineName(a.agentId);
    const bName = opts.choice.kind === "agent" ? engineName(opts.choice.agentId, opts.choice.model) : opts.choice.modelId;
    setRelay({ aKey: a.key, bKey: "", aName, bName, round: 1, rounds, phase: "opening", note: "" });

    const p = ioRef.current.prompts();
    const focus = opts.focus.trim() || "a second view on the work so far.";
    const aPath = await pathOf(a.sessionId);
    if (myGen !== gen.current) return;
    const vars = { a: aName, b: bName, focus, path: aPath, text: "" };
    const opening = fill(promptOr(p.opening, BRING_IN_DEFAULTS.opening), vars);

    const short = opts.choice.kind === "agent"
      ? (opts.choice.model || engineName(opts.choice.agentId))
      : (opts.choice.modelId.split("/").pop() || opts.choice.modelId);
    const opened = await ioRef.current.open(a.cwd, opts.choice, opening, { parentKey: a.key, title: short });
    if (myGen !== gen.current) return;
    if (!opened) { setRelay((r) => r && { ...r, phase: "error", note: "could not start the second agent" }); return; }
    const bKey = opened.key;
    setRelay((r) => r && { ...r, bKey, phase: "waitB" });

    try {
      let bBase = 0;
      let bPath = "";
      for (let round = 1; round <= rounds; round++) {
        setRelay((r) => r && { ...r, round, phase: "waitB" });
        const fromB = await awaitReply(bKey, bBase, myGen);
        if (myGen !== gen.current) return;
        if (!bPath) bPath = await pathOf(ioRef.current.tabs().find((t) => t.key === bKey)?.sessionId ?? opened.sessionId);
        const last = round === rounds;
        let toA = fill(promptOr(last ? p.toFirstLast : p.toFirst, last ? BRING_IN_DEFAULTS.toFirstLast : BRING_IN_DEFAULTS.toFirst), { ...vars, path: bPath, text: fromB });
        if (last && opts.auto) {
          toA += "\n\n" + fill(promptOr(p.approved, BRING_IN_DEFAULTS.approved), { ...vars, path: bPath, text: fromB });
        }
        const aBase = await countOf(a.key);
        ioRef.current.handle(a.key)?.sendComposed(toA);
        if (last) {
          setRelay((r) => r && { ...r, phase: "done", note: opts.auto ? "handed back as approved" : "handed back" });
          return;
        }
        setRelay((r) => r && { ...r, phase: "waitA" });
        const fromA = await awaitReply(a.key, aBase, myGen);
        if (myGen !== gen.current) return;
        bBase = await countOf(bKey);
        ioRef.current.handle(bKey)?.sendComposed(fill(promptOr(p.toSecond, BRING_IN_DEFAULTS.toSecond), { ...vars, path: aPath, text: fromA }));
      }
    } catch (e) {
      if (myGen === gen.current) setRelay((r) => r && { ...r, phase: "error", note: String(e instanceof Error ? e.message : e) });
    }
  }, [awaitReply]);

  useEffect(() => () => { if (timer.current !== null) clearTimeout(timer.current); }, []);
  const clear = useCallback(() => { stop("stopped"); setRelay(null); }, [stop]);

  return { relay, start, stop, clear };
}
