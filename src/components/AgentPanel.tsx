import { useEffect, useState } from "react";
import { fmtTime, fullTime, useTimeFormat } from "../timefmt";
import {
  AgentRun, Artifact, SessionTask, homeAbbrev, openPath,
  sessionAgents, sessionArtifacts, sessionTasks,
} from "../ipc";
import Icon from "./Icon";
import { Ban, Circle, CircleCheck } from "lucide-react";

interface Props {
  /** Session id of the active terminal tab, when its engine records tasks
   *  and artifacts aiterm can read (`caps.tasks`). */
  sessionId: string | null;
  /** Open an artifact in a center file tab instead of the system app. */
  onOpenFile?: (path: string) => void;
}

function statusIcon(t: SessionTask) {
  if (t.status === "completed") return <span className="task-icon done"><Icon of={CircleCheck} size="sm" /></span>;
  if (t.status === "in_progress") return <span className="task-icon busy">◐</span>;
  if (t.blocked_by.length > 0) return <span className="task-icon blocked"><Icon of={Ban} size="sm" /></span>;
  return <span className="task-icon"><Icon of={Circle} size="sm" /></span>;
}

export default function AgentPanel({ sessionId, onOpenFile }: Props) {
  const { format: timeFormat } = useTimeFormat();
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
        >Artifacts{artifacts.length ? ` (${artifacts.length})` : ""}</button>
      </div>
      {tab === "tasks" ? (
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
      ) : artifacts.length === 0 ? (
        <div className="empty-note">No files written in this session yet</div>
      ) : (
        <div className="tasks-body">
          {artifacts.map((a) => (
            <div
              key={a.path}
              className="task-row artifact-row"
              title={`${a.path} (${a.tool})`}
              onClick={() =>
                onOpenFile ? onOpenFile(a.path) : openPath(a.path).catch(() => {})}
            >
              <span className={"artifact-tool " + a.tool.toLowerCase()}>
                {a.tool === "Write" ? "W" : "E"}
              </span>
              <span className="task-subject">
                {a.path.split("/").pop()}
                <span className="artifact-dir"> {homeAbbrev(a.path).replace(/\/[^/]*$/, "")}</span>
              </span>
              {a.at && (
                <span className="artifact-time" title={fullTime(new Date(a.at).getTime())}>{fmtTime(new Date(a.at).getTime(), timeFormat)}</span>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
