import { useEffect, useMemo, useRef, useState } from "react";
import { Session, homeAbbrev, relTime } from "../ipc";

export interface SessionDisplayOpts {
  showPath: boolean;
  showBranch: boolean;
  showTime: boolean;
}

interface Group {
  id: string;
  name: string;
  color: string;
  collapsed: boolean;
  /** Project paths — a group holds projects, and every session of a member
   *  project shows under it. */
  members: string[];
}

const GROUPS_KEY = "aiterm.projectGroups";
const PALETTE = ["#61afef", "#98c379", "#e5c07b", "#e06c75", "#c678dd", "#56b6c2", "#da7756"];

function loadGroups(): Group[] {
  try {
    return JSON.parse(localStorage.getItem(GROUPS_KEY) ?? "[]");
  } catch {
    return [];
  }
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
  const [groups, setGroups] = useState<Group[]>(loadGroups);
  const [dragId, setDragId] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState<string | null>(null);
  const [dragPos, setDragPos] = useState<{ x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameText, setRenameText] = useState("");
  // Pointer-based drag: HTML5 DnD never fires in webkit2gtk on Wayland.
  const dragArm = useRef<{ path: string; x: number; y: number } | null>(null);
  const dragActive = useRef(false);
  const suppressClick = useRef(false);

  useEffect(() => localStorage.setItem(GROUPS_KEY, JSON.stringify(groups)), [groups]);

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

  const grouped = useMemo(() => new Set(groups.flatMap((g) => g.members)), [groups]);
  const ungrouped = filtered.filter((s) => !grouped.has(s.project_path));

  const toggle = (k: keyof SessionDisplayOpts) => onOptsChange({ ...opts, [k]: !opts[k] });

  const moveToGroup = (groupId: string | null, projectPath: string) => {
    setGroups((gs) =>
      gs.map((g) => ({
        ...g,
        members:
          g.id === groupId
            ? [...g.members.filter((m) => m !== projectPath), projectPath]
            : g.members.filter((m) => m !== projectPath),
      })),
    );
  };

  const createGroup = (projectPath: string) => {
    setGroups((gs) => [
      ...gs.map((g) => ({ ...g, members: g.members.filter((m) => m !== projectPath) })),
      {
        id: crypto.randomUUID(),
        name: projectPath.split("/").filter(Boolean).pop() ?? `Group ${gs.length + 1}`,
        color: PALETTE[gs.length % PALETTE.length],
        collapsed: false,
        members: [projectPath],
      },
    ]);
  };

  const deleteGroup = (id: string) =>
    setGroups((gs) => gs.filter((g) => g.id !== id));

  const cycleColor = (id: string) =>
    setGroups((gs) =>
      gs.map((g) =>
        g.id === id
          ? { ...g, color: PALETTE[(PALETTE.indexOf(g.color) + 1) % PALETTE.length] }
          : g,
      ),
    );

  const commitRename = () => {
    if (renaming) {
      const name = renameText.trim();
      if (name) {
        setGroups((gs) => gs.map((g) => (g.id === renaming ? { ...g, name } : g)));
      }
    }
    setRenaming(null);
  };

  // Global pointer tracking while a drag is armed/active.
  useEffect(() => {
    const move = (e: PointerEvent) => {
      const arm = dragArm.current;
      if (!arm) return;
      if (!dragActive.current) {
        if (Math.abs(e.clientX - arm.x) + Math.abs(e.clientY - arm.y) < 7) return;
        dragActive.current = true;
        setDragId(arm.path);
      }
      setDragPos({ x: e.clientX, y: e.clientY });
      const el = document.elementFromPoint(e.clientX, e.clientY);
      setDragOver(el?.closest<HTMLElement>("[data-drop]")?.dataset.drop ?? null);
    };
    const up = (e: PointerEvent) => {
      const arm = dragArm.current;
      if (!arm) return;
      if (dragActive.current) {
        suppressClick.current = true;
        // The suppressed click (if any) dispatches synchronously after
        // pointerup; clear the flag so the next real click works.
        setTimeout(() => { suppressClick.current = false; }, 0);
        const el = document.elementFromPoint(e.clientX, e.clientY);
        const target = el?.closest<HTMLElement>("[data-drop]")?.dataset.drop;
        if (target === "new") createGroup(arm.path);
        else if (target === "ungrouped") moveToGroup(null, arm.path);
        else if (target) moveToGroup(target, arm.path);
      }
      dragArm.current = null;
      dragActive.current = false;
      setDragId(null);
      setDragOver(null);
      setDragPos(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const renderItem = (s: Session) => {
    const isLive = liveSlots.has(s.id) || liveSlots.has(`shell:${s.project_path}`);
    const isShowing = activeSlot !== null &&
      (activeSlot === s.id || activeSlot === `shell:${s.project_path}`);
    return (
      <div
        key={s.id}
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          if ((e.target as HTMLElement).closest("button")) return;
          dragArm.current = { path: s.project_path, x: e.clientX, y: e.clientY };
        }}
        className={
          "session-item" +
          (s.project_path === activeProject ? " active" : "") +
          (isShowing ? " showing" : "") +
          (dragId === s.project_path ? " dragging" : "")
        }
        onClick={() => {
          if (suppressClick.current) {
            suppressClick.current = false;
            return;
          }
          onSelect(s);
        }}
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
  };

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
        {dragId && (
          <div
            className={"drop-zone" + (dragOver === "new" ? " over" : "")}
            data-drop="new"
          >
            ＋ Drop here to create a group
          </div>
        )}
        {groups.map((g) => {
          const members = filtered.filter((s) => g.members.includes(s.project_path));
          if (members.length === 0 && query) return null;
          const open = !g.collapsed || query.length > 0;
          return (
            <div key={g.id} className="session-group">
              <div
                className={"group-header" + (dragOver === g.id ? " over" : "")}
                data-drop={g.id}
                onClick={() => setGroups((gs) =>
                  gs.map((x) => (x.id === g.id ? { ...x, collapsed: !x.collapsed } : x)))}
              >
                <span className={"chevron" + (open ? " open" : "")}>›</span>
                <span
                  className="group-dot"
                  style={{ background: g.color }}
                  title="Change color"
                  onClick={(e) => { e.stopPropagation(); cycleColor(g.id); }}
                />
                {renaming === g.id ? (
                  <input
                    className="group-rename"
                    autoFocus
                    value={renameText}
                    onChange={(e) => setRenameText(e.target.value)}
                    onBlur={commitRename}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") commitRename();
                      if (e.key === "Escape") setRenaming(null);
                    }}
                    onClick={(e) => e.stopPropagation()}
                  />
                ) : (
                  <span
                    className="group-name"
                    title="Double-click to rename"
                    onDoubleClick={(e) => {
                      e.stopPropagation();
                      setRenaming(g.id);
                      setRenameText(g.name);
                    }}
                  >{g.name}</span>
                )}
                <span className="group-count">{members.length}</span>
                <button
                  className="icon-btn group-del"
                  title="Ungroup (items go back to the main list)"
                  onClick={(e) => { e.stopPropagation(); deleteGroup(g.id); }}
                >✕</button>
              </div>
              {open && members.map(renderItem)}
            </div>
          );
        })}
        <div
          className={"ungrouped" + (dragOver === "ungrouped" ? " over" : "")}
          data-drop="ungrouped"
        >
          {ungrouped.map(renderItem)}
          {filtered.length === 0 && <div className="empty-note">No sessions found</div>}
        </div>
      </div>
      {dragId && dragPos && (
        <div className="drag-ghost" style={{ left: dragPos.x + 12, top: dragPos.y + 8 }}>
          {dragId.split("/").filter(Boolean).pop()}
        </div>
      )}
    </div>
  );
}
