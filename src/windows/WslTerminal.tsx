import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { RotateCcw, TerminalSquare } from "lucide-react";
import "@xterm/xterm/css/xterm.css";


export type Workspace = { distribution: string; home: string; shell: string };
type Event =
  | { type: "ready"; version: number; home: string; shell: string; pid: number }
  | { type: "output"; sequence: number; data: string }
  | { type: "exit"; code: number | null; signal: string | null }
  | { type: "error"; message: string };
type Request = { type: "input"; data: string } | { type: "resize"; cols: number; rows: number } | { type: "ack"; sequence: number } | { type: "close" };

export default function WslTerminal({ id, cwd, command, active, fontSize, onEnd, onReady }: {
  id: string; cwd: string; command: string | null; active: boolean; fontSize: number;
  onEnd: (id: string) => void;
  onReady: (id: string) => void;
}) {
  const terminal = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const endedCallback = useRef(onEnd);
  endedCallback.current = onEnd;
  const readyCallback = useRef(onReady);
  readyCallback.current = onReady;
  const activeRef = useRef(active);
  activeRef.current = active;
  useEffect(() => {
    if (terminal.current) terminal.current.options.fontSize = fontSize;
    if (active) requestAnimationFrame(() => { fitRef.current?.fit(); terminal.current?.focus(); });
  }, [active, fontSize]);
  const host = useRef<HTMLDivElement>(null);
  const [attempt, setAttempt] = useState(0);
  const [workspace, setWorkspace] = useState<Workspace>();
  const [status, setStatus] = useState<"starting" | "ready" | "ended" | "error">("starting");
  const [detail, setDetail] = useState("");
  useEffect(() => {
    if (!host.current) return;
    // A retry owns a new connection. A late cleanup cannot close its successor.
    const connectionId = id + ":" + attempt;
    let disposed = false;
    let connected = false;
    let ended = false;
    let queue = Promise.resolve();
    const fail = (error: unknown) => {
      if (!disposed) { connected = false; setDetail(String(error)); setStatus("error"); }
    };
    const send = (request: Request) => {
      queue = queue.then(() => disposed ? undefined : invoke<void>("terminal_request", { id: connectionId, request })).catch(fail);
    };
    const term = new Terminal({
      fontFamily: '"Cascadia Mono", Consolas, monospace', fontSize,
      lineHeight: 1.15, cursorBlink: true, scrollback: 10000,
      theme: { background: "#121317", foreground: "#d8dae5", cursor: "#d8dae5", selectionBackground: "#3a4054", black: "#16171d", red: "#e06c75", green: "#98c379", yellow: "#e5c07b", blue: "#61afef", magenta: "#c678dd", cyan: "#56b6c2", white: "#d8dae5" },
    });
    const fit = new FitAddon(); terminal.current = term; fitRef.current = fit; term.loadAddon(fit); term.open(host.current); fit.fit();
    const events = new Channel<Event>();
    events.onmessage = event => {
      if (disposed) return;
      if (event.type === "output") {
        const bytes = Uint8Array.from(atob(event.data), c => c.charCodeAt(0));
        // Acknowledge after xterm consumes the bytes, bounding output in transit.
        term.write(bytes, () => send({ type: "ack", sequence: event.sequence }));
      } else if (event.type === "exit") {
        ended = true;
        endedCallback.current(id);
        connected = false; setStatus("ended");
        setDetail(event.signal ? `Terminal ended (${event.signal})` : `Terminal ended${event.code ? ` with exit code ${event.code}` : ""}`);
      } else if (event.type === "error") { ended = true; fail(event.message); }
    };
    const start = async () => {
      setStatus("starting"); setDetail("");
      await invoke("terminal_request", { id: connectionId, request: { type: "close" } });
      if (disposed) return;
      const value = await invoke<Workspace>("start_terminal", { id: connectionId, cwd, command, cols: Math.max(2, term.cols), rows: Math.max(1, term.rows), events });
      if (disposed) { await invoke("terminal_request", { id: connectionId, request: { type: "close" } }); return; }
      setWorkspace(value);
      if (!ended) { connected = true; setStatus("ready"); readyCallback.current(id); if (activeRef.current) { fit.fit(); term.focus(); } }
    };
    void start().catch(fail);
    const input = term.onData(data => {
      if (!connected) return;
      const bytes = new TextEncoder().encode(data);
      // Keep large pastes below the protocol frame limit and preserve ordering.
      for (let offset = 0; offset < bytes.length; offset += 16384) {
        send({ type: "input", data: btoa(String.fromCharCode(...bytes.subarray(offset, offset + 16384))) });
      }
    });
    const binary = term.onBinary(data => { if (connected) send({ type: "input", data: btoa(data) }); });
    const resize = term.onResize(({ cols, rows }) => { if (connected) send({ type: "resize", cols, rows }); });
    const observer = new ResizeObserver(() => { if (host.current?.clientWidth && host.current?.clientHeight) fit.fit(); }); observer.observe(host.current);
    return () => { disposed = true; observer.disconnect(); input.dispose(); binary.dispose(); resize.dispose(); term.dispose(); void invoke("terminal_request", { id: connectionId, request: { type: "close" } }).catch(() => {}); };
  }, [attempt]);

  return <div className="wsl-terminal">
    <div className="terminal-host" ref={host} aria-label="Linux terminal"/>
    {status === "starting" && <div className="workspace-overlay" role="status"><TerminalSquare size={28}/><h2>Opening your workspace</h2><p>Starting your terminal in Linux.</p></div>}
    {status === "error" && <div className="workspace-overlay"><h2>Let’s reconnect your workspace</h2><p>{detail}</p><button className="btn" onClick={() => setAttempt(n => n + 1)}><RotateCcw size={14}/> Try again</button></div>}
    {status === "ended" && <div className="wsl-terminal-ended"><span>{detail}</span><button className="btn" onClick={() => setAttempt(n => n + 1)}>Restart terminal</button></div>}
    <span className="wsl-terminal-label">{workspace?.distribution}</span>
  </div>;
}
