import { useCallback, useEffect, useRef, useState } from "react";
import SessionsPanel, { SessionDisplayOpts } from "./components/SessionsPanel";
import TerminalView, { TermHandle, TermTab } from "./components/TerminalView";
import FileExplorer from "./components/FileExplorer";
import GitPanel from "./components/GitPanel";
import Composer from "./components/Composer";
import { Session, SessionStatus, gitRepoState, listSessions, sessionStatus } from "./ipc";
import "./App.css";

const OPTS_KEY = "aiterm.sessionOpts";
const SIZES_KEY = "aiterm.panelSizes";

interface PanelSizes {
  left: number;
  right: number;
  explorerFrac: number;
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

  const [opts, setOpts] = useState<SessionDisplayOpts>(() =>
    loadJSON(OPTS_KEY, { showPath: true, showBranch: true, showTime: true }),
  );
  const [sizes, setSizes] = useState<PanelSizes>(() =>
    loadJSON(SIZES_KEY, { left: 280, right: 560, explorerFrac: 0.5 }),
  );

  useEffect(() => localStorage.setItem(OPTS_KEY, JSON.stringify(opts)), [opts]);
  useEffect(() => localStorage.setItem(SIZES_KEY, JSON.stringify(sizes)), [sizes]);

  const refreshSessions = useCallback(() => {
    listSessions().then(setSessions).catch(console.error);
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

  // --- splitter dragging ---
  const dragging = useRef<null | "left" | "right" | "rightsplit">(null);
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

  const startDrag = (which: "left" | "right" | "rightsplit") => {
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
        <div className="topbar-right" />
      </div>
      <div className="main">
        {showSessions && (
          <>
            <div className="panel sessions" style={{ width: sizes.left }}>
              <SessionsPanel
                sessions={sessions}
                activeProject={activeProject}
                liveSlots={new Set(tabs.map((t) => t.slotId))}
                activeSlot={activeTabObj?.slotId ?? null}
                opts={opts}
                onOptsChange={setOpts}
                onSelect={selectSession}
                onResume={resumeSession}
                onNewShell={newShell}
                onRefresh={refreshSessions}
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
            <div className="right-col" ref={rightColRef} style={{ width: sizes.right }}>
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
          </>
        )}
      </div>
    </div>
  );
}

function basename(p: string): string {
  return p.split("/").filter(Boolean).pop() ?? p;
}
