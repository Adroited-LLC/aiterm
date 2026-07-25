import { useEffect, useMemo, useRef, useState } from "react";
import { ProjectInfo, Session, TrashedSession, homeAbbrev, searchSessions } from "../ipc";

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
const ORDER_KEY = "aiterm.sessionOrder";
const PALETTE = ["#61afef", "#98c379", "#e5c07b", "#e06c75", "#c678dd", "#56b6c2", "#da7756"];

function loadGroups(): Group[] {
  try {
    return JSON.parse(localStorage.getItem(GROUPS_KEY) ?? "[]");
  } catch {
    return [];
  }
}

/** Manual drag-rearranged order per container ("recent" or "group:<id>"). */
function loadOrders(): Record<string, string[]> {
  try {
    return JSON.parse(localStorage.getItem(ORDER_KEY) ?? "{}");
  } catch {
    return {};
  }
}

/** Items the user hand-ordered keep that order; ones not ordered yet
 *  (new arrivals) stay on top in their natural order. */
function orderBy<T>(list: T[], id: (t: T) => string, order?: string[]): T[] {
  if (!order || order.length === 0) return list;
  const idx = new Map(order.map((k, i) => [k, i]));
  const fresh = list.filter((t) => !idx.has(id(t)));
  const ordered = list
    .filter((t) => idx.has(id(t)))
    .sort((a, b) => idx.get(id(a))! - idx.get(id(b))!);
  return [...fresh, ...ordered];
}
const applyOrder = (list: Session[], order?: string[]) =>
  orderBy(list, (s) => s.id, order);

interface DragArm {
  /** session = a row; group = a custom group header (recent view);
   *  section = an auto-section header (project view). */
  kind: "session" | "group" | "section";
  /** Session id, group id, or section key. */
  id: string;
  /** Session's project path (session drags — group membership drops). */
  path: string;
  /** Container the session was dragged from ("recent", "group:<id>",
   *  or "sec:<view>:<key>"). */
  container: string;
  label: string;
  x: number;
  y: number;
}

