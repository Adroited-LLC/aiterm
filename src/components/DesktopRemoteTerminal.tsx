import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { ArrowDown } from "lucide-react";
import { type AppSettings, termFontFamily, termTheme } from "../settings";
import { desktopType, desktopResize, desktopScrollback, remoteScreenAnsi, type RemoteClientView } from "../desktopRemote";
import { makeWriteQueue } from "../writeQueue";
import { onTerminalInput } from "../terminalUserInput";
import type { RemoteRow } from "../desktopRemoteScreen";

/** Keep the host's grid intact. Scrolling this viewport never sends input or
 * resizes someone else's terminal; only the input owner can change its size. */
export default function DesktopRemoteTerminal({ view, settings, onError }: { view: RemoteClientView; settings: AppSettings; onError: (error: string) => void }) {
  const viewport = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal | null>(null);
  const current = useRef(view); current.current = view;
  const error = useRef(onError); error.current = onError;
  const follow = useRef(true);
  const [atLive, setAtLive] = useState(true);
  const [rows, setRows] = useState<RemoteRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [exhausted, setExhausted] = useState(false);
  const historyState = useRef({ pending: false, offset: 0, exhausted: false, generation: 0 });
  const anchor = useRef<{ top: number; height: number } | null>(null);
  const resize = useRef<() => void>(() => {});

  const goLive = () => {
    historyState.current = { pending: false, offset: 0, exhausted: false, generation: historyState.current.generation + 1 };
    anchor.current = null; setRows([]); setLoading(false); setExhausted(false);
    follow.current = true; setAtLive(true);
    if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
    terminal.current?.focus();
  };
  const loadOlder = async () => {
    const state = historyState.current;
    if (state.pending || state.exhausted || current.current.connection !== "connected" || !current.current.screen) return;
    state.pending = true; setLoading(true);
    const generation = state.generation;
    try {
      const page = await desktopScrollback(state.offset);
      if (generation !== historyState.current.generation) return;
      const node = viewport.current;
      // Reveal fetched lines on the first gesture, including when the live grid fits.
      if (node) anchor.current = { top: node.scrollTop - settings.termFontSize * 6, height: node.scrollHeight };
      follow.current = false; setAtLive(false);
      state.offset += page.length;
      state.exhausted = page.length < 100 || state.offset >= 10000;
      setExhausted(state.exhausted);
      setRows(previous => [...page].reverse().concat(previous));
    } catch (e) {
      if (generation === historyState.current.generation) error.current(String(e));
    } finally {
      if (generation === historyState.current.generation) { state.pending = false; setLoading(false); }
    }
  };
  const older = useRef(loadOlder); older.current = loadOlder;

  useLayoutEffect(() => {
    const node = viewport.current;
    if (node && anchor.current) {
      node.scrollTop = anchor.current.top + node.scrollHeight - anchor.current.height;
      anchor.current = null;
    }
  }, [rows]);

  useEffect(() => () => { historyState.current.generation++; }, []);

  useEffect(() => {
    const node = viewport.current;
    const host = canvas.current;
    if (!node || !host) return;
    const term = new Terminal({ fontFamily: termFontFamily(settings), fontSize: settings.termFontSize, theme: termTheme(settings), scrollback: 0, cursorBlink: false, disableStdin: true });
    term.open(host); terminal.current = term;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const requestSize = () => {
      const v = current.current;
      const screen = host.querySelector<HTMLElement>(".xterm-screen");
      if (!screen || !v.has_focus || v.connection !== "connected") return;
      const width = screen.getBoundingClientRect().width / term.cols;
      const height = screen.getBoundingClientRect().height / term.rows;
      if (!width || !height) return;
      const cols = Math.min(512, Math.max(2, Math.floor((node.clientWidth - 40) / width)));
      const rows = Math.min(512, Math.max(2, Math.floor((node.clientHeight - 32) / height)));
      if (cols !== v.screen?.cols || rows !== v.screen?.rows) void desktopResize(cols, rows).catch(e => { if (!disposed) error.current(String(e)); });
    };
    resize.current = () => { clearTimeout(timer); timer = setTimeout(requestSize, 200); };
    const observer = new ResizeObserver(() => resize.current()); observer.observe(node);
    const rendered = term.onRender(() => {
      if (follow.current) node.scrollTop = node.scrollHeight;
    });
    // xterm normally consumes wheel events even without local scrollback. Use
    // one native scroll container for the live grid and earlier output instead.
    const wheel = (event: WheelEvent) => {
      if (event.ctrlKey) return; // preserve browser/accessibility zoom
      event.stopPropagation();
      event.preventDefault();
      const unit = event.deltaMode === 1 ? settings.termFontSize : event.deltaMode === 2 ? node.clientHeight : 1;
      if (event.deltaY < 0) {
        follow.current = false; setAtLive(false);
        if (node.scrollTop <= 40) void older.current();
      }
      node.scrollTop += event.deltaY * unit;
      node.scrollLeft += event.deltaX * unit;
    };
    node.addEventListener("wheel", wheel, { capture: true, passive: false });
    const type = makeWriteQueue<{ attachment: string; cols: number; rows: number }>(async (target, value) => {
      if (disposed || current.current.connection !== "connected" || current.current.attachment_id !== target.attachment) throw new Error("Terminal connection changed; input was not replayed");
      await desktopType(value, target.cols, target.rows, target.attachment);
    }, target => target.attachment);
    const data = onTerminalInput(term, (value, user) => {
      const v = current.current;
      if (!user || v.connection !== "connected" || !v.attachment_id) return;
      const screen = host.querySelector<HTMLElement>(".xterm-screen");
      if (!screen) return;
      const rect = screen.getBoundingClientRect();
      const cols = Math.min(512, Math.max(2, Math.floor((node.clientWidth - 40) / (rect.width / term.cols))));
      const rows = Math.min(512, Math.max(2, Math.floor((node.clientHeight - 32) / (rect.height / term.rows))));
      goLive();
      void type({attachment: v.attachment_id, cols, rows}, value).catch(e => { if (!disposed) error.current(String(e)); });
    });
    return () => {
      disposed = true; clearTimeout(timer); observer.disconnect(); rendered.dispose(); data.dispose();
      node.removeEventListener("wheel", wheel, true); terminal.current = null; term.dispose();

    };
  }, [settings.themeId, settings.termFont, settings.termFontSize]);

  useEffect(() => {
    const term = terminal.current;
    if (!term) return;
    term.options.disableStdin = view.connection !== "connected" || !view.attachment_id;
    if (view.screen) {
      if (term.cols !== view.screen.cols || term.rows !== view.screen.rows) term.resize(view.screen.cols, view.screen.rows);
      term.write(remoteScreenAnsi(view.screen));
    } else term.write("\x1b[0m\x1b[2J\x1b[H");
  }, [view.screen?.revision, view.screen?.tab_id, view.has_focus, view.attachment_id, view.connection, settings.themeId, settings.termFont, settings.termFontSize]);

  useEffect(() => { goLive(); }, [view.connection]);

  useEffect(() => {
    if (view.has_focus && view.connection === "connected") { goLive(); resize.current(); }
  }, [view.has_focus, view.connection]);

  return <div className="dc-terminal-frame">
    <div className="dc-terminal" ref={viewport} role="region" aria-label="Remote terminal and history" tabIndex={0}
      onScroll={() => {
        const node = viewport.current;
        if (!node || anchor.current) return;
        const live = node.scrollHeight - node.clientHeight - node.scrollTop < 4;
        follow.current = live; setAtLive(live);
      }} onKeyDown={event => {
        if (event.target !== viewport.current) return;
        if (event.key === "Home" && event.ctrlKey) { event.preventDefault(); viewport.current.scrollTop = 0; void loadOlder(); }
        if (event.key === "End" && event.ctrlKey) { event.preventDefault(); goLive(); }
      }}>
      {rows.length > 0 && <div className="dc-scrollback" style={{ fontFamily: termFontFamily(settings), fontSize: settings.termFontSize }}>
        <button disabled={loading || exhausted} onClick={() => void loadOlder()}>{loading ? "Loading earlier output…" : exhausted ? "Beginning of available history" : "Earlier output"}</button>
        <pre>{rows.map(row => row.cells.filter(c => !c.continuation).map(c => c.text.replace(/[\x00-\x1f\x7f-\x9f]/g, "") || " ".repeat(c.width)).join("")).join("\n") + "\n"}</pre>
      </div>}
      <div className="dc-terminal-canvas" ref={canvas} onClick={() => terminal.current?.focus()} />
    </div>
    {!atLive && <button className="dc-return-live" onClick={goLive}><ArrowDown size={14} />Back to live</button>}
    {exhausted && !rows.length && !atLive && <span className="dc-history-loading" role="status">No earlier terminal output.</span>}
    {loading && <span className="dc-history-loading" role="status">Loading earlier output…</span>}
  </div>;
}
