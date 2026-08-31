/**
 * Bringing a second agent into a session, and letting the two talk.
 *
 * Not a review after the fact: a relay. The second agent opens in a tab of
 * its own with the first's conversation in front of it and a request to
 * address that agent directly. When its reply lands, aiterm types it into
 * the first agent's terminal as a message from the second; when the first
 * answers, that goes back. A few rounds, then it stops and the person —
 * who has both tabs — decides.
 *
 * "The reply landed" is read from the transcript (one more assistant
 * message than before) together with the terminal having gone quiet: the
 * message is written when the turn ends, and a moment of silence after it
 * keeps a tool-using turn from being relayed mid-way.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { homeAbbrev, sessionConversation, sessionDetail as getSessionDetail } from "./ipc";
import type { TermHandle, TermTab } from "./components/TerminalView";
import type { StartChoice } from "./components/StartControls";

export interface RelayState {
  aKey: number;
  bKey: number;
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
/** How much of the first conversation the second agent sees. */
const CONTEXT_CHARS = 24_000;

function engineName(agentId: string | undefined, model?: string | null): string {
  const base = agentId === "claude" ? "Claude Code" : agentId === "codex" ? "Codex" : agentId === "grok" ? "Grok" : agentId ?? "the other agent";
  return model ? `${base} (${model})` : base;
}

