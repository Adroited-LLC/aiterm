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
  /** Dedupe key linking this terminal to a sidebar item: a session id for
   *  resumes, "shell:<path>" for project shells. One terminal per slot. */
  slotId: string;
}

/** Control surface a mounted terminal registers with the app. */
export interface TermHandle {
  /** Send raw bytes to the PTY. */
  write: (data: string) => void;
  /** Force a clean TUI repaint (SIGWINCH jiggle) — Ctrl+Shift+L. */
  redraw: () => void;
  /** Paste text: bracketed when the running app enabled paste mode, so TUIs
   *  (claude turns image paths into [Image #N]) can tell it from typing. */
  paste: (text: string) => void;
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
  /** Bell = the program wants eyes (claude prompts ring it); typing clears. */
  onAttention: (key: number, on: boolean) => void;
  /** Focus the terminal itself when it becomes active (composer hidden). */
  autoFocus: boolean;
  fontSize: number;
  fontFamily: string;
  theme: Record<string, string>;
}

export default function TerminalView({
  tab, active, onExit, onRegister, onActivity, onAttention, autoFocus, fontSize, fontFamily, theme,
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
      fontFamily,
      fontSize,
      cursorBlink: true,
      allowProposedApi: true,
      theme,
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

    // Startup nudge: claude's TUI paints while panels/window-state are
    // still settling and only repaints on SIGWINCH, leaving stale lines on
    // screen until something resizes. Once the FIRST output burst goes
    // quiet (a fixed delay fires too early — resuming a big session can
    // take seconds to paint), resize one column down and fit back — two
    // SIGWINCHs force a clean full redraw, same as a manual window-drag.
    let jiggled = false;
    let quietTimer: number | null = null;
    const doJiggle = () => {
      if (term.cols > 2) {
        term.resize(term.cols - 1, term.rows);
        window.setTimeout(() => {
          fit.fit();
          term.refresh(0, term.rows - 1);
        }, 80);
      }
    };
    const scheduleJiggle = () => {
      if (jiggled) return;
      if (quietTimer !== null) clearTimeout(quietTimer);
      quietTimer = window.setTimeout(() => {
        if (jiggled || disposed) return;
        jiggled = true;
        doJiggle();
      }, 800);
    };
    scheduleJiggle();

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
        // A clear-screen means a full view transition (claude's agents
        // view, etc.) — those can strand stale rows, so re-arm the
        // quiet-time cleanup nudge.
        if (e.payload.includes("\x1b[2J") || e.payload.includes("\x1b[3J")) {
          jiggled = false;
        }
        scheduleJiggle();
      });
      unlistenExit = await listen<{ id: number }>("pty://exit", (e) => {
        if (e.payload.id === id) onExit(tab.key);
      });
      term.onBell(() => onAttention(tab.key, true));
      term.onData((data) => {
        onAttention(tab.key, false);
        ptyWrite(id, data);
      });
      term.onResize(({ cols, rows }) => ptyResize(id, cols, rows));

      onRegister(tab.key, {
        write: (data) => ptyWrite(id, data),
        redraw: doJiggle,
        paste: (text) =>
          ptyWrite(
            id,
            term.modes.bracketedPasteMode ? `\x1b[200~${text}\x1b[201~` : text,
          ),
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

    // Debounce resize→fit: splitter drags fire the observer continuously,
    // and a SIGWINCH storm makes TUIs (claude's input box) redraw over
    // half-painted frames. One fit at the end + a full repaint instead.
    let fitTimer: number | null = null;
    const ro = new ResizeObserver(() => {
      if (fitTimer !== null) clearTimeout(fitTimer);
      fitTimer = window.setTimeout(() => {
        fitTimer = null;
        if (elRef.current && elRef.current.offsetWidth > 0) {
          fit.fit();
          term.refresh(0, term.rows - 1);
        }
      }, 120);
    });
    ro.observe(elRef.current);

    return () => {
      disposed = true;
      if (quietTimer !== null) clearTimeout(quietTimer);
      if (fitTimer !== null) clearTimeout(fitTimer);
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
      const t = termRef.current;
      t?.refresh(0, t.rows - 1);
      if (autoFocus) termRef.current?.focus();
    }
  }, [active, autoFocus]);

  useEffect(() => {
    if (termRef.current && termRef.current.options.fontSize !== fontSize) {
      termRef.current.options.fontSize = fontSize;
      fitRef.current?.fit();
    }
  }, [fontSize]);

  useEffect(() => {
    const term = termRef.current;
    if (term && term.options.fontFamily !== fontFamily) {
      term.options.fontFamily = fontFamily;
      fitRef.current?.fit();
    }
  }, [fontFamily]);

  useEffect(() => {
    if (termRef.current) termRef.current.options.theme = theme;
  }, [theme]);

  return <div ref={elRef} className="term-host" style={{ display: active ? "block" : "none" }} />;
}