interface Props {
  sessions: Session[];
  projects: ProjectInfo[];
  activeProject: string | null;
  /** Slot ids aiterm has a terminal tab open for. Tab ownership only — NOT
   *  the same as the session being alive, and after a background-mode resume
   *  the two name different rows. */
  liveSlots: Set<string>;
  /** Session ids the Claude Code daemon is actually running right now. */
  liveSessions: Set<string>;
  /** Slot ids whose terminal rang the bell (claude waiting on input). */
  attentionSlots: Set<string>;
  /** Slot id of the terminal currently displayed. */
  activeSlot: string | null;
  opts: SessionDisplayOpts;
  onOptsChange: (o: SessionDisplayOpts) => void;
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
  onFork: (s: Session) => void;
  onExit: (s: Session) => void;
  onNewShell: (s: Session) => void;
  onDelete: (s: Session) => void;
  onSelectProject: (p: ProjectInfo) => void;
  onProjectShell: (p: ProjectInfo) => void;
  onProjectClaude: (p: ProjectInfo) => void;
  onRefresh: () => void;
  trashed: TrashedSession[];
  onRestore: (id: string) => void;
  onTrashDelete: (id: string) => void;
  onTrashEmpty: () => void;
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
  sessions, projects, activeProject, liveSlots, liveSessions, attentionSlots, activeSlot, opts,
  onOptsChange, onSelect, onResume, onFork, onExit, onNewShell, onDelete,
  onSelectProject, onProjectShell, onProjectClaude, onRefresh,
  trashed, onRestore, onTrashDelete, onTrashEmpty,
}: Props) {
  const [query, setQuery] = useState("");
  const [confirmDel, setConfirmDel] = useState<string | null>(null);
  // Folded auto-sections (Recent list, Project/Date buckets, PROJECTS),
  // keyed "<viewMode>:<label>" so each view remembers its own folds.
  const [collapsedSections, setCollapsedSections] = useState<Set<string>>(() => {
    try {
      return new Set(JSON.parse(localStorage.getItem("aiterm.collapsedSections") ?? "[]"));
    } catch {
      return new Set();
    }
  });
  useEffect(
    () => localStorage.setItem("aiterm.collapsedSections", JSON.stringify([...collapsedSections])),
    [collapsedSections],
  );
  const toggleSection = (key: string) =>
    setCollapsedSections((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  // Searching always shows everything regardless of folds.
  const sectionOpen = (key: string) => !collapsedSections.has(key) || query.trim().length > 0;
  const [emptyConfirm, setEmptyConfirm] = useState(false);
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
  const [orders, setOrders] = useState<Record<string, string[]>>(loadOrders);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [dragId, setDragId] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState<string | null>(null);
  const [dragOverItem, setDragOverItem] = useState<string | null>(null);
  const [dragOverAfter, setDragOverAfter] = useState(false);
  const [dragPos, setDragPos] = useState<{ x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameText, setRenameText] = useState("");
  // Pointer-based drag: HTML5 DnD never fires in webkit2gtk on Wayland.
  const dragArm = useRef<DragArm | null>(null);
  const dragActive = useRef(false);
  const suppressClick = useRef(false);
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  // Displayed id sequence per container, read by the drop handler.
  const containerSeq = useRef<Map<string, string[]>>(new Map());

  useEffect(() => localStorage.setItem(GROUPS_KEY, JSON.stringify(groups)), [groups]);
  useEffect(() => localStorage.setItem(ORDER_KEY, JSON.stringify(orders)), [orders]);

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

  // Group membership keys off `group_path`, not the raw cwd: a `/fork` can run
  // its agent in a `<repo>/.claude/worktrees/<name>` checkout, which would
  // otherwise strand the fork in a group of its own instead of listing it
  // beside the session it branched from. Rows still spawn in `project_path`.
  const grouped = useMemo(() => new Set(groups.flatMap((g) => g.members)), [groups]);
  const ungrouped = useMemo(
    () => applyOrder(filtered.filter((s) => !grouped.has(s.group_path)), orders["recent"]),
    [filtered, grouped, orders],
  );
  const groupMembers = useMemo(() => {
    const m = new Map<string, Session[]>();
    for (const g of groups) {
      m.set(
        g.id,
        applyOrder(
          filtered.filter((s) => g.members.includes(s.group_path)),
          orders[`group:${g.id}`],
        ),
      );
    }
    return m;
  }, [groups, filtered, orders]);
  containerSeq.current = new Map([
    ["recent", ungrouped.map((s) => s.id)],
    ...groups.map(
      (g) => [`group:${g.id}`, (groupMembers.get(g.id) ?? []).map((s) => s.id)] as const,
    ),
  ]);

  const sessionPaths = useMemo(
    () => new Set(sessions.map((s) => s.group_path)),
    [sessions],
  );
  const sessionlessProjects = useMemo(() => {
    const q = query.toLowerCase();
    return projects.filter(
      (p) => !sessionPaths.has(p.path) && (!q || p.name.toLowerCase().includes(q)),
    );
  }, [projects, sessionPaths, query]);

  // Auto sub-grouping for the Project / Date view modes. Project sections
  // drag-reorder by their header; date buckets stay chronological.
  const autoSections = useMemo(() => {
    if (viewMode === "recent" || searchList) return null;
    const map = new Map<string, Session[]>();
    for (const s of filtered) {
      const key = viewMode === "project"
        ? s.group_path
        : dateBucket(s.last_active);
      (map.get(key) ?? map.set(key, []).get(key)!).push(s);
    }
    if (viewMode === "date") {
      const order = ["Today", "Yesterday", "This week", "This month", "Older"];
      return order
        .filter((k) => map.has(k))
        .map((k) => ({ key: k, label: k, sessions: map.get(k)! }));
    }
    const secs = [...map.entries()]
      .sort((a, b) => b[1][0].last_active - a[1][0].last_active)
      .map(([path, ss]) => ({
        key: path,
        label: path.split("/").filter(Boolean).pop() ?? path,
        sessions: ss,
      }));
    return orderBy(secs, (s) => s.key, orders["sections:project"]);
  }, [viewMode, filtered, searchList, orders]);
  if (viewMode === "project" && autoSections) {
    containerSeq.current.set("sections:project", autoSections.map((s) => s.key));
  }

  const toggle = (k: keyof SessionDisplayOpts) => onOptsChange({ ...opts, [k]: !opts[k] });

  const filteredTrash = useMemo(() => {
    const q = query.toLowerCase();
    if (!q) return trashed;
    return trashed.filter(
      (t) => t.title.toLowerCase().includes(q) || t.project_path.toLowerCase().includes(q),
    );
  }, [trashed, query]);

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
        setDragId(arm.id);
      }
      setDragPos({ x: e.clientX, y: e.clientY });
      const el = document.elementFromPoint(e.clientX, e.clientY);
      setDragOver(
        arm.kind === "section"
          ? el?.closest<HTMLElement>("[data-secwrap]")?.dataset.secwrap ?? null
          : el?.closest<HTMLElement>("[data-drop]")?.dataset.drop ?? null,
      );
      // Hovering a sibling row shows the insertion line (reorder).
      const itemEl = el?.closest<HTMLElement>("[data-item]");
      const hoverId =
        arm.kind === "session" &&
        itemEl?.dataset.container === arm.container &&
        itemEl.dataset.item !== arm.id
          ? itemEl.dataset.item ?? null
          : null;
      setDragOverItem(hoverId);
      if (hoverId) {
        const seq = containerSeq.current.get(arm.container) ?? [];
        setDragOverAfter(seq.indexOf(arm.id) < seq.indexOf(hoverId));
      }
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
        const itemEl = el?.closest<HTMLElement>("[data-item]");
        const target = el?.closest<HTMLElement>("[data-drop]")?.dataset.drop;
        if (arm.kind === "section") {
          // Project-view sections reorder — dropping anywhere on another
          // section (header or its rows) targets that section.
          const sec = el?.closest<HTMLElement>("[data-secwrap]")?.dataset.secwrap;
          if (sec && sec !== arm.id) {
            const seq = containerSeq.current.get("sections:project") ?? [];
            const rest = seq.filter((k) => k !== arm.id);
            const after = seq.indexOf(arm.id) < seq.indexOf(sec);
            rest.splice(rest.indexOf(sec) + (after ? 1 : 0), 0, arm.id);
            setOrders((o) => ({ ...o, "sections:project": rest }));
          }
        } else if (arm.kind === "group") {
          // Dropping a group header on another header reorders the groups.
          if (target && target !== arm.id) {
            setGroups((gs) => {
              const from = gs.findIndex((g) => g.id === arm.id);
              const to = gs.findIndex((g) => g.id === target);
              if (from < 0 || to < 0) return gs;
              const next = [...gs];
              const [moved] = next.splice(from, 1);
              next.splice(to, 0, moved);
              return next;
            });
          }
        } else if (
          itemEl?.dataset.container === arm.container &&
          itemEl.dataset.item &&
          itemEl.dataset.item !== arm.id
        ) {
          // Dropped on a sibling row: rearrange within the container. A drag
          // of a selected row carries the whole selection along.
          const targetId = itemEl.dataset.item;
          const seq = containerSeq.current.get(arm.container) ?? [];
          const sel = selectedRef.current;
          const moving = sel.has(arm.id)
            ? seq.filter((id) => sel.has(id))
            : [arm.id];
          if (!moving.includes(targetId)) {
            const rest = seq.filter((id) => !moving.includes(id));
            const after = seq.indexOf(arm.id) < seq.indexOf(targetId);
            rest.splice(rest.indexOf(targetId) + (after ? 1 : 0), 0, ...moving);
            setOrders((o) => ({ ...o, [arm.container]: rest }));
          }
        } else if (target === "new") createGroup(arm.path);
        else if (target === "ungrouped") moveToGroup(null, arm.path);
        else if (target) moveToGroup(target, arm.path);
      }
      dragArm.current = null;
      dragActive.current = false;
      setDragId(null);
      setDragOver(null);
      setDragOverItem(null);
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

  // Every foldable section key visible in the current view, for the
  // collapse/expand-all toolbar button.
  const sectionKeys = useMemo(() => {
    const keys: string[] = ["projects", "trash"];
    if (viewMode === "recent") keys.push("recent:all");
    else if (autoSections) keys.push(...autoSections.map((s) => `${viewMode}:${s.label}`));
    return keys;
  }, [viewMode, autoSections]);
  const anyOpen =
    sectionKeys.some((k) => !collapsedSections.has(k)) ||
    (viewMode === "recent" && groups.some((g) => !g.collapsed));
  const toggleAll = () => {
    if (anyOpen) {
      setCollapsedSections((prev) => new Set([...prev, ...sectionKeys]));
      if (viewMode === "recent") {
        setGroups((gs) => gs.map((g) => ({ ...g, collapsed: true })));
      }
    } else {
      setCollapsedSections((prev) => {
        const next = new Set(prev);
        sectionKeys.forEach((k) => next.delete(k));
        return next;
      });
      if (viewMode === "recent") {
        setGroups((gs) => gs.map((g) => ({ ...g, collapsed: false })));
      }
    }
  };

  const renderTrash = (t: TrashedSession) => (
    <div key={t.id} className="session-item trash-item">
      <div className="agent-badge trash-badge">
        <svg viewBox="0 0 16 16" width="13" height="13" fill="none"
          stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
          <path d="M2.5 4h11M6.5 4V2.5h3V4M4 4l.8 9.5a1 1 0 0 0 1 .9h4.4a1 1 0 0 0 1-.9L12 4M6.5 7v4.5M9.5 7v4.5" />
        </svg>
      </div>
      <div className="session-text">
        <div className="session-title-row">
          <span className="session-title">{t.title}</span>
          <span className="session-time">{shortTime(t.deleted_at)}</span>
        </div>
        <div className="session-meta">
          <span className="session-sub">
            {t.project_path ? homeAbbrev(t.project_path) : "unknown project"}
          </span>
        </div>
      </div>
      <div
        className={"session-actions" + (confirmDel === `trash:${t.id}` ? " confirming" : "")}
        onClick={(e) => e.stopPropagation()}
      >
        {confirmDel === `trash:${t.id}` ? (
          <>
            <span className="confirm-label">Delete forever?</span>
            <button
              className="act-btn danger"
              onClick={() => { setConfirmDel(null); onTrashDelete(t.id); }}
            >Delete</button>
            <button className="act-btn" onClick={() => setConfirmDel(null)}>Cancel</button>
          </>
        ) : (
          <>
            <button
              className="act-btn" title="Restore session"
              onClick={() => onRestore(t.id)}
            >↩</button>
            <button
              className="act-btn danger" title="Delete forever…"
              onClick={() => setConfirmDel(`trash:${t.id}`)}
            >✕</button>
          </>
        )}
      </div>
    </div>
  );

  const renderItem = (s: Session, container?: string) => {
    // Two different facts, deliberately kept apart. `hasTab` is whether aiterm
    // has a terminal open for this row; `isRunning` is whether the session is
    // alive at all. They were one value, which put the live badge — and the
    // fork/exit menu, and the absence of Delete — on whichever row the tab
    // happened to name. After a background-mode resume that's the frozen
    // parent, while the real conversation runs elsewhere wearing no badge and
    // offering a Delete that would truncate it.
    const hasTab = liveSlots.has(s.id) || liveSlots.has(`shell:${s.project_path}`);
    const isRunning = liveSessions.has(s.id);
    const hasAttn =
      attentionSlots.has(s.id) || attentionSlots.has(`shell:${s.project_path}`);
    const isShowing = activeSlot !== null &&
      (activeSlot === s.id || activeSlot === `shell:${s.project_path}`);
    const isDragging =
      dragId === s.id || (dragId !== null && dragActive.current && selected.has(dragId) && selected.has(s.id));
    return (
      <div
        key={s.id}
        data-item={container ? s.id : undefined}
        data-container={container}
        onPointerDown={(e) => {
          if (e.button !== 0 || !container || searchList) return;
          if ((e.target as HTMLElement).closest("button")) return;
          dragArm.current = {
            // group_path, matching how membership is read below — dragging a
            // worktree session must group the repo it belongs to, not the
            // throwaway checkout path, or the drop silently does nothing.
            kind: "session", id: s.id, path: s.group_path,
            container, label: s.title, x: e.clientX, y: e.clientY,
          };
        }}
        className={
          "session-item" +
          // Only the single focused session tints — not every session that
          // happens to share the active project. Multi-highlight is opt-in
          // via ctrl/shift-click (builds the `selected` set below).
          (isShowing ? " active" : "") +
          (selected.has(s.id) ? " selected" : "") +
          (isShowing ? " showing" : "") +
          (isDragging ? " dragging" : "") +
          (dragOverItem === s.id ? " drop-mark" + (dragOverAfter ? " drop-after" : "") : "")
        }
        onClick={(e) => {
          if (suppressClick.current) {
            suppressClick.current = false;
            return;
          }
          if (e.ctrlKey || e.metaKey || e.shiftKey) {
            // Ctrl/Shift-click builds a multi-selection without switching
            // sessions; a plain click selects just this one.
            setSelected((prev) => {
              const next = new Set(prev);
              if (next.has(s.id)) next.delete(s.id);
              else next.add(s.id);
              return next;
            });
            return;
          }
          setSelected(new Set([s.id]));
          onSelect(s);
        }}
      >
        <div className={"agent-badge" + (s.agent === "claude" ? " claude" : "")}>
          <AgentIcon agent={s.agent} />
          {/* Green: the session is running. Hollow: aiterm has a tab for it.
              Usually the same row — but not after a background-mode resume,
              and when they differ you need to see both. */}
          {(isRunning || hasAttn) && (
            <span
              className={"live-dot badge-dot" + (hasAttn ? " attn" : "")}
              title={hasAttn ? "Waiting for your input" : "Session is running"}
            />
          )}
          {hasTab && (
            <span className="tab-dot badge-dot" title="Open in a tab here" />
          )}
        </div>
        <div className="session-text">
          <div className="session-title-row">
            {s.forked && (
              <span
                className="fork-mark"
                title={
                  s.background
                    ? "Forked from another session — carries its context up to the fork"
                    : "Continues an earlier session past a compaction"
                }
              >
                ⑂
              </span>
            )}
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
        <div
          className={"session-actions" + (confirmDel === s.id ? " confirming" : "")}
          onClick={(e) => e.stopPropagation()}
        >
          {confirmDel === s.id ? (
            <>
              <span className="confirm-label">Delete session?</span>
              <button
                className="act-btn danger"
                title="Move this session's transcript and task data to ~/.claude/trash (kept 7 days)"
                onClick={() => { setConfirmDel(null); onDelete(s); }}
              >Delete</button>
              <button
                className="act-btn" title="Keep it"
                onClick={() => setConfirmDel(null)}
              >Cancel</button>
            </>
          ) : (
            <>
              {/* Each action is offered only when it can actually do
                  something. Branching needs a session; exiting needs a tab to
                  close; resuming needs the session NOT to be running already.
                  These used to hang off one flag, so a frozen snapshot got
                  Fork/Exit (branching stale history, closing a tab that wasn't
                  driving the conversation) while the running session got
                  Resume. */}
              {isRunning && (
                <button
                  className="act-btn" title="Fork into a new tab (branch a copy)"
                  onClick={() => onFork(s)}
                >⑂</button>
              )}
              {hasTab && (
                <button
                  className="act-btn" title="Exit — close this tab"
                  onClick={() => onExit(s)}
                >⏻</button>
              )}
              {!isRunning && !hasTab && (
                <button
                  className="act-btn" title="Resume claude session"
                  onClick={() => onResume(s)}
                >▶</button>
              )}
              <button
                className="act-btn" title="New shell here"
                onClick={() => onNewShell(s)}
              >＋</button>
              {/* Never offer Delete on a running session: trashing one doesn't
                  stick — the process recreates its transcript seconds later,
                  rebuilt from the deletion point, losing the history before
                  it. Stop it first. */}
              {!isRunning && !hasTab && (
                <button
                  className="act-btn danger" title="Delete session…"
                  onClick={() => setConfirmDel(s.id)}
                >
                  <svg viewBox="0 0 16 16" width="11" height="11" fill="none"
                    stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                    <path d="M2.5 4h11M6.5 4V2.5h3V4M4 4l.8 9.5a1 1 0 0 0 1 .9h4.4a1 1 0 0 0 1-.9L12 4M6.5 7v4.5M9.5 7v4.5" />
                  </svg>
                </button>
              )}
            </>
          )}
        </div>
      </div>
    );
  };

  const renderProject = (p: ProjectInfo) => {
    const isLive = liveSlots.has(`claude:${p.path}`) || liveSlots.has(`shell:${p.path}`);
    const hasAttn =
      attentionSlots.has(`claude:${p.path}`) || attentionSlots.has(`shell:${p.path}`);
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
          {(isLive || hasAttn) && (
            <span
              className={"live-dot badge-dot" + (hasAttn ? " attn" : "")}
              title={hasAttn ? "Waiting for your input" : "Terminal running"}
            />
          )}
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
        <button
          className="icon-btn"
          title={anyOpen ? "Collapse all" : "Expand all"}
          onClick={toggleAll}
        >
          <svg viewBox="0 0 16 16" width="12" height="12" fill="none"
            stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
            {anyOpen ? (
              <path d="M4 9.5L8 6l4 3.5M4 13.5L8 10l4 3.5" transform="translate(0 -1.5)" />
            ) : (
              <path d="M4 3l4 3.5L12 3M4 8l4 3.5L12 8" transform="translate(0 1)" />
            )}
          </svg>
        </button>
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
            {searchList.map((s) => renderItem(s))}
            {searchList.length === 0 && sessionlessProjects.length === 0 && (
              <div className="empty-note">No matches</div>
            )}
          </>
        ) : autoSections ? (
          autoSections.map((sec) => {
            const key = `${viewMode}:${sec.label}`;
            const open = sectionOpen(key);
            const draggable = viewMode === "project";
            const secKeys = autoSections.map((x) => x.key);
            const isTarget =
              dragArm.current?.kind === "section" &&
              dragId !== null && dragId !== sec.key && dragOver === sec.key;
            const targetCls = isTarget
              ? secKeys.indexOf(dragId!) < secKeys.indexOf(sec.key)
                ? " sec-drop-after"
                : " sec-drop-before"
              : "";
            return (
              <div
                key={sec.key}
                className={"session-group" + targetCls}
                data-secwrap={draggable ? sec.key : undefined}
              >
                <div
                  className={
                    "group-header static clickable" +
                    (dragId === sec.key ? " dragging" : "")
                  }
                  onPointerDown={(e) => {
                    if (e.button !== 0 || !draggable) return;
                    if ((e.target as HTMLElement).closest("button, input")) return;
                    dragArm.current = {
                      kind: "section", id: sec.key, path: "", container: "",
                      label: sec.label, x: e.clientX, y: e.clientY,
                    };
                  }}
                  onClick={() => {
                    if (suppressClick.current) {
                      suppressClick.current = false;
                      return;
                    }
                    toggleSection(key);
                  }}
                >
                  <span className={"chevron" + (open ? " open" : "")}>›</span>
                  <span className="group-name">{sec.label}</span>
                  <span className="group-count">{sec.sessions.length}</span>
                </div>
                {open && sec.sessions.map((s) => renderItem(s))}
              </div>
            );
          })
        ) : (
          <>
        {dragId && dragArm.current?.kind === "session" && (
          <div
            className={"drop-zone" + (dragOver === "new" ? " over" : "")}
            data-drop="new"
          >
            ＋ Drop here to create a group
          </div>
        )}
        {groups.map((g) => {
          const members = groupMembers.get(g.id) ?? [];
          if (members.length === 0 && query) return null;
          const open = !g.collapsed || query.length > 0;
          return (
            <div key={g.id} className="session-group">
              <div
                className={
                  "group-header" +
                  (dragOver === g.id ? " over" : "") +
                  (dragId === g.id ? " dragging" : "")
                }
                data-drop={g.id}
                onPointerDown={(e) => {
                  if (e.button !== 0) return;
                  if ((e.target as HTMLElement).closest("button, input")) return;
                  dragArm.current = {
                    kind: "group", id: g.id, path: "", container: "",
                    label: g.name, x: e.clientX, y: e.clientY,
                  };
                }}
                onClick={() => {
                  if (suppressClick.current) {
                    suppressClick.current = false;
                    return;
                  }
                  setGroups((gs) =>
                    gs.map((x) => (x.id === g.id ? { ...x, collapsed: !x.collapsed } : x)));
                }}
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
              {open && members.map((s) => renderItem(s, `group:${g.id}`))}
            </div>
          );
        })}
        <div className="session-group">
          <div
            className="group-header static clickable"
            onClick={() => toggleSection("recent:all")}
          >
            <span className={"chevron" + (sectionOpen("recent:all") ? " open" : "")}>›</span>
            <span className="group-name">Recent</span>
            <span className="group-count">{ungrouped.length}</span>
          </div>
          <div
            className={"ungrouped" + (dragOver === "ungrouped" ? " over" : "")}
            data-drop="ungrouped"
          >
            {sectionOpen("recent:all") && ungrouped.map((s) => renderItem(s, "recent"))}
            {filtered.length === 0 && <div className="empty-note">No sessions found</div>}
          </div>
        </div>
          </>
        )}
        {sessionlessProjects.length > 0 && (
          <div className="session-group">
            <div
              className="group-header static clickable projects-header"
              onClick={() => toggleSection("projects")}
            >
              <span className={"chevron" + (sectionOpen("projects") ? " open" : "")}>›</span>
              <span className="group-name">Projects</span>
              <span className="group-count">{sessionlessProjects.length}</span>
            </div>
            {sectionOpen("projects") && sessionlessProjects.map(renderProject)}
          </div>
        )}
        {filteredTrash.length > 0 && (
          <div className="session-group trash-group">
            <div
              className="group-header static clickable"
              onClick={() => toggleSection("trash")}
            >
              <span className={"chevron" + (sectionOpen("trash") ? " open" : "")}>›</span>
              <span className="group-name">Trash</span>
              <span className="group-count">{filteredTrash.length}</span>
              {emptyConfirm ? (
                <span className="trash-empty-confirm" onClick={(e) => e.stopPropagation()}>
                  <span className="confirm-label">Empty trash?</span>
                  <button
                    className="act-btn danger"
                    onClick={() => { setEmptyConfirm(false); onTrashEmpty(); }}
                  >Empty</button>
                  <button className="act-btn" onClick={() => setEmptyConfirm(false)}>Cancel</button>
                </span>
              ) : (
                <button
                  className="icon-btn group-del"
                  title="Empty trash…"
                  onClick={(e) => { e.stopPropagation(); setEmptyConfirm(true); }}
                >✕</button>
              )}
            </div>
            {sectionOpen("trash") && filteredTrash.map(renderTrash)}
          </div>
        )}
      </div>
      {dragId && dragPos && (
        <div className="drag-ghost" style={{ left: dragPos.x + 12, top: dragPos.y + 8 }}>
          {dragArm.current?.label ?? ""}
        </div>
      )}
    </div>
  );
}
