import { useEffect, useState } from "react";
import { SessionTask, sessionTasks } from "../ipc";

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
  const [tasks, setTasks] = useState<SessionTask[]>([]);

  useEffect(() => {
    if (!sessionId) {
      setTasks([]);
      return;
    }
    let stop = false;
    const poll = () =>
      sessionTasks(sessionId)
        .then((t) => !stop && setTasks(t))
        .catch(() => {});
    poll();
    const iv = setInterval(poll, 4000);
    return () => {
      stop = true;
      clearInterval(iv);
    };
  }, [sessionId]);

  if (!sessionId) {
    return <div className="empty-note">Open a claude session to see its tasks</div>;
  }
  if (tasks.length === 0) {
    return <div className="empty-note">No tasks in this session yet</div>;
  }

  const done = tasks.filter((t) => t.status === "completed").length;

  return (
    <div className="tasks-body">
      <div className="tasks-progress">
        <div className="tasks-progress-bar" style={{ width: `${(done / tasks.length) * 100}%` }} />
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
  );
}
