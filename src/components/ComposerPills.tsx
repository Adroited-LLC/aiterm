import { useEffect, useRef, useState } from "react";
import GitPanel from "./GitPanel";
import { CYCLE_MODES, PermissionMode } from "../term/screen";
import {
  AgentRun, Artifact, ModelChoice, SessionTask, UsageBar,
  homeAbbrev, openPath, relTime,
  sessionAgents, sessionArtifacts, sessionModel, sessionTasks,
} from "../ipc";

/** Aliases used only to shorten a recorded model id for the pill — "claude-
 *  opus-5" to "opus". The picker no longer lists these: it shows claude's own
 *  entries, read off the screen, so there are no descriptions here to drift
 *  out of date. */
const MODEL_ALIASES = ["opus", "opusplan", "sonnet", "haiku", "fable"];

/** What each permission mode actually means, in claude's own terms. */
const PERM_DESC: Record<PermissionMode, string> = {
  "auto": "Claude's classifier vets each action and asks when unsure",
  "manual": "Ask before every tool call",
  "accept edits": "File edits go through, everything else asks",
  "plan": "Research and plan only — no changes until you approve",
  "bypass permissions": "Never ask. Only enabled when the session was started with it",
};

/** `/effort <level>` choices. The medium and high wordings are claude's own. */
const EFFORTS: [string, string][] = [
  ["auto", "Let claude pick per turn"],
  ["low", "Quick and direct — minimal deliberation"],
  ["medium", "Balanced approach with standard implementation and testing"],
  ["high", "Comprehensive implementation with extensive testing and documentation"],
  ["xhigh", "Deeper reasoning for expensive-to-get-wrong work"],
  ["max", "Maximum deliberation, slowest"],
  ["ultracode", "Max effort plus multi-agent orchestration by default"],
];

/** "claude-opus-5" → "opus". Falls back to the raw id when unrecognised, so a
 *  new model shows *something* rather than going blank. */
function shortModel(id: string | null): string {
  if (!id) return "—";
  const m = MODEL_ALIASES.find((n) => id.includes(n));
  return m ?? id.replace(/^claude-/, "");
}

/**
 * What was last *chosen* here, per session.
 *
 * Clicking a model is a request, not a fact. It can be refused (`/model sonnet`
 * answering "Kept model as Opus 5"), or swallowed by a prompt you were halfway
 * through typing. The transcript is the fact: it records what each turn
 * actually ran at.
 *
 * Showing only the request lies once it fails. Showing only the transcript
 * ignores you until the next turn runs, which reads as the pill being broken.
 * So show the request immediately, marked pending, and let the transcript
 * overrule it the moment a turn settles the question — which is what `seenAt`
 * detects: the timestamp of the newest assistant record when the click
 * happened. Once that moves, a turn has run and the file wins.
 *
 * `auto` effort is the exception that outlives settling, because every turn
 * resolves to a concrete level and so `auto` can never appear in a transcript.
 */
const PICK_KEY = "aiterm.composerPick";
type Picks = Record<
  string,
  { model?: string; effort?: string; seenAt?: string | null }
>;

function loadPicks(): Picks {
  try {
    return JSON.parse(localStorage.getItem(PICK_KEY) ?? "{}");
  } catch {
    return {};
  }
}

/**
 * The composer's control strip: a row of counts that open a panel when clicked.
 *
 * The composer has always been collapsed by default, because as a text input it
 * is redundant — claude draws its own. This is the part that earns the space:
 * what the session is doing, visible without giving up a panel to it, and one
 * click from the detail.
 *
 * Reads the same session commands the right-hand panels read. Plan usage is
 * the exception: App owns that one reading and hands it to both this and the
 * top-bar gauges, because the endpoint rate limits and two pollers would have
 * them disagreeing whenever one request was refused.
 */

// "model" is absent on purpose: choosing a model opens claude's own picker
// with a dialog over it, rather than a list of our own invention.
type PanelKey = "tasks" | "artifacts" | "agents" | "usage" | "effort" | "git" | "perms";

