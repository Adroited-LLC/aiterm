import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { ptySpawn, ptyWrite, ptyResize, ptyKill } from "../ipc";
import "@xterm/xterm/css/xterm.css";

export interface TermTab {
  key: number;
  title: string;
  cwd: string | null;
  command: string | null;
  /** Claude session id when this tab was opened via resume. */
  sessionId?: string;
}

/** Control surface a mounted terminal registers with the app. */
export interface TermHandle {
  /** Send raw bytes to the PTY. */
  write: (data: string) => void;
  /** Send composed input from the bottom input box (adds Enter, wraps
   *  multiline text in bracketed paste when the running app supports it). */
  sendComposed: (text: string) => void;
}

interface Props {
  tab: TermTab;
  active: boolean;
  onExit: (key: number) => void;
  onRegister: (key: number, handle: TermHandle | null) => void;
  onActivity: (key: number) => void;
  /** Focus the terminal itself when it becomes active (composer hidden). */
  autoFocus: boolean;
}

export default function TerminalView({
  tab, active, onExit, onRegister, onActivity, autoFocus,
}: Props) {
  const elRef = useRef<HTMLDivElement>(null);
  const started = useRef(false);
  const fitRef = useRef<FitAddon | null>(null);
  const ptyIdRef = useRef<number | null>(null);
  const termRef = useRef<Terminal | null>(null);

  useEffect(() => {
    if (!elRef.current || started.current) return;
    started.current = true;

    const term = new Terminal({
      fontFamily: "'JetBrainsMono Nerd Font', 'JetBrains Mono', 'Fira Code', monospace",
      fontSize: 13,
      cursorBlink: true,
      allowProposedApi: true,
      theme: {
        background: "#121317",
        foreground: "#d8dae5",
        cursor: "#da7756",
        selectionBackground: "#33364180",
        black: "#1c1e24",
        red: "#e06c75",
        green: "#98c379",
        yellow: "#e5c07b",
        blue: "#61afef",
        magenta: "#c678dd",
        cyan: "#56b6c2",
        white: "#d8dae5",
        brightBlack: "#5c6370",
      },
    });
    const fit = new FitAddon();
    fitRef.current = fit;
    termRef.current = term;
    term.loadAddon(fit);
    term.open(elRef.current);
    fit.fit();

    let unlistenOut: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;
    let disposed = false;

    (async () => {
      const id = await ptySpawn(tab.cwd, tab.command, term.cols, term.rows);
      if (disposed) {
        ptyKill(id);
        return;
      }
      ptyIdRef.current = id;
      unlistenOut = await listen<string>(`pty://output/${id}`, (e) => {
        term.write(e.payload);
        onActivity(tab.key);
      });
      unlistenExit = await listen<{ id: number }>("pty://exit", (e) => {
        if (e.payload.id === id) onExit(tab.key);
      });
      term.onData((data) => ptyWrite(id, data));
      term.onResize(({ cols, rows }) => ptyResize(id, cols, rows));

      onRegister(tab.key, {
        write: (data) => ptyWrite(id, data),
        sendComposed: (text) => {
          const bracketed = term.modes.bracketedPasteMode;
          let payload: string;
          if (text.includes("\n")) {
            payload = bracketed
              ? `\x1b[200~${text}\x1b[201~\r`
              : text.replace(/\n/g, "\r") + "\r";
          } else {
            payload = text + "\r";
          }
          ptyWrite(id, payload);
        },
      });
    })();

    const ro = new ResizeObserver(() => {
      if (elRef.current && elRef.current.offsetWidth > 0) fit.fit();
    });
    ro.observe(elRef.current);

    return () => {
      disposed = true;
      ro.disconnect();
      unlistenOut?.();
      unlistenExit?.();
      onRegister(tab.key, null);
      if (ptyIdRef.current !== null) ptyKill(ptyIdRef.current);
      term.dispose();
      started.current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (active) {
      fitRef.current?.fit();
      if (autoFocus) termRef.current?.focus();
    }
  }, [active, autoFocus]);

  return <div ref={elRef} className="term-host" style={{ display: active ? "block" : "none" }} />;
}
