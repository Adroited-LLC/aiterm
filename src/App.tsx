import { useCallback, useEffect, useRef, useState } from "react";
import SessionsPanel, { SessionDisplayOpts } from "./components/SessionsPanel";
import TerminalView, { TermHandle, TermTab } from "./components/TerminalView";
import FileExplorer from "./components/FileExplorer";
import GitPanel from "./components/GitPanel";
import Composer from "./components/Composer";
import AgentPanel from "./components/AgentPanel";
import {
  ProjectInfo, Session, SessionStatus,
  gitRepoState, hasTmux, listProjects, listSessions, reindexSessions,
  sessionStatus, tmuxSessions,
} from "./ipc";
import "./App.css";

const OPTS_KEY = "aiterm.sessionOpts";
const SIZES_KEY = "aiterm.panelSizes";
const FONT_KEY = "aiterm.fontScale";
const TMUX_KEY = "aiterm.tmux";
const TMUX_TABS_KEY = "aiterm.tmuxTabs";

interface SavedTab {
  slotId: string;
  title: string;
  cwd: string | null;
  command: string | null;
  sessionId?: string;
}

const tmuxName = (slotId: string) =>
  "aiterm-" + slotId.replace(/[^a-zA-Z0-9_-]/g, "_");

/** Wrap a tab's command in a reattachable tmux session (status bar off). */
function tmuxWrap(slotId: string, command: string | null): string {
  const inner = command ? ` '${command.replace(/'/g, "'\\''")}'` : "";
  return `tmux new-session -A -s ${tmuxName(slotId)}${inner} \\; set-option status off`;
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
  const [tabs, setTabs] = useState<TermTab[]>([]);
  const [activeTab, setActiveTab] = useState<number | null>(null);
  const nextKey = useRef(1);
  const [gitRefresh, setGitRefresh] = useState(0);

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

  const [tmuxOk, setTmuxOk] = useState(false);
  const [persistTmux, setPersistTmux] = useState<boolean>(
    () => loadJSON(TMUX_KEY, { on: true }).on,
  );
  useEffect(() => localStorage.setItem(TMUX_KEY, JSON.stringify({ on: persistTmux })), [persistTmux]);
  const useTmux = tmuxOk && persistTmux;
  const useTmuxRef = useRef(useTmux);
  useTmuxRef.current = useTmux;

  const [opts, setOpts] = useState<SessionDisplayOpts>(() =>
    loadJSON(OPTS_KEY, { showPath: true, showBranch: true, showTime: true }),
  );
  const [sizes, setSizes] = useState<PanelSizes>(() =>
    loadJSON(SIZES_KEY, { left: 280, right: 560, explorerFrac: 0.5, agentFrac: 0.3 }),
  );

  const [fontScale, setFontScale] = useState<number>(
    () => loadJSON(FONT_KEY, { scale: 1 }).scale,
  );

  useEffect(() => localStorage.setItem(OPTS_KEY, JSON.stringify(opts)), [opts]);
  useEffect(() => localStorage.setItem(SIZES_KEY, JSON.stringify(sizes)), [sizes]);
  useEffect(() => localStorage.setItem(FONT_KEY, JSON.stringify({ scale: fontScale })), [fontScale]);

  const bumpFont = useCallback((dir: 1 | -1 | 0) => {
    setFontScale((s) =>
      dir === 0 ? 1 : Math.max(0.7, Math.min(1.6, +(s + dir * 0.1).toFixed(2))),
    );
  }, []);

  // Ctrl+= / Ctrl+- / Ctrl+0 font zoom, captured before xterm sees the keys.
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.altKey || e.metaKey) return;
      if (e.key === "=" || e.key === "+") { e.preventDefault(); bumpFont(1); }
      else if (e.key === "-") { e.preventDefault(); bumpFont(-1); }
      else if (e.key === "0") { e.preventDefault(); bumpFont(0); }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [bumpFont]);

  const termFont = Math.round(13 * fontScale);
  const panelZoom = { zoom: fontScale } as React.CSSProperties;

  const [projects, setProjects] = useState<ProjectInfo[]>([]);

  const refreshSessions = useCallback(() => {
    listSessions().then(setSessions).catch(console.error);
    listProjects().then(setProjects).catch(console.error);
    // Keep the full-text index warm in the background.
    reindexSessions().catch(() => {});
  }, []);
  useEffect(() => {
    refreshSessions();
    const iv = setInterval(refreshSessions, 30_000);
    return () => clearInterval(iv);
  }, [refreshSessions]);

  const openTab = useCallback(
    (title: string, cwd: string | null, command: string | null, slotId: string, sessionId?: string) => {
      setTabs((t) => {
        const existing = t.find((x) => x.slotId === slotId);
        if (existing) {
          setActiveTab(existing.key);
          return t;
        }
        const wrapped = useTmuxRef.current ? tmuxWrap(slotId, command) : command;
        if (useTmuxRef.current) {
          const saved = loadJSON<Record<string, SavedTab>>(TMUX_TABS_KEY, {});
          saved[tmuxName(slotId)] = { slotId, title, cwd, command, sessionId };
          localStorage.setItem(TMUX_TABS_KEY, JSON.stringify(saved));
        }
        const key = nextKey.current++;
        setActiveTab(key);
        return [...t, { key, title, cwd, command: wrapped, sessionId, slotId }];
      });
    },
    [],
  );

  // On launch: reattach tabs for aiterm tmux sessions that survived a restart.
  useEffect(() => {
    (async () => {
      const ok = await hasTmux().catch(() => false);
      setTmuxOk(ok);
      if (!ok || !loadJSON(TMUX_KEY, { on: true }).on) return;
      const alive = new Set(await tmuxSessions().catch(() => [] as string[]));
      const saved = loadJSON<Record<string, SavedTab>>(TMUX_TABS_KEY, {});
      let restoredProject: string | null = null;
      for (const [name, t] of Object.entries(saved)) {
        if (alive.has(name)) {
          openTab(t.title, t.cwd, t.command, t.slotId, t.sessionId);
          restoredProject = t.cwd ?? restoredProject;
        } else {
          delete saved[name];
        }
      }
      localStorage.setItem(TMUX_TABS_KEY, JSON.stringify(saved));
      if (restoredProject) setActiveProject((p) => p ?? restoredProject);
    })();
  }, [openTab]);

  const registerHandle = useCallback((key: number, handle: TermHandle | null) => {
    if (handle) handles.current.set(key, handle);
    else handles.current.delete(key);
  }, []);

  const noteActivity = useCallback((key: number) => {
    lastOutput.current.set(key, Date.now());
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
      const closing = t.find((x) => x.key === key);
      if (closing) {
        // The pty exiting means the tmux session ended — drop its saved entry.
        const saved = loadJSON<Record<string, SavedTab>>(TMUX_TABS_KEY, {});
        delete saved[tmuxName(closing.slotId)];
        localStorage.setItem(TMUX_TABS_KEY, JSON.stringify(saved));
      }
      const next = t.filter((x) => x.key !== key);
      setActiveTab((cur) => (cur === key ? (next[next.length - 1]?.key ?? null) : cur));
      return next;
    });
  }, []);

  const selectSession = (s: Session) => {
    setActiveProject(s.project_path);
    // Warp-style: the sidebar is the tab list — switch to this item's live
    // terminal if it has one (resume first, then a project shell).
    const live =
      tabs.find((t) => t.slotId === s.id) ??
      tabs.find((t) => t.slotId === `shell:${s.project_path}`);
    if (live) setActiveTab(live.key);
  };
  const resumeSession = (s: Session) => {
    setActiveProject(s.project_path);
    openTab(s.title, s.project_path, `claude --resume ${s.id}`, s.id, s.id);
  };
  const newShell = (s: Session) => {
    setActiveProject(s.project_path);
    openTab(basename(s.project_path), s.project_path, null, `shell:${s.project_path}`);
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
        setSizes((s) => ({ ...s, left: Math.max(180, Math.min(500, e.clientX)) }));
      } else if (dragging.current === "right") {
        setSizes((s) => ({
          ...s,
          right: Math.max(220, Math.min(700, window.innerWidth - e.clientX)),
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
          <button
            className="icon-btn"
            title={activeProject ? `New shell in ${basename(activeProject)}` : "New shell"}
            onClick={() =>
              openTab(
                activeProject ? basename(activeProject) : "shell",
                activeProject,
                null,
                activeProject ? `shell:${activeProject}` : "shell:home",
              )
            }
          >＋</button>
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
        </div>
      </div>
      <div className="main">
        {showSessions && (
          <>
            <div className="panel sessions" style={{ width: sizes.left, ...panelZoom }}>
              <SessionsPanel
                sessions={sessions}
                projects={projects}
                activeProject={activeProject}
                liveSlots={new Set(tabs.map((t) => t.slotId))}
                activeSlot={activeTabObj?.slotId ?? null}
                opts={opts}
                onOptsChange={setOpts}
                onSelect={selectSession}
                onResume={resumeSession}
                onNewShell={newShell}
                onSelectProject={selectProject}
                onProjectShell={projectShell}
                onProjectClaude={projectClaude}
                onRefresh={refreshSessions}
                tmuxAvailable={tmuxOk}
                tmuxOn={persistTmux}
                onTmuxChange={setPersistTmux}
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
                autoFocus={!showComposer}
                fontSize={termFont}
              />
            ))}
            {tabs.length === 0 && (
              <div className="empty-note big">Pick a session on the left — ▶ resumes claude, ＋ opens a shell</div>
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
            <div className="right-col" ref={rightColRef} style={{ width: sizes.right, ...panelZoom }}>
              <div
                className="right-top"
                style={{ height: showAgent ? `${(1 - sizes.agentFrac) * 100}%` : "100%" }}
              >
                {showExplorer && (
                  <div
                    className="panel explorer"
                    style={{ width: showGit ? `${sizes.explorerFrac * 100}%` : "100%" }}
                  >
                    <div className="panel-header">
                      <span>EXPLORER</span>
                      <button className="icon-btn" onClick={() => setShowExplorer(false)}>✕</button>
                    </div>
                    <FileExplorer root={activeProject} />
                  </div>
                )}
                {showExplorer && showGit && (
                  <div className="splitter v" onMouseDown={() => startDrag("rightsplit")} />
                )}
                {showGit && (
                  <div className="panel git" style={{ flex: 1, minWidth: 0 }}>
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
                  <div className="panel agent" style={{ flex: 1, minHeight: 0 }}>
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
    </div>
  );
}

function basename(p: string): string {
  return p.split("/").filter(Boolean).pop() ?? p;
}
