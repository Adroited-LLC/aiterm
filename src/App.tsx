import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import SessionsPanel, { SessionDisplayOpts } from "./components/SessionsPanel";
import TerminalView, { TermHandle, TermTab } from "./components/TerminalView";
import FileExplorer from "./components/FileExplorer";
import GitPanel from "./components/GitPanel";
import Composer from "./components/Composer";
import AgentPanel from "./components/AgentPanel";
import SettingsModal from "./components/SettingsModal";
import SessionPreview from "./components/SessionPreview";
import { UsageBars } from "./components/UsageBars";
import { Clock } from "./components/Clock";
import {
  AppSettings, applySettings, loadSettings, saveSettings, termFontFamily, termTheme,
} from "./settings";
import {
  ProjectInfo, Session, SessionStatus,
  TrashedSession,
  gitRepoState, listProjects, listSessions, reindexSessions,
  resolveResumableId, runningSessionIds, bgAgentSessionIds,
  sessionDelete, sessionStatus, trashDelete, trashEmpty, trashList, trashRestore,
  watchProject,
} from "./ipc";
import "./App.css";

const OPTS_KEY = "aiterm.sessionOpts";
const SIZES_KEY = "aiterm.panelSizes";
const FONT_KEY = "aiterm.fontScale";
const SUPERSEDED_KEY = "aiterm.superseded";

function loadSuperseded(): Set<string> {
  try {
    const raw = localStorage.getItem(SUPERSEDED_KEY);
    return new Set(raw ? (JSON.parse(raw) as string[]) : []);
  } catch {
    return new Set();
  }
}

interface PanelSizes {
  left: number;
  right: number;
  explorerFrac: number;
  /** Height fraction of the right column taken by the agent (tasks) panel. */
  agentFrac: number;
}

function loadJSON<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? { ...fallback, ...JSON.parse(raw) } : fallback;
  } catch {
    return fallback;
  }
}

