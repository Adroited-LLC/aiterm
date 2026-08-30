import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { fmtTime, fullTime, useTimeFormat } from "../timefmt";
import {
  AgentRun, Artifact, Change, SessionTask, homeAbbrev, openPath,
  isImagePath, isVideoPath, readFileBase64,
  sessionAgents, sessionArtifacts, sessionChanges, sessionTasks,
} from "../ipc";
import Icon from "./Icon";
import { Ban, Circle, CircleCheck } from "lucide-react";

interface Props {
  /** Session id of the active terminal tab, when its engine records tasks
   *  and artifacts aiterm can read (`caps.tasks`). */
  sessionId: string | null;
  /** The active tab's session for the Changes tab — every engine, since the
   *  filesystem watcher does not care which one wrote the file. */
  changesSessionId?: string | null;
  /** Open an artifact in a center file tab instead of the system app. */
  onOpenFile?: (path: string) => void;
}

/** A thumbnail for an image the agent made, read on demand. */
function Thumb({ path }: { path: string }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let stop = false;
    readFileBase64(path).then((f) => !stop && setSrc(`data:${f.mime};base64,${f.data}`)).catch(() => {});
    return () => { stop = true; };
  }, [path]);
  return src ? <img className="change-thumb" src={src} alt="" /> : <span className="change-thumb blank" />;
}

function ChangesList({ sessionId, extra, onOpenFile }: { sessionId: string; extra?: Artifact[]; onOpenFile?: (path: string) => void }) {
  const { format: timeFormat } = useTimeFormat();
  const [changes, setChanges] = useState<Change[]>([]);
  const [big, setBig] = useState<string | null>(null);
  useEffect(() => {
    let stop = false;
    const load = () => sessionChanges(sessionId).then((c) => !stop && setChanges(c)).catch(() => {});
    load();
    const un = listen<Change>("changes://file", (e) => { if (e.payload.session_id === sessionId) load(); });
    return () => { stop = true; un.then((f) => f()); };
  }, [sessionId]);
  // One list for everything the session produced: the filesystem's word
  // (the ledger, plus harness output read live) and, folded in, files the
  // transcript says it wrote that the watcher never saw.
  const rows: Change[] = [
    ...changes,
    ...(extra ?? [])
      .filter((a) => !changes.some((c) => c.path === a.path))
      .map((a) => ({
        path: a.path,
        name: a.path.split("/").pop() ?? a.path,
        kind: a.tool === "Write" ? "created" : "modified",
        at: Math.floor(Date.parse(a.at) / 1000) || 0,
        session_id: sessionId,
        bytes: 0,
      })),
  ].sort((x, y) => y.at - x.at);
  if (rows.length === 0) {
    return <div className="empty-note">Nothing produced in this session yet. Files it creates or edits — any engine, any tool — show up here as they land.</div>;
  }
  return (
    <div className="tasks-body">
      {rows.map((c) => (
        <div
          key={c.path}
          className={"task-row artifact-row change-row " + c.kind}
          title={`${c.path} — ${c.kind} ${fullTime(c.at * 1000)}`}
          onClick={() => {
            if (c.kind === "deleted") return;
            if (isImagePath(c.path) || isVideoPath(c.path)) setBig(big === c.path ? null : c.path);
            else if (onOpenFile) onOpenFile(c.path);
            else openPath(c.path).catch(() => {});
          }}
        >
          {isImagePath(c.path) && c.kind !== "deleted" ? <Thumb path={c.path} /> : (
            <span className={"artifact-tool " + (c.kind === "created" ? "write" : c.kind === "deleted" ? "gone" : "edit")}>
              {c.kind === "created" ? "+" : c.kind === "deleted" ? "×" : "~"}
            </span>
          )}
          <span className="task-subject">
            {c.name}
            <span className="change-meta"> · {fmtTime(c.at * 1000, timeFormat)} · {homeAbbrev(c.path.slice(0, c.path.lastIndexOf("/")))}</span>
          </span>
        </div>
      ))}
      {big && (
        <div className="change-big" onClick={() => setBig(null)}>
          {isVideoPath(big) ? <BigVideo path={big} /> : <BigImage path={big} />}
          <div className="change-big-actions">
            <button className="act-btn" onClick={(e) => { e.stopPropagation(); openPath(big).catch(() => {}); }}>Open in app</button>
          </div>
        </div>
      )}
    </div>
  );
}

function BigImage({ path }: { path: string }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => { readFileBase64(path).then((f) => setSrc(`data:${f.mime};base64,${f.data}`)).catch(() => {}); }, [path]);
  return src ? <img src={src} alt="" /> : <span className="empty-note">Loading…</span>;
}