export function useRelay(io: {
  tabs: () => TermTab[];
  handle: (key: number) => TermHandle | undefined;
  /** ms since this tab last produced output. */
  quietFor: (key: number) => number;
  /** Whether the tab is reporting progress (a turn in flight). */
  busy: (key: number) => boolean;
  /** Start a session; resolves with the tab opened. */
  open: (cwd: string, choice: StartChoice, prompt: string, extra: { parentKey: number; title: string }) => Promise<{ key: number; sessionId?: string } | null>;
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

  /** Wait for one more assistant message in a tab's session, then a quiet
   *  moment; resolve with its text. Rejects when the tab is gone. */
  const awaitReply = useCallback((key: number, baseline: number, myGen: number): Promise<string> =>
    new Promise((resolve, reject) => {
      const tick = async () => {
        if (myGen !== gen.current) return; // stopped or superseded
        const tab = ioRef.current.tabs().find((t) => t.key === key);
        if (!tab) { reject(new Error("that tab was closed")); return; }
        try {
          if (tab.sessionId) {
            const d = await getSessionDetail(tab.sessionId);
            const n = d?.assistant_messages ?? 0;
            const text = d?.last_assistant?.trim() ?? "";
            if (n > baseline && text && !ioRef.current.busy(key) && ioRef.current.quietFor(key) > QUIET_MS) {
              resolve(text);
              return;
            }
          }
        } catch { /* a transient read failure is just a missed tick */ }
        timer.current = window.setTimeout(tick, POLL_MS);
      };
      timer.current = window.setTimeout(tick, POLL_MS);
    }), []);

  const countOf = async (key: number): Promise<number> => {
    const tab = ioRef.current.tabs().find((t) => t.key === key);
    if (!tab?.sessionId) return 0;
    try { return (await getSessionDetail(tab.sessionId))?.assistant_messages ?? 0; } catch { return 0; }
  };

  const start = useCallback(async (opts: { aKey: number; choice: StartChoice; focus: string; rounds: number; auto?: boolean }) => {
    const a = ioRef.current.tabs().find((t) => t.key === opts.aKey);
    if (!a?.sessionId || !a.cwd) return;
    gen.current++;
    const myGen = gen.current;
    const aName = engineName(a.agentId);
    const bName = opts.choice.kind === "agent" ? engineName(opts.choice.agentId, opts.choice.model) : opts.choice.modelId;
    setRelay({ aKey: a.key, bKey: -1, aName, bName, round: 1, rounds: opts.rounds, phase: "opening", note: "" });

    let turns: [string, string][] = [];
    try { turns = await sessionConversation(a.sessionId, CONTEXT_CHARS); } catch { /* an empty context still works */ }
    const focus = opts.focus.trim() || "a second view: is this the best way to do it, and what would you do differently?";
    const transcript = turns.map(([r, t]) => `[${r}]\n${t}`).join("\n\n");
    const opening = [
      `You are being brought into a live working session as a second agent. Another agent — ${aName} — has been working with the user in ${homeAbbrev(a.cwd)}; the conversation so far is below. The user wants ${focus}`,
      ``,
      `Form your own view — read the files it touched if you need to — and then write a message addressed to that agent directly. It will receive your message verbatim and reply to you; the user is reading along. Be concrete: what you agree with, what you would do differently and why, and if you would take a different approach, sketch it. Do not restate its work back to it. End with a line starting "PROPOSAL:" giving the direction you recommend in one or two sentences.`,
      ``,
      `Do not modify files in this round. This is a conversation between agents; the user decides what happens next.`,
      ``,
      `--- conversation so far (${aName} with the user) ---`,
      transcript || "(nothing yet)",
    ].join("\n");

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
      for (let round = 1; round <= opts.rounds; round++) {
        setRelay((r) => r && { ...r, round, phase: "waitB" });
        const fromB = await awaitReply(bKey, bBase, myGen);
        if (myGen !== gen.current) return;
        const aBase = await countOf(a.key);
        const toA = round === 1
          ? [
              `A second agent — ${bName} — has been brought into this session by the user for another view on: ${focus}`,
              `They read this conversation and wrote the following to you:`,
              `---`, fromB, `---`,
              `Reply to them directly: where you agree, where you do not and why, and what you would change. Do not edit files yet — the user will decide the direction after you two have talked. If you now agree on a way forward, end with a line starting "AGREED:" summarising it.`,
            ].join("\n")
          : [
              `${bName} replied:`, `---`, fromB, `---`,
              `Respond to them. If you now agree on a way forward, end with a line starting "AGREED:" summarising it; if you still differ, end with "PROPOSAL:" and your recommendation. No file edits yet.`,
            ].join("\n");
        ioRef.current.handle(a.key)?.sendComposed(toA);
        setRelay((r) => r && { ...r, phase: "waitA" });
        const fromA = await awaitReply(a.key, aBase, myGen);
        if (myGen !== gen.current) return;
        const agreed = /^AGREED:/m.test(fromA) || /^AGREED:/m.test(fromB);
        if (round === opts.rounds || agreed) {
          // The last word goes to the second agent's tab too, so both
          // transcripts hold the whole exchange.
          bBase = await countOf(bKey);
          ioRef.current.handle(bKey)?.sendComposed([
            `${aName} replied:`, `---`, fromA, `---`,
            agreed
              ? `You two have converged. Do nothing further; the user will take it from here.`
              : `That is the last round. Do nothing further; the user has both views and will decide.`,
          ].join("\n"));
          if (opts.auto) {
            // The user pre-approved acting on the outcome: the first agent
            // proceeds instead of parking for a decision.
            ioRef.current.handle(opts.aKey)?.sendComposed([
              `The user pre-approved acting on this exchange. Proceed now:`,
              agreed
                ? `implement the AGREED direction.`
                : `weigh both views and implement the direction you judge best, noting where you differed from ${bName}.`,
              `You may edit files. Work as usual and report when done.`,
            ].join("\n"));
          }
          setRelay((r) => r && {
            ...r,
            phase: "done",
            note: (agreed ? "they agreed" : "both views are in") + (opts.auto ? " — acting on it" : ""),
          });
          return;
        }
        bBase = await countOf(bKey);
        ioRef.current.handle(bKey)?.sendComposed([
          `${aName} replied:`, `---`, fromA, `---`,
          `Respond to them. If you now agree on a way forward, end with a line starting "AGREED:" summarising it; if you still differ, end with "PROPOSAL:" and your recommendation. No file edits yet.`,
        ].join("\n"));
      }
    } catch (e) {
      if (myGen === gen.current) setRelay((r) => r && { ...r, phase: "error", note: String(e instanceof Error ? e.message : e) });
    }
  }, [awaitReply]);

  useEffect(() => () => { if (timer.current !== null) clearTimeout(timer.current); }, []);
  const clear = useCallback(() => { stop("stopped"); setRelay(null); }, [stop]);

  return { relay, start, stop, clear };
}
