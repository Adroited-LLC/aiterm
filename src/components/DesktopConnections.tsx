import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowLeft, Monitor, Plus, Unplug, X } from "lucide-react";
import { linuxPath } from "../platform";
import { type AppSettings, termFontFamily, termTheme } from "../settings";
import { desktopAttach, desktopCancelPairing, desktopConnect, desktopDisconnect, desktopFocus, desktopForget, desktopInput, desktopList, desktopPair, desktopResize, desktopScrollback, desktopWatch, remoteScreenAnsi, type RemoteClientView, type RemoteDesktop } from "../desktopRemote";
import "@xterm/xterm/css/xterm.css";
import "./DesktopConnections.css";

function LiveTerminal({ view, settings, onError }: { view: RemoteClientView; settings: AppSettings; onError: (error: string) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const current = useRef(view); current.current = view;
  const error = useRef(onError); error.current = onError;
  useEffect(() => {
    if (!host.current) return;
    const term = new Terminal({ fontFamily: termFontFamily(settings), fontSize: settings.termFontSize, theme: termTheme(settings), scrollback: 0, cursorBlink: false, allowProposedApi: false, disableStdin: true });
    const fit = new FitAddon(); fitRef.current = fit; term.loadAddon(fit); term.open(host.current); terminal.current = term;
    let timer: ReturnType<typeof setTimeout>;
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        const v = current.current;
        if (!v.has_focus || v.connection !== "connected") return;
        const size = fit.proposeDimensions();
        if (size && (size.cols !== v.screen?.cols || size.rows !== v.screen?.rows)) void desktopResize(Math.min(512, Math.max(2, size.cols)), Math.min(512, Math.max(2, size.rows))).catch(e => error.current(String(e)));
      }, 200);
    });
    observer.observe(host.current);
    const data = term.onData(value => {
      if (current.current.has_focus && current.current.connection === "connected") void desktopInput(value).catch(e => error.current(String(e)));
    });
    return () => { clearTimeout(timer); observer.disconnect(); data.dispose(); terminal.current = null; fitRef.current = null; term.dispose(); };
    // Recreate on theme/font changes, not every remote revision.
  }, [settings.themeId, settings.termFont, settings.termFontSize]);
  useEffect(() => {
    const term = terminal.current;
    if (!term) return;
    term.options.disableStdin = !view.has_focus || view.connection !== "connected";
    if (view.screen) {
      if (term.cols !== view.screen.cols || term.rows !== view.screen.rows) term.resize(view.screen.cols, view.screen.rows);
      term.write(remoteScreenAnsi(view.screen));
    } else { term.clear(); term.write("\x1b[2J\x1b[H"); }
  }, [view.screen?.revision, view.screen?.tab_id, view.has_focus, view.connection, settings.themeId, settings.termFont, settings.termFontSize]);
  useEffect(() => {
    if (!view.has_focus || view.connection !== "connected") return;
    const size = fitRef.current?.proposeDimensions();
    if (size && (size.cols !== view.screen?.cols || size.rows !== view.screen?.rows)) void desktopResize(Math.min(512, Math.max(2, size.cols)), Math.min(512, Math.max(2, size.rows))).catch(e => error.current(String(e)));
    terminal.current?.focus();
  }, [view.has_focus, view.connection]);
  return <div className="dc-terminal" ref={host} onClick={() => terminal.current?.focus()} />;
}