function BigVideo({ path }: { path: string }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => { readFileBase64(path).then((f) => setSrc(`data:${f.mime};base64,${f.data}`)).catch(() => {}); }, [path]);
  return src ? <video src={src} controls autoPlay /> : <span className="empty-note">Loading…</span>;
}

function statusIcon(t: SessionTask) {
  if (t.status === "completed") return <span className="task-icon done"><Icon of={CircleCheck} size="sm" /></span>;
  if (t.status === "in_progress") return <span className="task-icon busy">◐</span>;
  if (t.blocked_by.length > 0) return <span className="task-icon blocked"><Icon of={Ban} size="sm" /></span>;
  return <span className="task-icon"><Icon of={Circle} size="sm" /></span>;
}

export default function AgentPanel({ sessionId, changesSessionId, onOpenFile }: Props) {
  const [tab, setTab] = useState<"tasks" | "artifacts">("tasks");
  const [tasks, setTasks] = useState<SessionTask[]>([]);
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  const [agents, setAgents] = useState<AgentRun[]>([]);

  useEffect(() => {
    if (!sessionId) {
      setTasks([]);
      setArtifacts([]);
      setAgents([]);
      return;
    }
    let stop = false;
    const poll = () => {
      sessionTasks(sessionId).then((t) => !stop && setTasks(t)).catch(() => {});
      sessionArtifacts(sessionId).then((a) => !stop && setArtifacts(a)).catch(() => {});
      sessionAgents(sessionId).then((a) => !stop && setAgents(a)).catch(() => {});
    };
    poll();
    const iv = setInterval(poll, 4000);
    return () => {
      stop = true;
      clearInterval(iv);
    };
  }, [sessionId]);

  if (!sessionId) {
    // No task-recording engine, but the filesystem still has a word.
    if (changesSessionId) {
      return (
        <>
          <div className="git-tabs agent-tabs">
            <button className="git-tab on">Changes</button>
          </div>
          <ChangesList sessionId={changesSessionId} onOpenFile={onOpenFile} />
        </>
      );
    }
    return (
      <div className="empty-note">
        Tasks and artifacts appear here for engines that record them —
        Claude Code, Grok, and Codex today.
      </div>
    );
  }

  const done = tasks.filter((t) => t.status === "completed").length;
  const running = agents.filter((a) => a.status === "running").length;
  // Running agents lead; finished ones follow, newest first.
  const pills = [
    ...agents.filter((a) => a.status === "running"),
    ...agents.filter((a) => a.status !== "running").reverse(),
  ];

  return (
    <div className="agent-body">
      {agents.length > 0 && (
        <div className="agent-pills">
          {pills.map((a) => (
            <span
              key={a.id}
              className={"agent-pill" + (a.status === "running" ? " running" : "")}
              title={
                `${a.agent_type} — ${a.description}` +
                (a.result ? `\n\n${a.result}` : a.status === "running" ? "\n\nworking…" : "")
              }
            >
              <span className="agent-pill-dot" />
              <span className="agent-pill-type">{a.agent_type}</span>
              <span className="agent-pill-desc">{a.description}</span>
            </span>
          ))}
          {running > 0 && (
            <span className="agent-pill-count">{running} active</span>
          )}
        </div>
      )}
      <div className="git-tabs agent-tabs">
        <button
          className={"git-tab" + (tab === "tasks" ? " on" : "")}
          onClick={() => setTab("tasks")}
        >Tasks{tasks.length ? ` (${done}/${tasks.length})` : ""}</button>
        <button
          className={"git-tab" + (tab === "artifacts" ? " on" : "")}
          onClick={() => setTab("artifacts")}
        >Artifacts</button>
      </div>
      {tab !== "tasks" ? (
        <ChangesList sessionId={changesSessionId ?? sessionId} extra={artifacts} onOpenFile={onOpenFile} />
      ) : (
        tasks.length === 0 ? (
          <div className="empty-note">No tasks in this session yet</div>
        ) : (
          <div className="tasks-body">
            <div className="tasks-progress">
              <div
                className="tasks-progress-bar"
                style={{ width: `${(done / tasks.length) * 100}%` }}
              />
            </div>
            {tasks.map((t) => (
              <div key={t.id} className={"task-row " + t.status} title={t.subject}>
                {statusIcon(t)}
                <span className="task-subject">
                  {t.status === "in_progress" && t.active_form ? t.active_form : t.subject}
                </span>
              </div>
            ))}
          </div>
        )
      )}
    </div>
  );
}
