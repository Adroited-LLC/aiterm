import { useEffect, useMemo, useRef, useState } from "react";
import { ProjectInfo, Session, homeAbbrev, searchSessions } from "../ipc";

/** Compact relative time for the row corner: "now", "5m", "3h", "2d". */
function shortTime(ms: number): string {
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 30 * 86400) return `${Math.floor(s / 86400)}d`;
  return `${Math.floor(s / (30 * 86400))}mo`;
}

type ViewMode = "recent" | "project" | "date";

function dateBucket(ms: number): string {
  const now = new Date();
  const startOfDay = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (ms >= startOfDay) return "Today";
  if (ms >= startOfDay - 86400_000) return "Yesterday";
  if (ms >= startOfDay - 6 * 86400_000) return "This week";
  if (ms >= startOfDay - 29 * 86400_000) return "This month";
  return "Older";
}

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
  projects: ProjectInfo[];
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
  onSelectProject: (p: ProjectInfo) => void;
  onProjectShell: (p: ProjectInfo) => void;
  onProjectClaude: (p: ProjectInfo) => void;
  onRefresh: () => void;
  tmuxAvailable: boolean;
  tmuxOn: boolean;
  onTmuxChange: (on: boolean) => void;
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
  sessions, projects, activeProject, liveSlots, activeSlot, opts,
  onOptsChange, onSelect, onResume, onNewShell,
  onSelectProject, onProjectShell, onProjectClaude, onRefresh,
  tmuxAvailable, tmuxOn, onTmuxChange,
}: Props) {
  const [query, setQuery] = useState("");
  const [showSettings, setShowSettings] = useState(false);
  const [viewMode, setViewMode] = useState<ViewMode>(
    () => (localStorage.getItem("aiterm.viewMode") as ViewMode) || "recent",
  );
  const [ftResults, setFtResults] = useState<Session[] | null>(null);

  useEffect(() => localStorage.setItem("aiterm.viewMode", viewMode), [viewMode]);

  // Debounced full-text search (tantivy index over titles + message text).
  useEffect(() => {
    if (!query.trim()) {
      setFtResults(null);
      return;
    }
    const t = setTimeout(() => {
      searchSessions(query).then(setFtResults).catch(() => setFtResults(null));
    }, 250);
    return () => clearTimeout(t);
  }, [query]);
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

  // When searching: ranked full-text hits first, then substring matches the
  // index may have missed (still catching up, etc).
  const searchList = useMemo(() => {
    if (!query.trim()) return null;
    const seen = new Set((ftResults ?? []).map((s) => s.id));
    return [...(ftResults ?? []), ...filtered.filter((s) => !seen.has(s.id))];
  }, [query, ftResults, filtered]);

  const grouped = useMemo(() => new Set(groups.flatMap((g) => g.members)), [groups]);
  const ungrouped = filtered.filter((s) => !grouped.has(s.project_path));

  const sessionPaths = useMemo(
    () => new Set(sessions.map((s) => s.project_path)),
    [sessions],
  );
  const sessionlessProjects = useMemo(() => {
    const q = query.toLowerCase();
    return projects.filter(
      (p) => !sessionPaths.has(p.path) && (!q || p.name.toLowerCase().includes(q)),
    );
  }, [projects, sessionPaths, query]);

  // Auto sub-grouping for the Project / Date view modes.
  const autoSections = useMemo(() => {
    if (viewMode === "recent" || searchList) return null;
    const map = new Map<string, Session[]>();
    for (const s of filtered) {
      const key = viewMode === "project"
        ? s.project_path
        : dateBucket(s.last_active);
      (map.get(key) ?? map.set(key, []).get(key)!).push(s);
    }
    if (viewMode === "date") {
      const order = ["Today", "Yesterday", "This week", "This month", "Older"];
      return order
        .filter((k) => map.has(k))
        .map((k) => ({ label: k, sessions: map.get(k)! }));
    }
    return [...map.entries()]
      .sort((a, b) => b[1][0].last_active - a[1][0].last_active)
      .map(([path, ss]) => ({
        label: path.split("/").filter(Boolean).pop() ?? path,
        sessions: ss,
      }));
  }, [viewMode, filtered, searchList]);

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
          if (e.button !== 0 || viewMode !== "recent" || searchList) return;
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
        <div className={"agent-badge" + (s.agent === "claude" ? " claude" : "")}>
          <AgentIcon agent={s.agent} />
          {isLive && <span className="live-dot badge-dot" title="Terminal running" />}
        </div>
        <div className="session-text">
          <div className="session-title-row">
            <span className="session-title">{s.title}</span>
            {opts.showTime && <span className="session-time">{shortTime(s.last_active)}</span>}
          </div>
          {(opts.showPath || (opts.showBranch && s.branch)) && (
            <div className="session-meta">
              {opts.showPath && (
                <span className="session-sub">{homeAbbrev(s.project_path)}</span>
              )}
              {opts.showBranch && s.branch && (
                <span className="branch">
                  <svg viewBox="0 0 16 16" width="9" height="9" fill="currentColor">
                    <path d="M9.5 3.25a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.128a2.251 2.251 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.5 0v1.836A2.493 2.493 0 0 1 6 7h4a1 1 0 0 0 1-1v-.628a2.25 2.25 0 0 1-1.5-2.122z" />
                  </svg>
                  {s.branch}
                </span>
              )}
            </div>
          )}
        </div>
        <div className="session-actions">
          <button
            className="act-btn" title="Resume claude session"
            onClick={(e) => { e.stopPropagation(); onResume(s); }}
          >▶</button>
          <button
            className="act-btn" title="New shell here"
            onClick={(e) => { e.stopPropagation(); onNewShell(s); }}
          >＋</button>
        </div>
      </div>
    );
  };

  const renderProject = (p: ProjectInfo) => {
    const isLive = liveSlots.has(`claude:${p.path}`) || liveSlots.has(`shell:${p.path}`);
    const isShowing = activeSlot === `claude:${p.path}` || activeSlot === `shell:${p.path}`;
    return (
      <div
        key={p.path}
        className={
          "session-item project-item" +
          (p.path === activeProject ? " active" : "") +
          (isShowing ? " showing" : "")
        }
        onClick={() => onSelectProject(p)}
      >
        <div className="agent-badge folder">
          <svg className="agent-icon" viewBox="0 0 24 24" width="15" height="15" fill="none"
            stroke="currentColor" strokeWidth="2">
            <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z" />
          </svg>
          {isLive && <span className="live-dot badge-dot" title="Terminal running" />}
        </div>
        <div className="session-text">
          <div className="session-title-row">
            <span className="session-title">{p.name}</span>
          </div>
          {opts.showPath && (
            <div className="session-meta">
              <span className="session-sub">{homeAbbrev(p.path)}</span>
            </div>
          )}
        </div>
        <div className="session-actions">
          <button
            className="act-btn" title="Start claude here"
            onClick={(e) => { e.stopPropagation(); onProjectClaude(p); }}
          >▶</button>
          <button
            className="act-btn" title="New shell here"
            onClick={(e) => { e.stopPropagation(); onProjectShell(p); }}
          >＋</button>
        </div>
      </div>
    );
  };

  return (
    <div className="sessions-panel">
      <div className="panel-toolbar">
        <div className="search-box">
          <svg className="search-icon" viewBox="0 0 16 16" width="12" height="12"
            fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
            <circle cx="7" cy="7" r="4.5" />
            <path d="M10.5 10.5L14 14" />
          </svg>
          <input
            className="search-input"
            placeholder="Search sessions…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQuery("")}
          />
          {query && (
            <button className="search-clear" title="Clear" onClick={() => setQuery("")}>✕</button>
          )}
        </div>
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
              <div className="settings-sep" />
              <label title={tmuxAvailable
                ? "New terminals run inside tmux and survive app restarts"
                : "tmux is not installed"}>
                <input
                  type="checkbox"
                  disabled={!tmuxAvailable}
                  checked={tmuxAvailable && tmuxOn}
                  onChange={() => onTmuxChange(!tmuxOn)}
                /> Persistent terminals
              </label>
            </div>
          )}
        </div>
      </div>
      <div className="view-tabs">
        {(["recent", "project", "date"] as ViewMode[]).map((m) => (
          <button
            key={m}
            className={"view-tab" + (viewMode === m ? " on" : "")}
            onClick={() => setViewMode(m)}
          >
            {m === "recent" ? "Recent" : m === "project" ? "Project" : "Date"}
          </button>
        ))}
      </div>
      <div className="sessions-list">
        {searchList ? (
          <>
            {searchList.map(renderItem)}
            {searchList.length === 0 && sessionlessProjects.length === 0 && (
              <div className="empty-note">No matches</div>
            )}
          </>
        ) : autoSections ? (
          autoSections.map((sec) => (
            <div key={sec.label} className="session-group">
              <div className="group-header static">
                <span className="group-name">{sec.label}</span>
                <span className="group-count">{sec.sessions.length}</span>
              </div>
              {sec.sessions.map(renderItem)}
            </div>
          ))
        ) : (
          <>
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
          </>
        )}
        {sessionlessProjects.length > 0 && (
          <div className="session-group">
            <div className="group-header static projects-header">
              <span className="group-name">PROJECTS</span>
              <span className="group-count">{sessionlessProjects.length}</span>
            </div>
            {sessionlessProjects.map(renderProject)}
          </div>
        )}
      </div>
      {dragId && dragPos && (
        <div className="drag-ghost" style={{ left: dragPos.x + 12, top: dragPos.y + 8 }}>
          {dragId.split("/").filter(Boolean).pop()}
        </div>
      )}
    </div>
  );
}
