import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import DesktopRemoteTerminal from "./DesktopRemoteTerminal";
import { ArrowLeft, Monitor, Plus, Unplug, X, Search, Star, Play, MoreHorizontal, GitBranch, MessageSquare } from "lucide-react";
import { linuxPath } from "../platform";
import { type AppSettings } from "../settings";
import { desktopAttach, desktopCancelPairing, desktopConnect, desktopDisconnect, desktopFocus, desktopForget, desktopList, desktopPair, desktopRestore, desktopWatch, desktopSession, type RemoteClientView, type RemoteDesktop } from "../desktopRemote";
import { remoteSessionRows, filterRemoteSessions, type SessionRow } from "../desktopRemoteSessions";
import AgentIcon from "./AgentIcon";
import "@xterm/xterm/css/xterm.css";
import "./DesktopConnections.css";

export default function DesktopConnections({ settings, onClose }: { settings: AppSettings; onClose: () => void }) {
  const [desktops, setDesktops] = useState<RemoteDesktop[]>([]);
  const [view, setView] = useState<RemoteClientView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [deviceName, setDeviceName] = useState("My desktop");
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [menu, setMenu] = useState<string | null>(null);
  const [edit, setEdit] = useState<SessionRow | null>(null);
  const [title, setTitle] = useState("");
  const [confirm, setConfirm] = useState<{action: string; row: SessionRow} | null>(null);
  const historyRef = useRef<HTMLDivElement>(null);
  useEffect(() => { const el = historyRef.current; if (el) el.scrollTop = el.scrollHeight; }, [view?.preview_messages]);
  useEffect(() => { setMenu(null); setEdit(null); setConfirm(null); setQuery(""); }, [view?.desktop_id]);
  const alive = useRef(true);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { const node = dialog.current; node?.showModal(); return () => node?.close(); }, []);
  useEffect(() => {
    alive.current = true;
    let stopped = false;
    desktopList().then(setDesktops).catch(e => setError(String(e)));
    void (async () => {
      try { await desktopRestore(); } catch (e) { if (!stopped) setError(String(e)); }
      let revision: number | undefined;
      while (!stopped) {
        try {
          const next = await desktopWatch(revision);
          if (stopped) break;
          revision = next.revision; setView(next);
        } catch (e) {
          if (!stopped) { setError(String(e)); await new Promise(resolve => setTimeout(resolve, 2000)); }
        }
      }
    })();
    return () => { stopped = true; alive.current = false; };
  }, []);
  const run = async (action: () => Promise<unknown>) => {
    setError(null); setBusy(true);
    try { await action(); } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const pair = async () => {
    setError(null);
    try {
      const path = await open({ title: "Open AiTerm pairing file", multiple: false, directory: false, filters: [{ name: "AiTerm pairing", extensions: ["aiterm-pair"] }, { name: "All files", extensions: ["*"] }] });
      if (!path || typeof path !== "string") return;
      setPairing(true);
      const desktop = await desktopPair(linuxPath(path), deviceName.trim());
      if (alive.current) {
        setDesktops(await desktopList()); setAdding(false);
        await desktopConnect(desktop.id);
      }
    } catch (e) { if (alive.current) setError(String(e)); }
    finally { if (alive.current) setPairing(false); }
  };
  const selected = desktops.find(d => d.id === view?.desktop_id);
  const live = view?.tabs.filter(t => t.state === "running") ?? [];
  const connected = view?.connection === "connected";
  const active = live.find(t => t.id === view?.selected_tab);
  const rows = remoteSessionRows(view?.sessions ?? [], view?.tabs ?? [], view?.activity ?? {}, view?.stars ?? []);
  const filtered = filterRemoteSessions(rows, query, filter);
  const preview = rows.find(r => r.session?.id === view?.preview_session);
  const selectedRow = preview ?? rows.find(r => r.tab?.id === view?.selected_tab);
  const activate = (row: SessionRow) => void run(() => row.tab ? desktopAttach(row.tab.id) : desktopSession("preview", row.session!.id));
  const mutate = (action: string, row: SessionRow, value?: string) => void run(async () => {
    await desktopSession(action, row.session!.id, value, action === "star" ? !row.starred : undefined);
    setMenu(null); setEdit(null); setConfirm(null);
  });
  const sessionTitle = (tab: typeof live[number]) => {
    const session = view?.sessions.find(s => s.id === tab.sessionId);
    return session?.custom_title || session?.title || tab.title || "Terminal";
  };
  return <dialog ref={dialog} className="dc-overlay" onCancel={e => e.preventDefault()} aria-modal="true" aria-label="Connected desktops" onKeyDown={e => e.stopPropagation()}>
    <header className="dc-header" inert={!!edit || !!confirm}>
      <button className="dc-back" disabled={pairing} onClick={() => void run(async () => { await desktopDisconnect(); onClose(); })}><ArrowLeft size={16} />This desktop</button>
      <div className="dc-heading"><Monitor size={20} /><h2>Connected desktops</h2></div>
      <button className="dc-icon" aria-label="Close connected desktops" disabled={pairing} onClick={() => void run(async () => { await desktopDisconnect(); onClose(); })}><X size={19} /></button>
    </header>
    {(error || view?.error) && <div className="dc-error" role="alert">{error || view?.error}</div>}
    <div className="dc-body" inert={!!edit || !!confirm}>
      <aside className="dc-sidebar">
        <label className="dc-label" htmlFor="dc-desktop">Desktop</label>
        <select id="dc-desktop" value={view?.desktop_id ?? ""} disabled={pairing || busy} onChange={e => { void run(() => e.target.value ? desktopConnect(e.target.value) : desktopDisconnect()); }}>
          <option value="">Select a desktop</option>
          {desktops.map(d => <option key={d.id} value={d.id}>{d.name || d.address}</option>)}
        </select>
        <div className="dc-status"><span data-connected={connected} />{view?.connection || "disconnected"}</div>
        <button className="dc-add" onClick={() => setAdding(!adding)} disabled={pairing}><Plus size={16} />Pair another desktop</button>
        {adding && <div className="dc-pair">
          <p>On the other computer, open Settings → Remote Access and save a pairing file. Open it here, then approve this device there.</p>
          <label htmlFor="dc-name">This device’s name</label>
          <input id="dc-name" value={deviceName} maxLength={64} disabled={pairing} onChange={e => setDeviceName(e.target.value)} />
          <button className="dc-primary" disabled={pairing || !deviceName.trim()} onClick={() => void pair()}>{pairing ? "Waiting for approval…" : "Open pairing file"}</button>
          {pairing && <><p role="status">Approve the request on the other desktop. The request expires after five minutes.</p><button onClick={() => void desktopCancelPairing().catch(e => setError(String(e)))}>Cancel pairing</button></>}
        </div>}
        <div className="dc-list-heading">Sessions<span>{rows.length}</span></div>
        <label className="dc-search"><Search size={15} /><input aria-label="Search remote sessions" placeholder="Search sessions or projects" value={query} onChange={e => setQuery(e.target.value)} /></label>
        <div className="dc-filters" role="group" aria-label="Filter sessions">{[["all","All"],["live","Live"],["history","History"],["starred","Starred"]].map(([value,label]) => <button key={value} aria-pressed={filter === value} onClick={() => setFilter(value)}>{label}</button>)}</div>
        <nav className="dc-sessions" aria-label="Remote sessions">
          {filtered.map(row => <div key={row.key} className="dc-session-row">
            <button className={selectedRow?.key === row.key ? "selected" : ""} disabled={!connected || busy} onClick={() => activate(row)} onContextMenu={e => { if (row.session) { e.preventDefault(); setMenu(menu === row.key ? null : row.key); } }}>
              <span className="dc-row-title">{row.session ? <AgentIcon agent={row.session.agent} size={15} /> : <Monitor size={15} />}<strong>{row.title}</strong>{row.starred && <Star size={12} fill="currentColor" />}</span>
              <small title={row.path}>{row.path}</small>
              <span className="dc-row-meta"><span className="dc-phase" data-phase={row.status}>{row.status}</span>{row.session?.branch && <small><GitBranch size={11} />{row.session.branch}</small>}</span>
            </button>
            {row.session && <button className="dc-row-menu" aria-label={`Actions for ${row.title}`} aria-expanded={menu === row.key} disabled={!connected || busy} onClick={() => setMenu(menu === row.key ? null : row.key)}><MoreHorizontal size={16} /></button>}
            {menu === row.key && row.session && <div className="dc-session-menu" aria-label={`Session actions for ${row.title}`}>
              <button disabled={busy} onClick={() => { setMenu(null); activate(row); }}>{row.tab ? "Open terminal" : "View history"}</button>
              {row.tab && <button disabled={busy} onClick={() => { setMenu(null); void run(() => desktopSession("preview", row.session!.id)); }}>View conversation</button>}
              {!row.tab && view?.agent_caps?.[row.session.agent]?.resume && <button disabled={busy} onClick={() => mutate("open", row)}>Resume session</button>}
              <button disabled={busy} onClick={() => mutate("star", row)}>{row.starred ? "Unstar" : "Star"}</button>
              <button disabled={busy} onClick={() => { setTitle(row.title); setEdit(row); setMenu(null); }}>Rename</button>
              {view?.agent_caps?.[row.session.agent]?.fork && <button disabled={busy} onClick={() => mutate("fork", row)}>Fork session</button>}
              {row.status !== "History" && <button disabled={busy} onClick={() => { setConfirm({action: row.tab ? "close" : "stop", row}); setMenu(null); }}>{row.tab ? "Close session" : "Stop session"}</button>}
              {row.status === "History" && view?.agent_caps?.[row.session.agent]?.delete && <button disabled={busy} onClick={() => { setConfirm({action:"delete",row}); setMenu(null); }}>Delete session…</button>}
            </div>}
          </div>)}
          {!filtered.length && <p className="dc-muted">{!connected ? "Connect to a desktop to see its sessions." : rows.length ? "No sessions match this filter." : "No saved sessions on this desktop yet."}</p>}
        </nav>
        {selected && <div className="dc-desktop-actions"><span title={selected.fingerprint}>{selected.address}</span><button disabled={busy} onClick={() => void run(() => desktopDisconnect())}><Unplug size={14} />Disconnect</button><button disabled={busy} onClick={() => void run(async () => { await desktopForget(selected.id); setDesktops(await desktopList()); })}>Forget pairing</button></div>}
      </aside>
      <main className="dc-workspace">
        {preview && view ? <>
          <div className="dc-session-header"><div><h3>{preview.title}</h3><span>{preview.path} · {preview.status}</span></div>
            {preview.tab ? <button className="dc-primary" disabled={!connected || busy} onClick={() => activate(preview)}><Monitor size={15} />Open terminal</button> : view.agent_caps?.[preview.session!.agent]?.resume ? <button className="dc-primary" disabled={!connected || busy} onClick={() => mutate("open", preview)}><Play size={15} />Resume session</button> : <span className="dc-muted">This agent does not support resuming</span>}
          </div>
          <div className="dc-conversation" ref={historyRef}>
            {view.preview_loading ? <p className="dc-muted" role="status">Loading conversation…</p> : view.preview_messages.length ? view.preview_messages.map((message, index) => <article key={index} data-role={message.role}><span>{message.role === "user" ? "You" : message.role === "assistant" ? preview.session?.agent || "Assistant" : message.role}</span><div>{message.text}</div></article>) : <div className="dc-empty"><MessageSquare size={28} /><p>No conversation text is available for this session.</p></div>}
          </div>
        </> : active && view ? <>
          <div className="dc-session-header"><div><h3>{sessionTitle(active)}</h3><span>{selectedRow?.status || "Open"} · {view.has_focus ? "You have input control" : "Type to take control · Scroll for history"}</span></div>
            <button className="dc-primary" disabled={!connected || busy || !view.screen} onClick={() => void run(() => view.has_focus ? desktopAttach(active.id) : desktopFocus())}>{view.has_focus ? "Give back control" : "Take control"}</button>
          </div>
          <DesktopRemoteTerminal key={`${view.desktop_id}:${view.selected_tab}`} view={view} settings={settings} onError={setError} />
        </> : <div className="dc-empty"><Monitor size={38} strokeWidth={1.3} /><h3>Your sessions, on another desktop</h3><p>{desktops.length ? "Select a session to connect to its terminal or read its history. Saved sessions can be resumed here." : "Pair a desktop to browse its sessions. Sessions stay on the computer where they started."}</p>{!desktops.length && <button className="dc-primary" onClick={() => setAdding(true)}>Pair a desktop</button>}</div>}
      </main>
    </div>
    {(edit || confirm) && <div className="dc-prompt-shade" onKeyDown={e => { if (e.key === "Escape" && !busy) { e.preventDefault(); setEdit(null); setConfirm(null); } }}><section className="dc-prompt" role="dialog" aria-modal="true" aria-label={edit ? "Rename remote session" : "Confirm session action"}>
      {edit ? <form onSubmit={e => { e.preventDefault(); mutate("rename", edit, title.trim()); }}><h3>Rename session</h3><label htmlFor="dc-session-title">Session name</label><input id="dc-session-title" autoFocus maxLength={256} value={title} onChange={e => setTitle(e.target.value)} disabled={busy} /><div><button type="button" disabled={busy} onClick={() => setEdit(null)}>Cancel</button><button className="dc-primary" disabled={busy || !title.trim()}>Save name</button></div></form> : confirm && <><h3>{confirm.action === "delete" ? "Delete session?" : "Stop this session?"}</h3><p>{confirm.action === "delete" ? `Move “${confirm.row.title}” to the remote desktop’s trash?` : `Stop “${confirm.row.title}” on the remote desktop? Its conversation history will be kept.`}</p><div><button autoFocus disabled={busy} onClick={() => setConfirm(null)}>Cancel</button><button className="dc-primary" disabled={busy || !connected || (confirm.action === "delete" && rows.find(r => r.key === confirm.row.key)?.status !== "History")} onClick={() => mutate(confirm.action, confirm.row)}>{confirm.action === "delete" ? "Delete session" : "Stop session"}</button></div></>}
      {error && <p role="alert">{error}</p>}
    </section></div>}
  </dialog>;
}