/** Relative "resets in 3h 55m"; "" when unknown or past. */
function resetsIn(iso: string): string {
  if (!iso) return "";
  const ms = new Date(iso).getTime() - Date.now();
  if (!isFinite(ms) || ms <= 0) return "resetting";
  const mins = Math.round(ms / 60000);
  const d = Math.floor(mins / 1440);
  const h = Math.floor((mins % 1440) / 60);
  const m = mins % 60;
  if (d > 0) return `resets in ${d}d ${h}h`;
  if (h > 0) return `resets in ${h}h ${m}m`;
  return `resets in ${m}m`;
}

interface Props {
  sessionId: string | null;
  /** Repo the git pill reports on — the active project. */
  projectRoot: string | null;
  /** Plan usage, owned by App. Fetched once there and shared with the top-bar
   *  gauges: the endpoint rate limits, so a second poller would both double
   *  the requests and let the two views disagree when one is refused. */
  usage: UsageBar[];
  /** Set when the reading came from the on-disk cache and no live call has
   *  succeeded yet — the numbers are real, just possibly old, and saying so is
   *  cheaper than letting them quietly mislead. */
  usageAsOf: number | null;
  /** Run a slash command in the live terminal (adds Enter). Absent when no
   *  terminal is focused, which is what disables the model/effort pills. */
  onCommand?: (text: string) => void;
  /** Put the keyboard back in the terminal. Called when a panel is dismissed
   *  deliberately (Escape, a choice, re-clicking the pill) or by clicking back
   *  into the terminal — never when the click went to other chrome, which owns
   *  its own focus. */
  onDismiss?: () => void;
  /** Whether the focused terminal has unsent text in its input line. A slash
   *  command typed into a non-empty prompt is appended to it and the whole
   *  thing is submitted as one message, so we ask before doing that. */
  hasPendingInput?: () => boolean;
  /** Ask the app to open claude's model picker and draw a dialog over it. */
  onOpenModelPicker?: () => void;
  /** Permission mode read off claude's status line; null when unreadable. */
  permMode: PermissionMode | null;
  /** Cycle the session to a mode with shift+tab. */
  onSetPermMode?: (m: PermissionMode) => Promise<void>;
}

