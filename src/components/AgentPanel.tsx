import { useEffect, useState } from "react";
import {
  Artifact, SessionTask, homeAbbrev, openPath, relTime, sessionArtifacts, sessionTasks,
} from "../ipc";

interface Props {
  /** Claude session id of the active terminal tab, if it's a claude tab. */
  sessionId: string | null;
}

function statusIcon(t: SessionTask) {
  if (t.status === "completed") return <span className="task-icon done">●</span>;
  if (t.status === "in_progress") return <span className="task-icon busy">◐</span>;
  if (t.blocked_by.length > 0) return <span className="task-icon blocked">⊘</span>;
  return <span className="task-icon">○</span>;
}

export default function AgentPanel({ sessionId }: Props) {
  const [tab, setTab] = useState<"tasks" | "artifacts">("tasks");
  const [tasks, setTasks] = useState<SessionTask[]>([]);
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);

  useEffect(() => {
    if (!sessionId) {
      setTasks([]);
      setArtifacts([]);
      return;
    }
    let stop = false;
    const poll = () => {
      sessionTasks(sessionId).then((t) => !stop && setTasks(t)).catch(() => {});
      sessionArtifacts(sessionId).then((a) => !stop && setArtifacts(a)).catch(() => {});
    };
    poll();
    const iv = setInterval(poll, 4000);
    return () => {
      stop = true;
      clearInterval(iv);
    };
  }, [sessionId]);

  if (!sessionId) {
    return <div className="empty-note">Open a claude session to see its tasks and artifacts</div>;
  }

  const done = tasks.filter((t) => t.status === "completed").length;

  return (
    <div className="agent-body">
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
          ))}
        </div>
      )}
    </div>
  );
}
