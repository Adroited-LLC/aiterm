import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { Channel, invoke } from "@tauri-apps/api/core";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { RotateCcw, TerminalSquare } from "lucide-react";
import "@xterm/xterm/css/xterm.css";
import "./windows.css";

type Workspace = { distribution: string; home: string; shell: string };
type Event =
  | { type: "ready"; version: number; home: string; shell: string; pid: number }
  | { type: "output"; sequence: number; data: string }
  | { type: "exit"; code: number | null; signal: string | null }
  | { type: "error"; message: string };
type Request = { type: "input"; data: string } | { type: "resize"; cols: number; rows: number } | { type: "ack"; sequence: number } | { type: "close" };

function App() {
  const host = useRef<HTMLDivElement>(null);
  const [attempt, setAttempt] = useState(0);
  const [workspace, setWorkspace] = useState<Workspace>();
  const [status, setStatus] = useState<"starting" | "ready" | "ended" | "error">("starting");
  const [detail, setDetail] = useState("");
  useEffect(() => {
    if (!host.current) return;
    let disposed = false;
    let connected = false;
    let ended = false;
    let queue = Promise.resolve();
    const fail = (error: unknown) => {
      if (!disposed) { connected = false; setDetail(String(error)); setStatus("error"); }
    };
    const send = (request: Request) => {
      queue = queue.then(() => disposed ? undefined : invoke<void>("terminal_request", { request })).catch(fail);
    };
    const term = new Terminal({
      fontFamily: '"Cascadia Mono", Consolas, monospace', fontSize: 14,
      lineHeight: 1.15, cursorBlink: true, scrollback: 10000,
      theme: { background: "#121317", foreground: "#d8dae5", cursor: "#d8dae5", selectionBackground: "#3a4054", black: "#16171d", red: "#e06c75", green: "#98c379", yellow: "#e5c07b", blue: "#61afef", magenta: "#c678dd", cyan: "#56b6c2", white: "#d8dae5" },
    });
    const fit = new FitAddon(); term.loadAddon(fit); term.open(host.current); fit.fit();
    const events = new Channel<Event>();
    events.onmessage = event => {
      if (disposed) return;
      if (event.type === "output") {
        const bytes = Uint8Array.from(atob(event.data), c => c.charCodeAt(0));
        // Acknowledge after xterm consumes the bytes, bounding output in transit.
        term.write(bytes, () => send({ type: "ack", sequence: event.sequence }));
      } else if (event.type === "exit") {
        ended = true;
        connected = false; setStatus("ended");
        setDetail(event.signal ? `Terminal ended (${event.signal})` : `Terminal ended${event.code ? ` with exit code ${event.code}` : ""}`);
      } else if (event.type === "error") { ended = true; fail(event.message); }
    };
    const start = async () => {
      setStatus("starting"); setDetail("");
      await invoke("terminal_request", { request: { type: "close" } });
      if (disposed) return;
      const value = await invoke<Workspace>("start_terminal", { cols: term.cols, rows: term.rows, events });
      if (disposed) { await invoke("terminal_request", { request: { type: "close" } }); return; }
      setWorkspace(value);
      if (!ended) { connected = true; setStatus("ready"); fit.fit(); term.focus(); }
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
    const observer = new ResizeObserver(() => fit.fit()); observer.observe(host.current);
    return () => { disposed = true; observer.disconnect(); input.dispose(); binary.dispose(); resize.dispose(); term.dispose(); void invoke("terminal_request", { request: { type: "close" } }); };
  }, [attempt]);

  return <main className="workspace">
    <header className="workspace-bar">
      <div className="wordmark"><TerminalSquare size={18} strokeWidth={1.7}/><span>aiterm</span></div>
      <div className="workspace-name">{workspace?.distribution ?? "Linux workspace"}</div>
      <span className="preview-label">Preview</span>
    </header>
    <div className="terminal-region">
      <div className="terminal-host" ref={host} aria-label="Linux terminal"/>
      {status === "starting" && <div className="workspace-overlay" role="status"><div className="startup-mark"><TerminalSquare size={30} strokeWidth={1.4}/></div><h1>Opening your workspace</h1><p>Starting Linux and preparing your terminal.</p></div>}
      {status === "error" && <div className="workspace-overlay"><h1>Let’s reconnect your workspace</h1><p>Check that your Linux distribution opens, then try again.</p><button onClick={() => setAttempt(n => n + 1)}><RotateCcw size={15}/>Try again</button><details><summary>Technical details</summary><pre>{detail}</pre></details></div>}
    </div>
    <footer className="workspace-footer" aria-live="polite">
      <span className={`status-dot ${status}`}/><span>{status === "ready" ? workspace?.home : status === "ended" ? detail : status === "starting" ? "Starting workspace" : "Connection needs attention"}</span>
      {status === "ended" && <button onClick={() => setAttempt(n => n + 1)}>Open a new terminal</button>}
    </footer>
  </main>;
}

// Each effect owns a real process; do not double-mount it with StrictMode.
createRoot(document.getElementById("root")!).render(<App/>);