export default function ComposerPills({
  sessionId, projectRoot, usage, usageAsOf, onCommand, onDismiss, hasPendingInput,
  onOpenModelPicker, permMode, onSetPermMode,
}: Props) {
  const bars = usage;
  const [open, setOpen] = useState<PanelKey | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [tasks, setTasks] = useState<SessionTask[]>([]);
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  const [agents, setAgents] = useState<AgentRun[]>([]);
  const [choice, setChoice] = useState<ModelChoice>({ model: null, effort: null, at: null });
  const [picks, setPicks] = useState<Picks>(loadPicks);
  /** An action held back because the prompt was not empty. */
  const [held, setHeld] = useState<{ label: string; send: () => void } | null>(null);
  const [permErr, setPermErr] = useState<string | null>(null);
  const pick = (sessionId && picks[sessionId]) || {};

  /** One exit path for every way a panel closes, so the keyboard always ends
   *  up in the same place. */
  const close = () => {
    setOpen(null);
    setHeld(null);
    onDismiss?.();
  };

  // A panel is an overlay covering part of the terminal, so it behaves like
  // one: Escape closes it, and so does a click anywhere outside it. Clicking
  // back into the terminal hands the keyboard back too — clicking other chrome
  // (sessions, git, the top bar) does not, because that click is the user
  // saying where focus should go, and stealing it back would fight them.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node | null;
      if (t && wrapRef.current?.contains(t)) return;
      setOpen(null);
      if (t instanceof Element && t.closest(".terminal-panel")) onDismiss?.();
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onDown, true);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onDown, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!sessionId) {
      setTasks([]);
      setArtifacts([]);
      setAgents([]);
      setChoice({ model: null, effort: null, at: null });
      return;
    }
    let stop = false;
    const poll = () => {
      sessionTasks(sessionId).then((t) => !stop && setTasks(t)).catch(() => {});
      sessionArtifacts(sessionId).then((a) => !stop && setArtifacts(a)).catch(() => {});
      sessionAgents(sessionId).then((a) => !stop && setAgents(a)).catch(() => {});
      sessionModel(sessionId).then((c) => !stop && setChoice(c)).catch(() => {});
    };
    poll();
    const iv = setInterval(poll, 5000);
    return () => { stop = true; clearInterval(iv); };
  }, [sessionId]);

  // Sending is fire-and-forget: claude writes the change into the transcript,
  // and the next poll reads it back. Nothing optimistic to get out of step —
  // if the command doesn't take, the pill keeps showing the truth.
  // A slash command is only a command when it starts the line. Typed into a
  // prompt you were halfway through writing, it becomes the tail of that
  // sentence and Enter sends the whole thing to the model. We cannot clear
  // claude's input for you — nothing we can send reliably empties it, and
  // guessing would throw away what you wrote — so ask instead.
  const guard = (label: string, send: () => void, force = false) => {
    if (!force && hasPendingInput?.()) {
      setHeld({ label, send });
      return;
    }
    setHeld(null);
    send();
  };

  /** Open claude's own picker; aiterm draws a dialog over it. */
  const openModelPicker = (force = false) =>
    guard("/model", () => { onOpenModelPicker?.(); close(); }, force);

  const run = (kind: "model" | "effort", value: string, force = false) =>
    guard(`/${kind} ${value}`, () => doRun(kind, value), force);

  const doRun = (kind: "model" | "effort", value: string) => {

    onCommand?.(`/${kind} ${value}`);

    if (sessionId) {
      setPicks((p) => {
        const next = {
          ...p,
          [sessionId]: { ...p[sessionId], [kind]: value, seenAt: choice.at },
        };
        try {
          localStorage.setItem(PICK_KEY, JSON.stringify(next));
        } catch { /* private mode / quota — the pill just won't persist */ }
        return next;
      });
    }
    close();
  };

  // A turn has run since the click, so the transcript has settled the question
  // and takes over from what was asked for.
  //
  // A pick with no `seenAt` was written before picks recorded when they were
  // made. There is no way to tell whether a turn has run since, so treat it as
  // settled and let the transcript win — the alternative leaves it pending for
  // ever, quietly overriding the truth until the pill is clicked again.
  const settled =
    !("seenAt" in pick) || choice.at !== pick.seenAt;
  const liveModel = choice.model ? shortModel(choice.model) : null;
  const pendingModel = settled ? null : pick.model ?? null;
  const pendingEffort = settled ? null : pick.effort ?? null;
  const shownModel = pendingModel ?? liveModel ?? "—";
  const shownEffort =
    pick.effort === "auto" ? "auto" : pendingEffort ?? choice.effort ?? "—";

  const done = tasks.filter((t) => t.status === "completed").length;
  const running = agents.filter((a) => a.status === "running").length;
  const session = bars.find((b) => b.kind === "session") ?? bars[0];

  const pill = (key: PanelKey, icon: string, label: string, count: string, extra = "") => (
    <button
      className={"cpill" + (open === key ? " on" : "") + (extra ? " " + extra : "")}
      onClick={() => (open === key ? close() : setOpen(key))}
      title={open === key ? `Hide ${label.toLowerCase()}` : `Show ${label.toLowerCase()}`}
    >
      <span className="cpill-icon">{icon}</span>
      <span className="cpill-label">{label}</span>
      {count && <span className="cpill-count">{count}</span>}
    </button>
  );

  return (
    <div className="cpill-wrap" ref={wrapRef}>
      {open === "tasks" && (
        <div className="cpill-panel">
          {tasks.length === 0 ? (
            <div className="empty-note">No tasks in this session yet</div>
          ) : (
            <>
              <div className="tasks-progress">
                <div className="tasks-progress-bar" style={{ width: `${(done / tasks.length) * 100}%` }} />
              </div>
              {tasks.map((t) => (
                <div key={t.id} className={"task-row " + t.status} title={t.subject}>
                  <span className={"task-icon" + (t.status === "completed" ? " done" : t.status === "in_progress" ? " busy" : "")}>
                    {t.status === "completed" ? "●" : t.status === "in_progress" ? "◐" : "○"}
                  </span>
                  <span className="task-subject">
                    {t.status === "in_progress" && t.active_form ? t.active_form : t.subject}
                  </span>
                </div>
              ))}
            </>
          )}
        </div>
      )}
      {open === "artifacts" && (
        <div className="cpill-panel">
          {artifacts.length === 0 ? (
            <div className="empty-note">No files written in this session yet</div>
          ) : (
            artifacts.map((a) => (
              <div
                key={a.path}
                className="task-row artifact-row"
                title={`${a.path} (${a.tool})`}
                onClick={() => openPath(a.path).catch(() => {})}
              >
                <span className={"artifact-tool " + a.tool.toLowerCase()}>
                  {a.tool === "Write" ? "W" : "E"}
                </span>
                <span className="task-subject">
                  {a.path.split("/").pop()}
                  <span className="artifact-dir"> {homeAbbrev(a.path).replace(/\/[^/]*$/, "")}</span>
                </span>
                <span className="artifact-time">{relTime(new Date(a.at).getTime())}</span>
              </div>
            ))
          )}
        </div>
      )}
      {open === "agents" && (
        <div className="cpill-panel">
          {agents.length === 0 ? (
            <div className="empty-note">No agents run in this session yet</div>
          ) : (
            // Running first, then finished newest-first — the same order the
            // right-hand panel uses, so the two never disagree on screen.
            [...agents.filter((a) => a.status === "running"),
             ...agents.filter((a) => a.status !== "running").reverse()].map((a) => (
              <div key={a.id} className="task-row" title={a.result ?? ""}>
                <span className={"agent-pill-dot" + (a.status === "running" ? " running" : "")} />
                <span className="artifact-tool">{a.agent_type}</span>
                <span className="task-subject">{a.description}</span>
              </div>
            ))
          )}
        </div>
      )}
      {open === "usage" && (
        <div className="cpill-panel">
          {bars.length === 0 ? (
            <div className="empty-note">No usage data — offline, or not signed in</div>
          ) : (
            bars.map((b, i) => (
              <div key={b.kind + i} className="cpill-usage-row">
                <div className="cpill-usage-head">
                  <span className="cpill-usage-label">{b.label}</span>
                  <span className="cpill-usage-pct">{Math.round(b.percent)}%</span>
                </div>
                <div className="usage-track">
                  <div
                    className={"usage-fill sev-" + b.severity}
                    style={{ width: Math.min(100, Math.max(0, b.percent)) + "%" }}
                  />
                </div>
                <span className="cpill-usage-reset">{resetsIn(b.resets_at)}</span>
              </div>
            ))
          )}
          {usageAsOf && bars.length > 0 && (
            <div className="cpill-choice-note">
              Last known reading, {relTime(usageAsOf)} — refreshing
            </div>
          )}
        </div>
      )}
      {open === "effort" && (
        <div className="cpill-panel cpill-choices">
          {held && (
            <div className="cpill-hold">
              <div className="cpill-hold-text">
                Your prompt has unsent text. <b>{held.label}</b> would be added to
                the end of it and the whole thing sent as a message.
                Send or clear what you typed first.
              </div>
              <div className="cpill-hold-acts">
                <button
                  className="cpill-hold-go"
                  onClick={() => { held.send(); setHeld(null); }}
                >Send it anyway</button>
                <button className="cpill-hold-no" onClick={() => setHeld(null)}>Cancel</button>
              </div>
            </div>
          )}
          {EFFORTS.map(([name, desc]) => {
            const selected = shownEffort === name;
            return (
              <button
                key={name}
                className={"cpill-choice" + (selected ? " on" : "")}
                onClick={() => run("effort", name)}
              >
                <span className="cpill-choice-name">
                  {name}
                  {selected && <span className="cpill-choice-mark">selected</span>}
                </span>
                <span className="cpill-choice-desc">{desc}</span>
              </button>
            );
          })}
          {/* With effort on auto every turn still records a concrete level, so
              say what it actually resolved to rather than implying `auto` is
              itself a setting the model ran at. */}
          {pick.effort === "auto" && choice.effort && (
            <div className="cpill-choice-note">
              Last turn ran at <b>{choice.effort}</b>
            </div>
          )}
          {/* Say plainly that this is a request until a turn proves it. A
              `/model` can be refused — sonnet answering "Kept model as Opus 5"
              — and the pill should not present that as settled. */}
          {pendingEffort && pick.effort !== "auto" && (
            <div className="cpill-choice-note">
              Asked for <b>{pendingEffort}</b> — the next turn will show what
              actually ran.
            </div>
          )}
        </div>
      )}
      {open === "perms" && (
        <div className="cpill-panel cpill-choices">
          {CYCLE_MODES.map((m) => (
            <button
              key={m}
              className={"cpill-choice" + (permMode === m ? " on" : "")}
              onClick={() => {
                setPermErr(null);
                onSetPermMode?.(m).catch((e) => setPermErr(String(e)));
                close();
              }}
            >
              <span className="cpill-choice-name">
                {m}
                {permMode === m && <span className="cpill-choice-mark">current</span>}
              </span>
              <span className="cpill-choice-desc">{PERM_DESC[m]}</span>
            </button>
          ))}
          {permErr && <div className="cpill-choice-note">{permErr}</div>}
          <div className="cpill-choice-note">
            Same as shift+tab in the terminal — this session only, nothing saved.
            {permMode === "auto" &&
              " Auto is not in the cycle: leaving it needs a new session to get back."}
          </div>
        </div>
      )}
      {open === "git" && (
        <div className="cpill-panel cpill-git">
          {/* The same component the right-hand panel uses. Reusing it means the
              two can never drift apart, and the pill costs no new git code. */}
          <GitPanel root={projectRoot} refreshKey={0} />
        </div>
      )}
      <div className="cpill-row">
        {onCommand && onOpenModelPicker && (
          <button
            className={"cpill" + (pendingModel ? " pending" : "")}
            onClick={() => openModelPicker()}
            title="Choose a model — opens claude's picker as a dialog"
          >
            <span className="cpill-icon">◆</span>
            <span className="cpill-label">Model</span>
            <span className="cpill-count">{shownModel}</span>
          </button>
        )}
        {onCommand && pill(
          "effort", "≡", "Effort", shownEffort,
          pendingEffort && pick.effort !== "auto" ? "pending" : "",
        )}
        {sessionId && pill("tasks", "◑", "Tasks", tasks.length ? `${done}/${tasks.length}` : "")}
        {sessionId && pill("artifacts", "▤", "Artifacts", artifacts.length ? `${artifacts.length}` : "")}
        {sessionId && pill(
          "agents", "✳", "Agents",
          agents.length ? (running ? `${running} active` : `${agents.length}`) : "",
          running ? "busy" : "",
        )}
        {session && pill("usage", "▮", "Usage", `${Math.round(session.percent)}%`, "sev-" + session.severity)}
        {onSetPermMode && permMode && pill(
          "perms", "⛨", "Perms", permMode,
          permMode === "bypass permissions" ? "sev-warning" : "",
        )}
        {projectRoot && pill("git", "⎇", "Git", "")}
      </div>
    </div>
  );
}
