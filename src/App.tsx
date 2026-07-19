import { useCallback, useEffect, useRef, useState } from "react";
import SessionsPanel, { SessionDisplayOpts } from "./components/SessionsPanel";
import TerminalView, { TermTab } from "./components/TerminalView";
import FileExplorer from "./components/FileExplorer";
import GitPanel from "./components/GitPanel";
import { Session, listSessions } from "./ipc";
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

  const [showSessions, setShowSessions] = useState(true);
  const [showExplorer, setShowExplorer] = useState(true);
  const [showGit, setShowGit] = useState(true);

  const [opts, setOpts] = useState<SessionDisplayOpts>(() =>
    loadJSON(OPTS_KEY, { showPath: true, showBranch: true, showTime: true }),
  );
  const [sizes, setSizes] = useState<PanelSizes>(() =>
    loadJSON(SIZES_KEY, { left: 280, right: 380, explorerFrac: 0.5 }),
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

  const openTab = useCallback((title: string, cwd: string | null, command: string | null) => {
    const key = nextKey.current++;
    setTabs((t) => [...t, { key, title, cwd, command }]);
    setActiveTab(key);
  }, []);

  const closeTab = useCallback((key: number) => {
    setTabs((t) => {
      const next = t.filter((x) => x.key !== key);
      setActiveTab((cur) => (cur === key ? (next[next.length - 1]?.key ?? null) : cur));
      return next;
    });
  }, []);

  useEffect(() => {
    // Start with one shell in the home directory.
    if (tabs.length === 0 && nextKey.current === 1) openTab("shell", null, null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const selectSession = (s: Session) => setActiveProject(s.project_path);
  const resumeSession = (s: Session) => {
    setActiveProject(s.project_path);
    openTab(s.title, s.project_path, `claude --resume ${s.id}`);
  };
  const newShell = (s: Session) => {
    setActiveProject(s.project_path);
    openTab(basename(s.project_path), s.project_path, null);
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
        const frac = (e.clientY - r.top) / r.height;
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
          <div className="tabbar">
            {tabs.map((t) => (
              <div
                key={t.key}
                className={"tab" + (t.key === activeTab ? " on" : "")}
                onClick={() => setActiveTab(t.key)}
              >
                <span className="tab-title">{t.title}</span>
                <button
                  className="tab-close"
                  onClick={(e) => { e.stopPropagation(); closeTab(t.key); }}
                >✕</button>
              </div>
            ))}
            <button
              className="icon-btn new-tab"
              title="New shell"
              onClick={() => openTab(activeProject ? basename(activeProject) : "shell", activeProject, null)}
            >＋</button>
          </div>
          <div className="term-stack">
            {tabs.map((t) => (
              <TerminalView key={t.key} tab={t} active={t.key === activeTab} onExit={closeTab} />
            ))}
            {tabs.length === 0 && (
              <div className="empty-note big">No terminal open — press ＋ or pick a session</div>
            )}
          </div>
        </div>

        {showRight && (
          <>
            <div className="splitter v" onMouseDown={() => startDrag("right")} />
            <div className="right-col" ref={rightColRef} style={{ width: sizes.right }}>
              {showExplorer && (
                <div
                  className="panel explorer"
                  style={{ height: showGit ? `${sizes.explorerFrac * 100}%` : "100%" }}
                >
                  <div className="panel-header">
                    <span>EXPLORER</span>
                    <button className="icon-btn" onClick={() => setShowExplorer(false)}>✕</button>
                  </div>
                  <FileExplorer root={activeProject} />
                </div>
              )}
              {showExplorer && showGit && (
                <div className="splitter h" onMouseDown={() => startDrag("rightsplit")} />
              )}
              {showGit && (
                <div className="panel git" style={{ flex: 1, minHeight: 0 }}>
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
