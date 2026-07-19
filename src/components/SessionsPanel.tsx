import { useMemo, useState } from "react";
import { Session, homeAbbrev, relTime } from "../ipc";

export interface SessionDisplayOpts {
  showPath: boolean;
  showBranch: boolean;
  showTime: boolean;
}

interface Props {
  sessions: Session[];
  activeProject: string | null;
  /** Slot ids that currently have a live terminal. */
  liveSlots: Set<string>;
  /** Slot id of the terminal currently displayed. */
  activeSlot: string | null;
  opts: SessionDisplayOpts;
  onOptsChange: (o: SessionDisplayOpts) => void;
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
  onNewShell: (s: Session) => void;
  onRefresh: () => void;
}

function AgentIcon({ agent }: { agent: string }) {
  if (agent === "claude") {
    // Claude "starburst" mark
    return (
      <svg className="agent-icon claude" viewBox="0 0 24 24" width="16" height="16">
        <g fill="currentColor">
          {Array.from({ length: 12 }).map((_, i) => (
            <rect key={i} x="11.1" y="2" width="1.8" height="7" rx="0.9"
              transform={`rotate(${i * 30} 12 12)`} />
          ))}
        </g>
      </svg>
    );
  }
  return (
    <svg className="agent-icon" viewBox="0 0 24 24" width="16" height="16" fill="none"
      stroke="currentColor" strokeWidth="2" strokeLinecap="round">
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M7 9l3 3-3 3M13 15h4" />
    </svg>
  );
}

export default function SessionsPanel({
  sessions, activeProject, liveSlots, activeSlot, opts,
  onOptsChange, onSelect, onResume, onNewShell, onRefresh,
}: Props) {
  const [query, setQuery] = useState("");
  const [showSettings, setShowSettings] = useState(false);

  const filtered = useMemo(() => {
    const q = query.toLowerCase();
    if (!q) return sessions;
    return sessions.filter(
      (s) =>
        s.title.toLowerCase().includes(q) ||
        s.project_path.toLowerCase().includes(q) ||
        (s.branch ?? "").toLowerCase().includes(q),
    );
  }, [sessions, query]);

  const toggle = (k: keyof SessionDisplayOpts) => onOptsChange({ ...opts, [k]: !opts[k] });

  return (
    <div className="sessions-panel">
      <div className="panel-toolbar">
        <input
          className="search-input"
          placeholder="Search sessions..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button className="icon-btn" title="Refresh" onClick={onRefresh}>⟳</button>
        <div className="settings-wrap">
          <button
            className={"icon-btn" + (showSettings ? " on" : "")}
            title="Display settings"
            onClick={() => setShowSettings(!showSettings)}
          >
            ⚙
          </button>
          {showSettings && (
            <div className="settings-pop">
              <label><input type="checkbox" checked={opts.showPath} onChange={() => toggle("showPath")} /> Project path</label>
              <label><input type="checkbox" checked={opts.showBranch} onChange={() => toggle("showBranch")} /> Git branch</label>
              <label><input type="checkbox" checked={opts.showTime} onChange={() => toggle("showTime")} /> Last active</label>
            </div>
          )}
        </div>
      </div>
      <div className="sessions-list">
        {filtered.map((s) => {
          const isLive = liveSlots.has(s.id) || liveSlots.has(`shell:${s.project_path}`);
          const isShowing = activeSlot !== null &&
            (activeSlot === s.id || activeSlot === `shell:${s.project_path}`);
          return (
          <div
            key={s.id}
            className={
              "session-item" +
              (s.project_path === activeProject ? " active" : "") +
              (isShowing ? " showing" : "")
            }
            onClick={() => onSelect(s)}
          >
            <AgentIcon agent={s.agent} />
            <div className="session-text">
              <div className="session-title">
                {isLive && <span className="live-dot" title="Terminal running" />}
                {s.title}
              </div>
              {opts.showPath && <div className="session-sub">{homeAbbrev(s.project_path)}</div>}
              <div className="session-meta">
                {opts.showBranch && s.branch && <span className="branch">⎇ {s.branch}</span>}
                {opts.showTime && <span className="time">{relTime(s.last_active)}</span>}
              </div>
            </div>
            <div className="session-actions">
              <button
                className="icon-btn" title="Resume session"
                onClick={(e) => { e.stopPropagation(); onResume(s); }}
              >▶</button>
              <button
                className="icon-btn" title="New shell here"
                onClick={(e) => { e.stopPropagation(); onNewShell(s); }}
              >＋</button>
            </div>
          </div>
          );
        })}
        {filtered.length === 0 && <div className="empty-note">No sessions found</div>}
      </div>
    </div>
  );
}