export default function App() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeProject, setActiveProject] = useState<string | null>(null);
  // Transient bottom toast (e.g. a resume with nothing resumable left).
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 6000);
    return () => clearTimeout(t);
  }, [notice]);
  const [tabs, setTabs] = useState<TermTab[]>([]);
  const [activeTab, setActiveTab] = useState<number | null>(null);
  const [previewSession, setPreviewSession] = useState<Session | null>(null);
  const nextKey = useRef(1);
  const [gitRefresh, setGitRefresh] = useState(0);
  const [explorerRefresh, setExplorerRefresh] = useState(0);

  // Tabs whose terminal rang the bell while not being looked at.
  const [attention, setAttention] = useState<Set<number>>(new Set());
  const activeTabRef = useRef<number | null>(null);
  // Latest tabs, read by the [sessions] supersession effect without putting
  // `tabs` in its deps (which would re-run it on every tab change).
  const tabsRef = useRef<TermTab[]>(tabs);
  tabsRef.current = tabs;

  // Sessions hidden because a focused-tab `/clear` spawned a fresh sibling
  // that superseded them. Persisted; purely a hide + tab-rebind (reversible).
  const [superseded, setSuperseded] = useState<Set<string>>(loadSuperseded);
  useEffect(() => {
    localStorage.setItem(SUPERSEDED_KEY, JSON.stringify([...superseded]));
  }, [superseded]);
  // Undo affordance for the most recent supersession.
  const [undoInfo, setUndoInfo] = useState<{ id: string; title: string } | null>(null);
  useEffect(() => {
    if (!undoInfo) return;
    const t = setTimeout(() => setUndoInfo(null), 8000);
    return () => clearTimeout(t);
  }, [undoInfo]);
  // Previous scan's session ids, to detect newly-appeared continuations.
  const knownIdsRef = useRef<Set<string> | null>(null);

  const handles = useRef<Map<number, TermHandle>>(new Map());
  const lastOutput = useRef<Map<number, number>>(new Map());
  const [working, setWorking] = useState(false);
  const [claudeStatus, setClaudeStatus] = useState<SessionStatus | null>(null);
  const [branch, setBranch] = useState<string | null>(null);

  const [showSessions, setShowSessions] = useState(true);
  const [showExplorer, setShowExplorer] = useState(true);
  const [showGit, setShowGit] = useState(true);
  // Collapsed by default: claude draws its own input bar, so the composer is opt-in.
  const [showComposer, setShowComposer] = useState(false);
  const [showAgent, setShowAgent] = useState(true);

  const [opts, setOpts] = useState<SessionDisplayOpts>(() =>
    loadJSON(OPTS_KEY, { showPath: true, showBranch: true, showTime: true }),
  );
  const [sizes, setSizes] = useState<PanelSizes>(() =>
    loadJSON(SIZES_KEY, { left: 280, right: 560, explorerFrac: 0.5, agentFrac: 0.3 }),
  );

  const [fontScale, setFontScale] = useState<number>(
    () => loadJSON(FONT_KEY, { scale: 1 }).scale,
  );

  const [settings, setSettings] = useState<AppSettings>(loadSettings);
  const [showSettingsModal, setShowSettingsModal] = useState(false);
  useEffect(() => {
    applySettings(settings);
    saveSettings(settings);
  }, [settings]);
  useEffect(() => {
    if (!showSettingsModal) return;
    const h = (e: KeyboardEvent) => e.key === "Escape" && setShowSettingsModal(false);
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [showSettingsModal]);

  useEffect(() => localStorage.setItem(OPTS_KEY, JSON.stringify(opts)), [opts]);
  useEffect(() => localStorage.setItem(SIZES_KEY, JSON.stringify(sizes)), [sizes]);
  useEffect(() => localStorage.setItem(FONT_KEY, JSON.stringify({ scale: fontScale })), [fontScale]);

  const bumpFont = useCallback((dir: 1 | -1 | 0) => {
    setFontScale((s) =>
      dir === 0 ? 1 : Math.max(0.7, Math.min(1.6, +(s + dir * 0.1).toFixed(2))),
    );
  }, []);

  // Ctrl+= / Ctrl+- / Ctrl+0 font zoom, captured before xterm sees the keys.
  // Ctrl+Shift+L: force a clean repaint of the active terminal.
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.altKey || e.metaKey) return;
      if (e.shiftKey && (e.key === "L" || e.key === "l")) {
        e.preventDefault();
        const key = activeTabRef.current;
        if (key !== null) handles.current.get(key)?.redraw();
        return;
      }
      if (e.shiftKey) return;
      if (e.key === "=" || e.key === "+") { e.preventDefault(); bumpFont(1); }
      else if (e.key === "-") { e.preventDefault(); bumpFont(-1); }
      else if (e.key === "0") { e.preventDefault(); bumpFont(0); }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [bumpFont]);

  const termFont = Math.round(settings.termFontSize * fontScale);
  const xtermTheme = useMemo(() => termTheme(settings), [settings]);
  const xtermFont = useMemo(() => termFontFamily(settings), [settings]);
  const zoomFor = (panel: keyof AppSettings["panelScale"]): React.CSSProperties =>
    ({ zoom: fontScale * settings.panelScale[panel] } as React.CSSProperties);

  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [trashed, setTrashed] = useState<TrashedSession[]>([]);

  // List-only refresh: cheap, safe to run on every fs event.
  const refreshSessionList = useCallback(() => {
    listSessions().then(setSessions).catch(console.error);
    listProjects().then(setProjects).catch(console.error);
    trashList().then(setTrashed).catch(() => setTrashed([]));
  }, []);
  const refreshSessions = useCallback(() => {
    refreshSessionList();
    // Keep the full-text index warm in the background (30s poll only).
    reindexSessions().catch(() => {});
  }, [refreshSessionList]);
  useEffect(() => {
    refreshSessions();
    const iv = setInterval(refreshSessions, 30_000);
    return () => clearInterval(iv);
  }, [refreshSessions]);
  // Event-driven refresh: Claude's transcripts changed (backend debounces).
  useEffect(() => {
    const un = listen("sessions://changed", () => {
      refreshSessionList();
    });
    return () => {
      un.then((f) => f());
    };
  }, [refreshSessionList]);

  // Detect a focused-tab `/clear`: it writes a brand-new, fully unlinked
  // session in the same folder (new id, no bridge/fork link), so the disk-based
  // fork-collapse can't catch it. We catch it live: when a session that *newly
  // appeared this scan* is a strict continuation of the focused resume tab's
  // session, hide the old row and rebind the tab to the new session. The six
  // guards below jointly identify exactly this case — do not weaken them.
  useEffect(() => {
    const cur = new Set(sessions.map((s) => s.id));
    const known = knownIdsRef.current;
    knownIdsRef.current = cur;

    // Light pruning: forget hidden ids no longer present on disk.
    setSuperseded((prev) => {
      let changed = false;
      const next = new Set(prev);
      for (const id of prev) {
        if (!cur.has(id)) { next.delete(id); changed = true; }
      }
      return changed ? next : prev;
    });

    if (known === null) return; // (6) baseline on first load — never supersede
    const fresh = sessions.filter((s) => !known.has(s.id)); // (1) newly-appeared
    if (fresh.length === 0) return;

    // (2) focused tab only.
    const tab = tabsRef.current.find((t) => t.key === activeTabRef.current) ?? null;
    if (!tab) return;
    const sid = tab.sessionId;
    // (5) never steal another open tab's session.
    const otherSlots = new Set(
      tabsRef.current.filter((t) => t.key !== tab.key).map((t) => t.slotId),
    );

    // A fork tab (slot `fork:<id>:<n>`, opened by forkSession) has no listed
    // session row until `--fork-session` writes its new transcript. Adopt the
    // newest fresh session in the tab's project that no other tab holds. The
    // parent row can't be adopted by accident — it already existed, so it's
    // never in `fresh` — and it stays listed: forking leaves the parent
    // intact, frozen at its own context, independently resumable.
    if (tab.slotId.startsWith("fork:")) {
      const adopt = fresh
        .filter((s) => s.project_path === tab.cwd && !otherSlots.has(s.id))
        .reduce<Session | null>((a, b) => (!a || b.last_active > a.last_active ? b : a), null);
      if (!adopt) return;
      setTabs((ts) =>
        ts.map((t) => (t.key === tab.key ? { ...t, sessionId: adopt.id, slotId: adopt.id } : t)),
      );
      return;
    }

    // Otherwise: a real Claude resume tab (session-id slot) whose live session
    // was replaced by a continuation (compact/fork on resume).
    if (!sid || sid.startsWith("shell:") || sid.startsWith("claude:")) return;
    // (4) the tab's current session row must still exist to compare against.
    const curRow = sessions.find((s) => s.id === sid);
    if (!curRow) return;
    const candidates = fresh.filter(
      (s) =>
        s.project_path === tab.cwd &&           // (3) same project as the tab
        s.id !== sid &&                          // (4) strictly a different session
        s.last_active >= curRow.last_active &&   // (4) the continuation, not older
        !otherSlots.has(s.id),                   // (5) not another open tab
    );
    if (candidates.length === 0) return;
    // Newest wins if several qualify.
    const cont = candidates.reduce((a, b) => (b.last_active > a.last_active ? b : a));

    setSuperseded((prev) => {
      const next = new Set(prev);
      next.add(sid);
      return next;
    });
    // Rebind both sessionId AND slotId (slotId drives the live dot + tab dedup;
    // resumed tabs keep them equal). The Agents panel follows via sessionId.
    setTabs((ts) =>
      ts.map((t) => (t.key === tab.key ? { ...t, sessionId: cont.id, slotId: cont.id } : t)),
    );
    setUndoInfo({ id: sid, title: curRow.title });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessions]);

  // Render the sessions list without hidden (superseded) rows; detection and
  // knownIds diffing still run against the unfiltered `sessions`.
  const visibleSessions = useMemo(
    () => sessions.filter((s) => !superseded.has(s.id)),
    [sessions, superseded],
  );

  const openTab = useCallback(
    (title: string, cwd: string | null, command: string | null, slotId: string, sessionId?: string) => {
      setPreviewSession(null);
      setTabs((t) => {
        const existing = t.find((x) => x.slotId === slotId);
        if (existing) {
          setActiveTab(existing.key);
          return t;
        }
        const key = nextKey.current++;
        setActiveTab(key);
        return [...t, { key, title, cwd, command, sessionId, slotId }];
      });
    },
    [],
  );

  const registerHandle = useCallback((key: number, handle: TermHandle | null) => {
    if (handle) handles.current.set(key, handle);
    else handles.current.delete(key);
  }, []);

  const noteActivity = useCallback((key: number) => {
    lastOutput.current.set(key, Date.now());
  }, []);

  const noteAttention = useCallback((key: number, on: boolean) => {
    // A bell on the tab you're actively looking at isn't news.
    if (on && key === activeTabRef.current && document.hasFocus()) return;
    setAttention((prev) => {
      if (on === prev.has(key)) return prev;
      const next = new Set(prev);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });
    if (on && !document.hasFocus()) {
      getCurrentWindow()
        .requestUserAttention(UserAttentionType.Informational)
        .catch(() => {});
    }
  }, []);

  // Viewing a tab (with the window focused) clears its badge.
  useEffect(() => {
    activeTabRef.current = activeTab;
    if (activeTab === null) return;
    const clear = () => {
      if (document.hasFocus() && activeTabRef.current !== null) {
        setAttention((prev) => {
          if (!prev.has(activeTabRef.current!)) return prev;
          const next = new Set(prev);
          next.delete(activeTabRef.current!);
          return next;
        });
      }
    };
    clear();
    window.addEventListener("focus", clear);
    return () => window.removeEventListener("focus", clear);
  }, [activeTab]);

  // (Removed the startup OS-window ±1px "jiggle". It forced a Wayland surface
  // reconfigure to fix bottom-edge clipping / stale content — a symptom of the
  // WebKitGTK DMABUF renderer, now disabled at the Rust entry point. No more
  // window growing/shrinking on launch.)

  // Dropping files onto the window pastes their quoted paths into the
  // active terminal (like any terminal emulator) instead of letting the
  // webview navigate to the file.
  const previewRef = useRef<Session | null>(null);
  useEffect(() => {
    previewRef.current = previewSession;
  }, [previewSession]);
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type !== "drop" || e.payload.paths.length === 0) return;
      const key = activeTabRef.current;
      if (key === null || previewRef.current) return;
      const h = handles.current.get(key);
      // One paste per path, like a real terminal drop — pasted (not typed)
      // so claude recognizes image/file paths and shows [Image #N].
      e.payload.paths.forEach((p, i) => {
        if (i > 0) h?.write(" ");
        h?.paste(shellEscape(p));
      });
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // Watch the active project: git changes refresh the repo panel, tree
  // changes refresh the explorer (git status also follows tree edits).
  useEffect(() => {
    if (!activeProject) return;
    watchProject(activeProject).catch(console.error);
  }, [activeProject]);
  useEffect(() => {
    const un = listen<{ git: boolean; tree: boolean }>("fs://changed", (e) => {
      setGitRefresh((n) => n + 1);
      if (e.payload.tree) setExplorerRefresh((n) => n + 1);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // "working" pulse: active tab produced output within the last 2.5s.
  useEffect(() => {
    const iv = setInterval(() => {
      const last = activeTab !== null ? (lastOutput.current.get(activeTab) ?? 0) : 0;
      setWorking(Date.now() - last < 2500);
    }, 1000);
    return () => clearInterval(iv);
  }, [activeTab]);

  // Poll the claude session status for the active tab (if it's a resume tab).
  const activeTabObj = tabs.find((t) => t.key === activeTab) ?? null;
  const activeSessionId = activeTabObj?.sessionId ?? null;
  useEffect(() => {
    if (!activeSessionId) {
      setClaudeStatus(null);
      return;
    }
    let stop = false;
    const poll = () =>
      sessionStatus(activeSessionId)
        .then((s) => !stop && setClaudeStatus(s.exists ? s : null))
        .catch(() => {});
    poll();
    const iv = setInterval(poll, 5000);
    return () => {
      stop = true;
      clearInterval(iv);
    };
  }, [activeSessionId]);

  // Branch for the status bar follows the active project.
  useEffect(() => {
    if (!activeProject) {
      setBranch(null);
      return;
    }
    gitRepoState(activeProject)
      .then((s) => setBranch(s.is_repo ? s.branch : null))
      .catch(() => setBranch(null));
  }, [activeProject, gitRefresh]);

  const closeTab = useCallback((key: number) => {
    setTabs((t) => {
      const next = t.filter((x) => x.key !== key);
      setActiveTab((cur) => (cur === key ? (next[next.length - 1]?.key ?? null) : cur));
      return next;
    });
  }, []);

  const selectSession = (s: Session) => {
    setActiveProject(s.project_path);
    // Warp-style: the sidebar is the tab list — switch to this item's live
    // terminal if it has one (resume first, then a project shell). Without
    // one, show a read-only conversation preview so ▶ can be an informed
    // choice.
    const live =
      tabs.find((t) => t.slotId === s.id) ??
      tabs.find((t) => t.slotId === `shell:${s.project_path}`);
    if (live) {
      setPreviewSession(null);
      setActiveTab(live.key);
    } else {
      setPreviewSession(s);
    }
  };
  const resumeSession = async (s: Session) => {
    setActiveProject(s.project_path);
    // The pinned id can go stale: a `/clear` or compaction retires the
    // original transcript (Claude Code deletes it or renames it to
    // `<id>.orphaned-…`), so `claude --resume <original-id>` dies with "no
    // conversation found" — a black pane, the core "broken feel" of resume.
    // Resolve to the surviving continuation first; if nothing resumable is
    // left, say so instead of launching a doomed resume. (A forked parent is
    // NOT stale — forking leaves it intact, and it resolves to itself.)
    let liveId = s.id;
    try {
      const resolved = await resolveResumableId(s.id);
      if (resolved === null) {
        setNotice(`"${s.title}" was cleared or superseded — no resumable transcript remains.`);
        return;
      }
      liveId = resolved;
    } catch {
      liveId = s.id; // resolver unavailable → fall back to the pinned id
    }
    // A plain `claude --resume` on a session still running as a bg agent exits
    // with "…add --fork-session to branch off a copy", leaving a black pane —
    // so those MUST fork. But forking unconditionally mints a new transcript on
    // every resume, which is what buried the sessions/agents lists in
    // duplicates. So fork only when actually running (authoritative: a live
    // claude process names it in /proc); otherwise resume in place, same id, no
    // duplicate. The agents panel follows either path because the backend
    // resolves a pinned id to the newest transcript in its fork family.
    let running: string[] = [];
    let bgAgents: string[] = [];
    try {
      [running, bgAgents] = await Promise.all([
        runningSessionIds(),
        bgAgentSessionIds(),
      ]);
    } catch {
      /* keep whatever resolved; defaults stand */
    }
    // A session the daemon holds as a *background agent* (a `/fork`, `--bg`)
    // isn't resumable at all: a prompt-less fork stub has a title-only
    // transcript (resume dies with "no conversation found"), and even a
    // prompted one only yields a detached copy via --fork-session. The real
    // agent lives behind the daemon, reachable through the agent view — so
    // point the row there and leave the session itself alone.
    if (bgAgents.includes(liveId)) {
      openTab(
        `⑂ agents · ${s.title}`, s.project_path,
        `claude agents --cwd ${s.project_path}`,
        `agents:${s.project_path}`, liveId,
      );
      return;
    }
    // Match a full id (from /proc) OR the first UUID segment (daemon bg-agent
    // sockets are named by the short id). A running session — incl. a bg-agent
    // fork — must resume with --fork-session or Claude Code errors out.
    const isRunning =
      running.includes(liveId) || running.includes(liveId.split("-")[0]);
    const command = isRunning
      ? `claude --fork-session --resume ${liveId}`
      : `claude --resume ${liveId}`;
    openTab(s.title, s.project_path, command, liveId, liveId);
  };
  // Fork an active session into its own tab — branch a copy with full history.
  // Resume already forks a *running* session, but folds onto the existing tab
  // (slotId dedup); an explicit fork wants a separate terminal, so give it a
  // unique slot. The continuation tracker rebinds that slot to the real fork id
  // once Claude Code writes it.
  const forkSession = async (s: Session) => {
    setActiveProject(s.project_path);
    let liveId = s.id;
    try {
      const resolved = await resolveResumableId(s.id);
      if (resolved === null) {
        setNotice(`"${s.title}" was cleared or superseded — nothing to fork.`);
        return;
      }
      liveId = resolved;
    } catch {
      liveId = s.id;
    }
    openTab(
      s.title, s.project_path,
      `claude --fork-session --resume ${liveId}`,
      `fork:${liveId}:${nextKey.current}`, liveId,
    );
    // The fork carries the conversation forward — exit the parent's live
    // terminal so one context isn't running twice. Its row stays listed
    // (scan keeps fork siblings) and resumes later at its pre-fork context.
    // Focus stays on the fork tab: closeTab only refocuses when closing the
    // active tab. A parent running as a bg agent has no tab here; that's
    // Claude Code's process to manage, not ours. (tabsRef, not tabs: the
    // resolver await above means the render-time snapshot can be stale.)
    const parent = tabsRef.current.find((t) => t.slotId === liveId);
    if (parent) closeTab(parent.key);
  };
  // Exit an active session: close its live terminal tab (ends the running
  // claude process). The transcript stays on disk, so it's resumable later.
  const exitSession = (s: Session) => {
    const live = tabs.find((t) => t.slotId === s.id);
    if (live) closeTab(live.key);
  };
  const newShell = (s: Session) => {
    setActiveProject(s.project_path);
    openTab(basename(s.project_path), s.project_path, null, `shell:${s.project_path}`);
  };
  const deleteSession = async (s: Session) => {
    try {
      await sessionDelete(s.id);
    } catch (e) {
      console.error("delete failed:", e);
    }
    setPreviewSession((p) => (p?.id === s.id ? null : p));
    refreshSessions();
  };
  const restoreTrashed = async (id: string) => {
    try {
      await trashRestore(id);
    } catch (e) {
      console.error("restore failed:", e);
    }
    refreshSessions();
  };
  const deleteTrashed = async (id: string) => {
    try {
      await trashDelete(id);
    } catch (e) {
      console.error("trash delete failed:", e);
    }
    refreshSessions();
  };
  const emptyTrash = async () => {
    try {
      await trashEmpty();
    } catch (e) {
      console.error("empty trash failed:", e);
    }
    refreshSessions();
  };
  const selectProject = (p: ProjectInfo) => {
    setActiveProject(p.path);
    const live =
      tabs.find((t) => t.slotId === `claude:${p.path}`) ??
      tabs.find((t) => t.slotId === `shell:${p.path}`);
    if (live) setActiveTab(live.key);
  };
  const projectShell = (p: ProjectInfo) => {
    setActiveProject(p.path);
    openTab(p.name, p.path, null, `shell:${p.path}`);
  };
  const projectClaude = (p: ProjectInfo) => {
    setActiveProject(p.path);
    openTab(p.name, p.path, "claude", `claude:${p.path}`);
  };

  // --- splitter dragging ---
  const dragging = useRef<null | "left" | "right" | "rightsplit" | "agentsplit">(null);
  const rightColRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const move = (e: MouseEvent) => {
      if (!dragging.current) return;
      e.preventDefault();
      if (dragging.current === "left") {
        setSizes((s) => ({
          ...s,
          left: Math.max(140, Math.min(window.innerWidth - 260, e.clientX)),
        }));
      } else if (dragging.current === "right") {
        setSizes((s) => ({
          ...s,
          right: Math.max(150, Math.min(window.innerWidth - 260, window.innerWidth - e.clientX)),
        }));
      } else if (dragging.current === "rightsplit" && rightColRef.current) {
        const r = rightColRef.current.getBoundingClientRect();
        const frac = (e.clientX - r.left) / r.width;
        setSizes((s) => ({ ...s, explorerFrac: Math.max(0.15, Math.min(0.85, frac)) }));
      } else if (dragging.current === "agentsplit" && rightColRef.current) {
        const r = rightColRef.current.getBoundingClientRect();
        const frac = (r.bottom - e.clientY) / r.height;
        setSizes((s) => ({ ...s, agentFrac: Math.max(0.12, Math.min(0.7, frac)) }));
      }
    };
    const up = () => {
      dragging.current = null;
      document.body.classList.remove("dragging");
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, []);

  const startDrag = (which: "left" | "right" | "rightsplit" | "agentsplit") => {
    dragging.current = which;
    document.body.classList.add("dragging");
  };

  const showRight = showExplorer || showGit;

  return (
    <div className="app">
      {notice && (
        <div className="app-toast" role="status" onClick={() => setNotice(null)}>
          {notice}
        </div>
      )}
      {undoInfo && (
        <div className="app-toast undo-toast" role="status">
          <span className="undo-msg">Hid superseded session “{undoInfo.title}”</span>
          <button
            className="undo-btn"
            onClick={() => {
              const id = undoInfo.id;
              setSuperseded((prev) => {
                const next = new Set(prev);
                next.delete(id);
                return next;
              });
              setUndoInfo(null);
            }}
          >Undo</button>
        </div>
      )}
      <div className="topbar">
        <div className="topbar-left">
          <button
            className={"icon-btn" + (showSessions ? " on" : "")}
            title="Toggle sessions panel"
            onClick={() => setShowSessions(!showSessions)}
          >▤</button>
          <button
            className={"icon-btn" + (showExplorer ? " on" : "")}
            title="Toggle file explorer"
            onClick={() => setShowExplorer(!showExplorer)}
          >🗀</button>
          <button
            className={"icon-btn" + (showGit ? " on" : "")}
            title="Toggle repository panel"
            onClick={() => setShowGit(!showGit)}
          >⎇</button>
          <button
            className={"icon-btn" + (showAgent ? " on" : "")}
            title="Toggle tasks panel"
            onClick={() => setShowAgent(!showAgent)}
          >☑</button>
          <button
            className={"icon-btn" + (showComposer ? " on" : "")}
            title="Toggle input composer"
            onClick={() => setShowComposer(!showComposer)}
          >⌨</button>
          <UsageBars />
          <Clock />
        </div>
        <div className="topbar-title">
          {activeProject ? activeProject.replace(/^\/home\/[^/]+/, "~") : "aiterm"}
        </div>
        <div className="topbar-right">
          <button className="icon-btn" title="Smaller fonts (Ctrl+-)" onClick={() => bumpFont(-1)}>A−</button>
          <button
            className="icon-btn"
            title="Reset font size (Ctrl+0)"
            onClick={() => bumpFont(0)}
          >{Math.round(fontScale * 100)}%</button>
          <button className="icon-btn" title="Larger fonts (Ctrl+=)" onClick={() => bumpFont(1)}>A+</button>
          <button
            className={"icon-btn" + (showSettingsModal ? " on" : "")}
            title="Settings"
            onClick={() => setShowSettingsModal(!showSettingsModal)}
          >⚙</button>
        </div>
      </div>
      <div className="main">
        {showSessions && (
          <>
            <div className="panel sessions" style={{ width: sizes.left, ...zoomFor("sessions") }}>
              <SessionsPanel
                sessions={visibleSessions}
                projects={projects}
                activeProject={activeProject}
                liveSlots={new Set(tabs.map((t) => t.slotId))}
                attentionSlots={new Set(
                  tabs.filter((t) => attention.has(t.key)).map((t) => t.slotId),
                )}
                activeSlot={activeTabObj?.slotId ?? null}
                opts={opts}
                onOptsChange={setOpts}
                onSelect={selectSession}
                onResume={resumeSession}
                onFork={forkSession}
                onExit={exitSession}
                onNewShell={newShell}
                onDelete={deleteSession}
                onSelectProject={selectProject}
                onProjectShell={projectShell}
                onProjectClaude={projectClaude}
                onRefresh={refreshSessions}
                trashed={trashed}
                onRestore={restoreTrashed}
                onTrashDelete={deleteTrashed}
                onTrashEmpty={emptyTrash}
              />
            </div>
            <div className="splitter v" onMouseDown={() => startDrag("left")} />
          </>
        )}

        <div className="panel terminal-panel">
          <div className="term-stack">
            {tabs.map((t) => (
              <TerminalView
                key={t.key}
                tab={t}
                active={t.key === activeTab}
                onExit={closeTab}
                onRegister={registerHandle}
                onActivity={noteActivity}
                onAttention={noteAttention}
                autoFocus={!showComposer}
                fontSize={termFont}
                fontFamily={xtermFont}
                theme={xtermTheme}
              />
            ))}
            {tabs.length === 0 && !previewSession && (
              <div className="empty-note big">Pick a session on the left — ▶ resumes claude, ＋ opens a shell</div>
            )}
            {previewSession && (
              <SessionPreview
                session={previewSession}
                onResume={resumeSession}
                onClose={() => setPreviewSession(null)}
              />
            )}
          </div>
          {showComposer && <Composer
            tabKey={activeTab}
            tabTitle={activeTabObj?.title ?? null}
            shells={tabs.length}
            working={working}
            claudeStatus={claudeStatus}
            projectLabel={activeProject ? activeProject.replace(/^\/home\/[^/]+/, "~") : null}
            branch={branch}
            onSend={(text) => activeTab !== null && handles.current.get(activeTab)?.sendComposed(text)}
            onControl={(seq) => activeTab !== null && handles.current.get(activeTab)?.write(seq)}
          />}
        </div>

        {showRight && (
          <>
            <div className="splitter v" onMouseDown={() => startDrag("right")} />
            <div className="right-col" ref={rightColRef} style={{ width: sizes.right }}>
              <div
                className="right-top"
                style={{ height: showAgent ? `${(1 - sizes.agentFrac) * 100}%` : "100%" }}
              >
                {showExplorer && (
                  <div
                    className="panel explorer"
                    style={{ width: showGit ? `${sizes.explorerFrac * 100}%` : "100%", ...zoomFor("explorer") }}
                  >
                    <div className="panel-header">
                      <span>EXPLORER</span>
                      <button className="icon-btn" onClick={() => setShowExplorer(false)}>✕</button>
                    </div>
                    <FileExplorer root={activeProject} refreshKey={explorerRefresh} />
                  </div>
                )}
                {showExplorer && showGit && (
                  <div className="splitter v" onMouseDown={() => startDrag("rightsplit")} />
                )}
                {showGit && (
                  <div className="panel git" style={{ flex: 1, minWidth: 0, ...zoomFor("git") }}>
                    <div className="panel-header">
                      <span>REPOSITORY</span>
                      <div>
                        <button className="icon-btn" title="Refresh"
                          onClick={() => setGitRefresh((n) => n + 1)}>⟳</button>
                        <button className="icon-btn" onClick={() => setShowGit(false)}>✕</button>
                      </div>
                    </div>
                    <GitPanel root={activeProject} refreshKey={gitRefresh} />
                  </div>
                )}
              </div>
              {showAgent && (
                <>
                  <div className="splitter h" onMouseDown={() => startDrag("agentsplit")} />
                  <div className="panel agent" style={{ flex: 1, minHeight: 0, ...zoomFor("agent") }}>
                    <div className="panel-header">
                      <span>AGENT{activeTabObj?.title ? ` — ${activeTabObj.title}` : ""}</span>
                      <button className="icon-btn" onClick={() => setShowAgent(false)}>✕</button>
                    </div>
                    <AgentPanel sessionId={activeSessionId} />
                  </div>
                </>
              )}
            </div>
          </>
        )}
      </div>
      {showSettingsModal && (
        <SettingsModal
          settings={settings}
          onChange={setSettings}
          onClose={() => setShowSettingsModal(false)}
        />
      )}
    </div>
  );
}

function basename(p: string): string {
  return p.split("/").filter(Boolean).pop() ?? p;
}

// Backslash-escape (the way terminals escape dropped paths) — claude's
// pasted-path detection understands this form, unlike single quotes.
function shellEscape(p: string): string {
  return p.replace(/[^A-Za-z0-9_\-./~+:@%=]/g, (c) => "\\" + c);
}
