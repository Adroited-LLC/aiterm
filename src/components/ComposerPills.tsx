import { useEffect, useState } from "react";
import {
  AgentRun, Artifact, ModelChoice, SessionTask, UsageBar,
  homeAbbrev, openPath, relTime,
  sessionAgents, sessionArtifacts, sessionModel, sessionTasks, usageLimits,
} from "../ipc";

/** `/model <name>` choices, straight from the command's own usage line. */
const MODELS = ["default", "best", "opus", "opusplan", "sonnet", "haiku", "fable"];
/** `/effort <level>` choices, likewise. */
const EFFORTS = ["auto", "low", "medium", "high", "xhigh", "max", "ultracode"];

/** "claude-opus-5" → "opus". Falls back to the raw id when unrecognised, so a
 *  new model shows *something* rather than going blank. */
function shortModel(id: string | null): string {
  if (!id) return "—";
  const m = MODELS.find((n) => n !== "default" && n !== "best" && id.includes(n));
  return m ?? id.replace(/^claude-/, "");
}

/**
 * The composer's control strip: a row of counts that open a panel when clicked.
 *
 * The composer has always been collapsed by default, because as a text input it
 * is redundant — claude draws its own. This is the part that earns the space:
 * what the session is doing, visible without giving up a panel to it, and one
 * click from the detail.
 *
 * Deliberately self-contained. It reads the same commands the right-hand panels
 * read and owns no state they own, so it can be removed by deleting this file
 * and one line in Composer — nothing that currently works depends on it.
 */

type PanelKey = "tasks" | "artifacts" | "agents" | "usage" | "model" | "effort";

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
  /** Run a slash command in the live terminal (adds Enter). Absent when no
   *  terminal is focused, which is what disables the model/effort pills. */
  onCommand?: (text: string) => void;
}

export default function ComposerPills({ sessionId, onCommand }: Props) {
  const [open, setOpen] = useState<PanelKey | null>(null);
  const [tasks, setTasks] = useState<SessionTask[]>([]);
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  const [agents, setAgents] = useState<AgentRun[]>([]);
  const [bars, setBars] = useState<UsageBar[]>([]);
  const [choice, setChoice] = useState<ModelChoice>({ model: null, effort: null });

  useEffect(() => {
    if (!sessionId) {
      setTasks([]);
      setArtifacts([]);
      setAgents([]);
      setChoice({ model: null, effort: null });
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
  const run = (cmd: string) => {
    onCommand?.(cmd);
    setOpen(null);
  };

  useEffect(() => {
    let alive = true;
    // Same rule as the top-bar gauges: keep the last good reading. A transient
    // failure returns [], and replacing the value with that makes the pill
    // blink out for a whole interval.
    const load = () => usageLimits().then((b) => alive && b.length && setBars(b)).catch(() => {});
    load();
    const iv = setInterval(load, 60_000);
    return () => { alive = false; clearInterval(iv); };
  }, []);

  const done = tasks.filter((t) => t.status === "completed").length;
  const running = agents.filter((a) => a.status === "running").length;
  const session = bars.find((b) => b.kind === "session") ?? bars[0];

  const pill = (key: PanelKey, icon: string, label: string, count: string, extra = "") => (
    <button
      className={"cpill" + (open === key ? " on" : "") + (extra ? " " + extra : "")}
      onClick={() => setOpen((o) => (o === key ? null : key))}
      title={open === key ? `Hide ${label.toLowerCase()}` : `Show ${label.toLowerCase()}`}
    >
      <span className="cpill-icon">{icon}</span>
      <span className="cpill-label">{label}</span>
      {count && <span className="cpill-count">{count}</span>}
    </button>
  );

  return (
    <div className="cpill-wrap">
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
                <span className="cpill-usage-label">{b.label}</span>
                <div className="usage-track">
                  <div
                    className={"usage-fill sev-" + b.severity}
                    style={{ width: Math.min(100, Math.max(0, b.percent)) + "%" }}
                  />
                </div>
                <span className="cpill-usage-pct">{Math.round(b.percent)}%</span>
                <span className="cpill-usage-reset">{resetsIn(b.resets_at)}</span>
              </div>
            ))
          )}
        </div>
      )}
      {(open === "model" || open === "effort") && (
        <div className="cpill-panel cpill-choices">
          {(open === "model" ? MODELS : EFFORTS).map((name) => {
            const current = open === "model"
              ? shortModel(choice.model) === name
              : choice.effort === name;
            return (
              <button
                key={name}
                className={"cpill-choice" + (current ? " on" : "")}
                onClick={() => run(`/${open} ${name}`)}
              >
                {name}
                {current && <span className="cpill-choice-mark">current</span>}
              </button>
            );
          })}
        </div>
      )}
      <div className="cpill-row">
        {onCommand && pill("model", "◆", "Model", shortModel(choice.model))}
        {onCommand && pill("effort", "≡", "Effort", choice.effort ?? "—")}
        {sessionId && pill("tasks", "◑", "Tasks", tasks.length ? `${done}/${tasks.length}` : "")}
        {sessionId && pill("artifacts", "▤", "Files", artifacts.length ? `${artifacts.length}` : "")}
        {sessionId && pill(
          "agents", "✳", "Agents",
          agents.length ? (running ? `${running} active` : `${agents.length}`) : "",
          running ? "busy" : "",
        )}
        {session && pill("usage", "▮", "Usage", `${Math.round(session.percent)}%`, "sev-" + session.severity)}
      </div>
    </div>
  );
}
