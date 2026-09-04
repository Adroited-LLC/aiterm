import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowLeft, Folder, FolderOpen, GitBranch, Home, PanelLeft, Plus, RefreshCw, Search, Settings, TerminalSquare, X } from "lucide-react";
import Icon from "../components/Icon";
import AgentIcon from "../components/AgentIcon";
import FileExplorer from "../components/FileExplorer";
import FileView from "../components/FileView";
import GitPanel from "../components/GitPanel";
import { Clock } from "../components/Clock";
import { DirEntry, ProjectInfo, listDir, listProjects, homeAbbrev, relTime } from "../ipc";
import { invoke } from "../platform";
import WslTerminal, { Workspace } from "./WslTerminal";
import "../App.css";
import "./windows.css";

type Agent = { id: string; display_name: string };
type History = { id: string; agent: string; title: string; project_path: string; last_active: number };
type Tab = { id: string; title: string; cwd: string; command: string | null; agent?: string; sessionId?: string };
type FileTab = { path: string; dirty: boolean };
const quote = (text: string) => "'" + text.split("'").join("'\\''") + "'";

function FolderPicker({ initial, onPick, onClose }: { initial: string; onPick: (path: string) => void; onClose: () => void }) {
  const [path, setPath] = useState(initial);
  const [draft, setDraft] = useState(initial);
  const [dirs, setDirs] = useState<DirEntry[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(true);
  useEffect(() => {
    let alive = true; setBusy(true); setError(""); setDraft(path);
    listDir(path).then(entries => { if (alive) setDirs(entries.filter(e => e.is_dir)); }).catch(e => { if (alive) setError(String(e)); }).finally(() => { if (alive) setBusy(false); });
    return () => { alive = false; };
  }, [path]);
  return <div className="modal-backdrop" onMouseDown={e => { if (e.target === e.currentTarget) onClose(); }}>
    <section className="wsl-dialog" role="dialog" aria-modal="true" aria-label="Choose a Linux folder">
      <div className="panel-header"><span>CHOOSE A FOLDER IN LINUX</span><button className="icon-btn" title="Close" onClick={onClose}><Icon of={X}/></button></div>
      <form className="panel-toolbar" onSubmit={e => { e.preventDefault(); if (draft.startsWith("/")) setPath(draft); else setError("Enter an absolute Linux path, starting with /."); }}>
        <button type="button" className="icon-btn" title="Parent folder" disabled={path === "/"} onClick={() => setPath(path.replace(/\/+$/, "").split("/").slice(0, -1).join("/") || "/")}><Icon of={ArrowLeft}/></button>
        <input className="wsl-path" aria-label="Linux folder path" value={draft} onChange={e => setDraft(e.target.value)}/><button className="btn" type="submit">Go</button>
      </form>
      <div className="wsl-folder-list">{busy ? <div className="empty-note">Loading folders…</div> : error ? <div className="empty-note" role="alert">{error}</div> : dirs.map(d => <button className="tree-row wsl-folder-row" key={d.path} onClick={() => setPath(d.path)}><Icon of={Folder}/><span>{d.name}</span></button>)}</div>
      <div className="wsl-dialog-actions"><button className="btn" onClick={onClose}>Cancel</button><button className="btn primary" disabled={busy || !!error} onClick={() => onPick(path)}>Use this folder</button></div>
    </section>
  </div>;
}

function App() {
  const [workspace, setWorkspace] = useState<Workspace>();
  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [history, setHistory] = useState<History[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [agent, setAgent] = useState("");
  const [cwd, setCwd] = useState("");
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [files, setFiles] = useState<FileTab[]>([]);
  const [activeFile, setActiveFile] = useState<string | null>(null);
  const [ended, setEnded] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState("");
  const [startupError, setStartupError] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [showSessions, setShowSessions] = useState(true);
  const [showFiles, setShowFiles] = useState(true);
  const [showGit, setShowGit] = useState(true);
  const [picker, setPicker] = useState(false);
  const [settings, setSettings] = useState(false);
  const [fontSize, setFontSize] = useState(() => Math.max(10, Math.min(24, Number(localStorage.getItem("aiterm.windows.fontSize")) || 14)));
  const [widths, setWidths] = useState({ left: 260, right: 400 });
  const [confirm, setConfirm] = useState<{ message: string; action: () => void }>();
  const dirty = useRef(false); dirty.current = files.some(f => f.dirty);
  const loading = useRef(false);
  const load = useCallback(async () => {
    if (loading.current) return;
    loading.current = true;
    try {
      const ws = await invoke<Workspace>("workspace");
      setWorkspace(ws); setCwd(current => current || ws.home); setStartupError("");
      const results = await Promise.allSettled([listProjects(), invoke<History[]>("list_sessions"), invoke<Agent[]>("agent_choices")]);
      if (results[0].status === "fulfilled") setProjects(results[0].value);
      if (results[1].status === "fulfilled") setHistory(results[1].value);
      if (results[2].status === "fulfilled") { const found = results[2].value; setAgents(found); setAgent(current => found.some(a => a.id === current) ? current : found[0]?.id ?? ""); }
      const failure = results.find(r => r.status === "rejected");
      if (failure?.status === "rejected") setNotice(String(failure.reason));
    } catch (e) { setStartupError(String(e)); }
    finally { loading.current = false; }
  }, []);
  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    const timer = window.setInterval(() => { if (document.visibilityState === "visible") { void load(); setRefresh(n => n + 1); } }, 15000);
    return () => window.clearInterval(timer);
  }, [load]);
  useEffect(() => { localStorage.setItem("aiterm.windows.fontSize", String(fontSize)); }, [fontSize]);
  useEffect(() => {
    const unlisten = getCurrentWindow().onCloseRequested(async event => {
      if (!dirty.current) return;
      event.preventDefault();
      setConfirm({ message: "Close aiterm and discard your unsaved file edits?", action: () => { void getCurrentWindow().destroy(); } });
    });
    return () => { void unlisten.then(fn => fn()); };
  }, []);
  const selected = tabs.find(t => t.id === active);
  const root = selected?.cwd ?? cwd;
  const goHome = () => { setActive(null); setActiveFile(null); };
  const openTerminal = (target = cwd, agentId?: string, session?: History) => {
    if (!target) return;
    if (session) {
      const existing = tabs.find(t => t.sessionId === session.id && t.agent === session.agent);
      if (existing) { setActive(existing.id); setActiveFile(null); return; }
    }
    const id = crypto.randomUUID();
    const command = agentId ? `exec ${quote(agentId)}${session ? agentId === "claude" ? ` --resume ${quote(session.id)}` : ` resume ${quote(session.id)}` : ""}` : null;
    const title = session?.title ?? (agentId ? agents.find(a => a.id === agentId)?.display_name ?? agentId : target.split("/").filter(Boolean).pop() || "Terminal");
    setTabs(current => [...current, { id, title, cwd: target, command, agent: agentId, sessionId: session?.id }]);
    setActive(id); setActiveFile(null);
  };
  const closeTab = (id: string) => {
    setTabs(current => current.filter(t => t.id !== id));
    if (active === id) { const remaining = tabs.filter(t => t.id !== id); setActive(remaining[remaining.length - 1]?.id ?? null); setActiveFile(null); }
    setEnded(current => { const next = new Set(current); next.delete(id); return next; });
  };
  const openFile = (path: string) => { setFiles(current => current.some(f => f.path === path) ? current : [...current, { path, dirty: false }]); setActiveFile(path); };
  const closeFile = (file: FileTab) => {
    const action = () => { setFiles(current => current.filter(f => f.path !== file.path)); if (activeFile === file.path) setActiveFile(null); };
    if (file.dirty) setConfirm({ message: `Discard unsaved changes to ${file.path.split("/").pop()}?`, action }); else action();
  };
  const drag = (side: "left" | "right", startX: number) => {
    const initial = widths[side];
    const move = (e: MouseEvent) => setWidths(w => ({ ...w, [side]: Math.max(190, Math.min(window.innerWidth * .42, initial + (e.clientX - startX) * (side === "left" ? 1 : -1))) }));
    const up = () => { document.removeEventListener("mousemove", move); document.removeEventListener("mouseup", up); document.body.classList.remove("dragging"); };
    document.body.classList.add("dragging"); document.addEventListener("mousemove", move); document.addEventListener("mouseup", up);
  };
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "t") { e.preventDefault(); openTerminal(root); }
      if (e.key === "Escape") { setPicker(false); setSettings(false); setConfirm(undefined); }
    };
    window.addEventListener("keydown", key); return () => window.removeEventListener("keydown", key);
  });
  const filtered = history.filter(s => `${s.title} ${s.project_path} ${s.agent}`.toLowerCase().includes(query.toLowerCase()));
  return <div className="app wsl-app">
    {notice && <div className="app-toast" role="alert" onClick={() => setNotice("")}>{notice}</div>}
    <div className="topbar">
      <div className="topbar-left">
        <button className={`icon-btn${showSessions ? " on" : ""}`} title="Toggle sessions panel" onClick={() => setShowSessions(v => !v)}><Icon of={PanelLeft}/></button>
        <button className={`icon-btn${showFiles ? " on" : ""}`} title="Toggle file explorer" onClick={() => setShowFiles(v => !v)}><Icon of={FolderOpen}/></button>
        <button className={`icon-btn${showGit ? " on" : ""}`} title="Toggle repository panel" onClick={() => setShowGit(v => !v)}><Icon of={GitBranch}/></button>
      </div>
      <span className="wsl-workspace-name">{workspace?.distribution ?? "Linux workspace"}</span>
      <div className="topbar-spacer"/>
      <div className="topbar-right"><Clock/><button className="icon-btn" title="Smaller terminal font" onClick={() => setFontSize(v => Math.max(10, v - 1))}>A−</button><button className="icon-btn" title="Larger terminal font" onClick={() => setFontSize(v => Math.min(24, v + 1))}>A+</button><button className="icon-btn" title="Settings" onClick={() => setSettings(true)}><Icon of={Settings}/></button></div>
    </div>
    <div className="main">
      {showSessions && <><aside className="panel sessions" style={{ width: widths.left }}>
        <div className="panel-header"><span>SESSIONS</span><div><button className="icon-btn" title="Refresh sessions" onClick={() => { void load(); setRefresh(n => n + 1); }}><Icon of={RefreshCw}/></button><button className="icon-btn" title="New session" onClick={goHome}><Icon of={Plus}/></button></div></div>
        <div className="panel-toolbar"><label className="search-box"><Icon of={Search} size="sm"/><input className="search-input" placeholder="Search sessions" aria-label="Search sessions" value={query} onChange={e => setQuery(e.target.value)}/></label></div>
        <div className="wsl-session-list">
          {tabs.length > 0 && <div className="wsl-section-label">OPEN TABS</div>}
          {tabs.map(t => <div className={`session-item${active === t.id ? " active" : ""}`} key={t.id} role="button" tabIndex={0} onClick={() => { setActive(t.id); setActiveFile(null); }} onKeyDown={e => { if (e.key === "Enter") { setActive(t.id); setActiveFile(null); } }}><AgentIcon agent={t.agent ?? "shell"}/><div className="session-text"><div className="session-title">{t.title}</div><div className="session-meta"><span className="session-sub">{homeAbbrev(t.cwd)}</span>{ended.has(t.id) && <span>Ended</span>}</div></div></div>)}
          <div className="wsl-section-label">RECENT</div>
          {filtered.map(s => <button className="session-item wsl-session-row" key={`${s.agent}:${s.id}`} title={`Resume in ${s.project_path}`} onClick={() => agents.some(a => a.id === s.agent) ? openTerminal(s.project_path, s.agent, s) : setNotice(`Install ${s.agent} in ${workspace?.distribution ?? "Linux"} to resume this session.`)}><AgentIcon agent={s.agent}/><div className="session-text"><div className="session-title">{s.title}</div><div className="session-meta"><span className="session-sub">{homeAbbrev(s.project_path)}</span><span className="session-time">{relTime(s.last_active)}</span></div></div></button>)}
          {!filtered.length && <div className="empty-note">{query ? "No matching sessions" : "Your Claude Code and Codex conversations in Linux will appear here."}</div>}
          <div className="wsl-section-label">PROJECTS</div>
          {projects.map(p => <button className={`tree-row wsl-folder-row${cwd === p.path ? " on" : ""}`} key={p.path} title={p.path} onClick={() => { setCwd(p.path); goHome(); }}><Icon of={Folder}/><span className="tree-name">{p.name}</span></button>)}
          <button className="tree-row wsl-folder-row" disabled={!workspace} onClick={() => setPicker(true)}><Icon of={FolderOpen}/><span>Open folder…</span></button>
        </div>
      </aside><div className="splitter v" onMouseDown={e => drag("left", e.clientX)}/></>}
      <div className="panel terminal-panel">
        <div className="center-tabs"><button className={`center-tab home-tab${active === null && !activeFile ? " on" : ""}`} title="Home — start a session" onClick={goHome}><Icon of={Home} size="sm"/></button>
          {tabs.map(t => <div className={`center-tab session${active === t.id && !activeFile ? " on" : ""}`} key={t.id}><button className="wsl-tab-label" onClick={() => { setActive(t.id); setActiveFile(null); }}><AgentIcon agent={t.agent ?? "shell"} size={13}/><span className="center-tab-name">{t.title}</span></button><button className="wsl-tab-close" title={`Close ${t.title}`} onClick={() => closeTab(t.id)}><Icon of={X} size="sm"/></button></div>)}
          {files.map(f => <div className={`center-tab${activeFile === f.path ? " on" : ""}`} key={f.path}><button className="wsl-tab-label" title={f.path} onClick={() => setActiveFile(f.path)}><span className="center-tab-name">{f.dirty ? "● " : ""}{f.path.split("/").pop()}</span></button><button className="wsl-tab-close" title="Close file" onClick={() => closeFile(f)}><Icon of={X} size="sm"/></button></div>)}
          <button className="icon-btn" title="New terminal (Ctrl+Shift+T)" disabled={!workspace} onClick={() => openTerminal(root)}><Icon of={Plus}/></button>
        </div>
        {active === null && !activeFile && <div className="home"><div className="home-inner">
          <div className="wsl-home-heading"><h1>aiterm</h1><span>Your workspace in Linux</span></div>
          {!workspace ? <div className="home-card home-start"><h2>{startupError ? "Let’s connect your workspace" : "Opening your workspace"}</h2><p>{startupError || "Starting Linux and looking for your projects and agents…"}</p>{startupError && <button className="btn" onClick={() => void load()}>Try again</button>}</div> : <div className="home-card home-start">
            <button className="home-field" onClick={() => setPicker(true)} title={cwd}><Icon of={FolderOpen}/><span className="home-field-value">{homeAbbrev(cwd)}</span><span>Change…</span></button>
            <div className="home-engines"><div className="ns-agent-tabs">{agents.map(a => <button className={`ns-agent-tab${agent === a.id ? " on" : ""}`} key={a.id} onClick={() => setAgent(a.id)}><AgentIcon agent={a.id}/>{a.display_name}</button>)}</div></div>
            {agents.length ? <p className="wsl-hint">Start with your agent’s own model and permission settings.</p> : <p className="wsl-hint">No coding agents found in {workspace.distribution} yet. Open a terminal to install or sign in to your preferred agent, then refresh.</p>}
            <div className="home-actions">{agents.length > 0 && <button className="btn primary home-go" onClick={() => openTerminal(cwd, agent)}>Start session</button>}<button className="home-quiet" onClick={() => openTerminal(cwd)}><Icon of={TerminalSquare} size="sm"/> Open terminal</button><button className="icon-btn" title="Refresh agents" onClick={() => void load()}><Icon of={RefreshCw}/></button></div>
          </div>}
          <div className="wsl-home-note">Windows outside. Your Linux tools, projects, and conversations inside.</div>
        </div></div>}
        {tabs.map(t => <div className="wsl-terminal-slot" key={t.id} style={{ display: active === t.id && !activeFile ? "flex" : "none" }}><WslTerminal {...t} active={active === t.id && !activeFile} fontSize={fontSize} onReady={id => setEnded(current => { const next = new Set(current); next.delete(id); return next; })} onEnd={id => { setEnded(current => new Set(current).add(id)); void load(); }}/></div>)}
        {files.map(f => <div key={f.path} className="wsl-file-slot" style={{ display: activeFile === f.path ? "flex" : "none" }}><FileView path={f.path} active={activeFile === f.path} refreshKey={refresh} onDirty={isDirty => setFiles(current => current.map(item => item.path === f.path && item.dirty !== isDirty ? { ...item, dirty: isDirty } : item))}/></div>)}
      </div>
      {(showFiles || showGit) && <><div className="splitter v" onMouseDown={e => drag("right", e.clientX)}/><div className="right-col" style={{ width: widths.right }}><div className="right-top" style={{ height: "100%" }}>
        {showFiles && <div className="panel explorer" style={{ width: showGit ? "45%" : "100%" }}><div className="panel-header"><span>EXPLORER</span><button className="icon-btn" title="Hide explorer" onClick={() => setShowFiles(false)}><Icon of={X}/></button></div><FileExplorer root={root || null} refreshKey={refresh} onOpenFile={openFile}/></div>}
        {showFiles && showGit && <div className="splitter v locked"/>}
        {showGit && <div className="panel git" style={{ flex: 1, minWidth: 0 }}><div className="panel-header"><span>REPOSITORY</span><div><button className="icon-btn" title="Refresh files and Git" onClick={() => setRefresh(n => n + 1)}><Icon of={RefreshCw}/></button><button className="icon-btn" title="Hide repository" onClick={() => setShowGit(false)}><Icon of={X}/></button></div></div><GitPanel root={root || null} refreshKey={refresh}/></div>}
      </div></div></>}
    </div>
    <footer className="wsl-status"><span className={`wsl-dot${workspace ? " ready" : ""}`}/><span>{workspace?.distribution ?? "Connecting to Linux"}</span><span className="wsl-status-path">{homeAbbrev(root)}</span><span>WSL Preview</span></footer>
    {picker && <FolderPicker initial={root || workspace!.home} onClose={() => setPicker(false)} onPick={path => { setCwd(path); goHome(); setPicker(false); }}/ >}
    {settings && <div className="modal-backdrop"><section className="wsl-dialog" role="dialog" aria-modal="true" aria-label="Settings"><div className="panel-header"><span>SETTINGS</span><button className="icon-btn" title="Close settings" onClick={() => setSettings(false)}><Icon of={X}/></button></div><div className="wsl-settings"><nav><button className="set-nav-item on">Windows</button></nav><div><h2>Windows</h2><p className="wsl-hint">aiterm uses your default Linux distribution in WSL.</p><dl><dt>Distribution</dt><dd>{workspace?.distribution ?? "Not connected"}</dd><dt>Linux home</dt><dd>{workspace?.home ?? "—"}</dd></dl><label>Terminal font size <input type="number" min={10} max={24} value={fontSize} onChange={e => setFontSize(Math.max(10, Math.min(24, Number(e.target.value) || 14)))}/></label><p className="wsl-hint">Ctrl+Shift+T opens a terminal. Ctrl+S saves a file.</p></div></div></section></div>}
    {confirm && <div className="modal-backdrop"><section className="wsl-dialog" role="alertdialog" aria-modal="true" aria-label="Unsaved changes"><div className="home-start"><h2>Unsaved changes</h2><p>{confirm.message}</p><div className="wsl-dialog-actions"><button className="btn" onClick={() => setConfirm(undefined)}>Keep editing</button><button className="btn" onClick={() => { const action = confirm.action; setConfirm(undefined); action(); }}>Discard changes</button></div></div></section></div>}
  </div>;
}

createRoot(document.getElementById("root")!).render(<App/>);