export default function DesktopConnections({ settings, onClose }: { settings: AppSettings; onClose: () => void }) {
  const [desktops, setDesktops] = useState<RemoteDesktop[]>([]);
  const [view, setView] = useState<RemoteClientView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [deviceName, setDeviceName] = useState("My desktop");
  const [history, setHistory] = useState(false);
  const [offset, setOffset] = useState(0);
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { const node = dialog.current; node?.showModal(); return () => node?.close(); }, []);
  useEffect(() => {
    alive.current = true;
    let stopped = false;
    desktopList().then(setDesktops).catch(e => setError(String(e)));
    void (async () => {
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
      const path = await open({ title: "Open AiTerm pairing file", multiple: false, directory: false, filters: [{ name: "AiTerm pairing", extensions: ["aiterm-pair"] }] });
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
  const sessionTitle = (tab: typeof live[number]) => {
    const session = view?.sessions.find(s => s.id === tab.sessionId);
    return session?.custom_title || session?.title || tab.title || "Terminal";
  };
  return <dialog ref={dialog} className="dc-overlay" onCancel={e => e.preventDefault()} aria-modal="true" aria-label="Connected desktops" onKeyDown={e => e.stopPropagation()}>
    <header className="dc-header">
      <button className="dc-back" disabled={pairing} onClick={() => void run(async () => { await desktopDisconnect(); onClose(); })}><ArrowLeft size={16} />This desktop</button>
      <div className="dc-heading"><Monitor size={20} /><h2>Connected desktops</h2></div>
      <button className="dc-icon" aria-label="Close connected desktops" disabled={pairing} onClick={() => void run(async () => { await desktopDisconnect(); onClose(); })}><X size={19} /></button>
    </header>
    {(error || view?.error) && <div className="dc-error" role="alert">{error || view?.error}</div>}
    <div className="dc-body">
      <aside className="dc-sidebar">
        <label className="dc-label" htmlFor="dc-desktop">Desktop</label>
        <select id="dc-desktop" value={view?.desktop_id ?? ""} disabled={pairing || busy} onChange={e => { setHistory(false); void run(() => e.target.value ? desktopConnect(e.target.value) : desktopDisconnect()); }}>
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
        <div className="dc-list-heading">Live sessions<span>{live.length}</span></div>
        <nav className="dc-sessions" aria-label="Remote live sessions">
          {live.map(tab => <button key={tab.id} className={tab.id === view?.selected_tab ? "selected" : ""} disabled={!connected || busy} onClick={() => { setHistory(false); void run(() => desktopAttach(tab.id)); }}><strong>{sessionTitle(tab)}</strong><small title={tab.cwd}>{tab.cwd}</small></button>)}
          {!live.length && <p className="dc-muted">{connected ? "No live sessions. Open a session on this desktop to see it here." : "Connect to a desktop to see its live sessions."}</p>}
        </nav>
        {selected && <div className="dc-desktop-actions"><span title={selected.fingerprint}>{selected.address}</span><button disabled={busy} onClick={() => void run(() => desktopDisconnect())}><Unplug size={14} />Disconnect</button><button disabled={busy} onClick={() => void run(async () => { await desktopForget(selected.id); setDesktops(await desktopList()); })}>Forget pairing</button></div>}
      </aside>
      <main className="dc-workspace">
        {active && view ? <>
          <div className="dc-session-header"><div><h3>{sessionTitle(active)}</h3><span>{view.has_focus ? "You have input control" : "Viewing live"}</span></div>
            <button disabled={!connected || busy || !view.screen} onClick={() => void run(async () => { if (!history) { await desktopScrollback(0); setOffset(0); } setHistory(!history); })}>{history ? "Back to live" : "History"}</button>
            <button className="dc-primary" disabled={!connected || busy || view.has_focus || !view.screen} onClick={() => void run(() => desktopFocus())}>{view.has_focus ? "In control" : "Take control"}</button>
          </div>
          {history ? <div className="dc-history"><button disabled={busy || view.scrollback.length < 100} onClick={() => void run(async () => { const next = offset + 100; await desktopScrollback(next); setOffset(next); })}>Older</button>{offset > 0 && <button disabled={busy} onClick={() => void run(async () => { const next = Math.max(0, offset - 100); await desktopScrollback(next); setOffset(next); })}>Newer</button>}<pre>{view.scrollback.length ? [...view.scrollback].reverse().map(row => row.cells.filter(c => !c.continuation).map(c => c.text).join("")).join("\n") : "No earlier terminal output."}</pre></div> : <LiveTerminal view={view} settings={settings} onError={setError} />}
        </> : <div className="dc-empty"><Monitor size={38} strokeWidth={1.3} /><h3>Your sessions, on another desktop</h3><p>{desktops.length ? "Choose a desktop and select a live session to view or control it here." : "Pair a desktop to work with its live sessions. Sessions stay on the computer where they started."}</p>{!desktops.length && <button className="dc-primary" onClick={() => setAdding(true)}>Pair a desktop</button>}</div>}
      </main>
    </div>
  </dialog>;
}
