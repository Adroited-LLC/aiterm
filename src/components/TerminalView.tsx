import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Channel } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { ptySpawn, ptyWrite, ptyResize, ptyKill } from "../ipc";
import "@xterm/xterm/css/xterm.css";

/** Attach the GPU WebGL renderer. It owns its own surface and clears+repaints
 *  every cell each frame — which is what kills the stale-cell ghosting the
 *  built-in DOM renderer left behind. Guarded so that if WebGL can't init (no
 *  GPU context) the terminal is never worse than the DOM default. On GPU
 *  context loss, dispose so xterm reverts to the DOM renderer rather than
 *  freezing on a dead canvas. */
function attachRenderer(term: Terminal) {
  try {
    const webgl = new WebglAddon();
    webgl.onContextLoss(() => webgl.dispose());
    term.loadAddon(webgl);
  } catch {
    /* WebGL unavailable — xterm's built-in DOM renderer remains */
  }
}

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
  /** A session aiterm started that has not written its transcript yet.
   *
   *  The sidebar is the tab list, so a tab with no row in it is a tab you
   *  cannot get back to — and claude writes nothing to disk until the first
   *  prompt. This marks the window in between, so the panel can list the tab
   *  on the strength of aiterm having started it. It stays set once the
   *  transcript lands; what retires the placeholder is the real session
   *  appearing under the same id, not this flag being cleared. */
  fresh?: boolean;
  /** Set while this tab is still waiting to learn its session id.
   *
   *  Only agents that have no `--session-id` — Codex — get one of these. They
   *  cannot be told what to call themselves, so the tab opens with no session
   *  id at all and watches for the transcript that appears in the directory it
   *  launched in. Cleared the moment that id is adopted, which is also what
   *  retires the placeholder row and stops the conversation owning two. */
  adopt?: {
    agentId: string;
    /** Unix millis at launch; nothing older than this can be ours. */
    since: number;
    /** Session ids the sidebar already listed, so adoption cannot take over a
     *  conversation that was open before this tab existed. */
    known: string[];
  };
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
  /** Put the keyboard back in the terminal — used when a chrome element that
   *  took focus (a composer pill panel) is dismissed. */
  focus: () => void;
  /** Whether unsent text appears to be sitting in the running program's input
   *  line. Typing a slash command into a non-empty prompt concatenates onto
   *  what is there and submits the lot, so the pills ask before doing it. */
  pendingInput: () => boolean;
  /** The visible screen as text, one string per row. xterm has already parsed
   *  the escape codes, so this is rendered characters rather than a guess at a
   *  byte stream. Used to recognise TUI screens worth replacing with a real
   *  dialog. */
  screen: () => string[];
}

interface Props {
  tab: TermTab;
  active: boolean;
  /** `code` is the child's exit status: 0 means the user left, anything else
   *  (or null, if it couldn't be reaped) means it died on its own. `signal` is
   *  set instead when it was killed — portable-pty reports code 1 for every
   *  signal, so the code alone cannot tell a SIGKILL from `exit 1`. */
  onExit: (key: number, code: number | null, signal: string | null) => void;
  onRegister: (key: number, handle: TermHandle | null) => void;
  onActivity: (key: number) => void;
  /** Bell = the program wants eyes (claude prompts ring it); typing clears. */
  onAttention: (key: number, on: boolean) => void;
  /** Focus the terminal when it becomes active. Once true only while the
   *  composer was hidden, back when the composer held a text input that would
   *  have been fighting for the same keystrokes. It is a pill strip now, so
   *  the terminal is always where typing should land. */
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
    attachRenderer(term);
    fit.fit();

    let unlistenExit: UnlistenFn | null = null;
    let disposed = false;

    // Manual repaint escape hatch (Ctrl+Shift+L): a clean refit + full
    // re-render. With an accelerated renderer this is rarely needed — no
    // window/grid jiggle, no SIGWINCH storm.
    const redraw = () => {
      fit.fit();
      term.refresh(0, term.rows - 1);
    };

    // PTY output arrives as raw bytes over a binary Channel. Write the
    // Uint8Array straight to xterm (it keeps a persistent UTF-8 decoder, so
    // multibyte chars split across chunks are reassembled correctly). Set
    // onmessage before spawning so no early output is missed.
    const onOutput = new Channel<ArrayBuffer>();
    onOutput.onmessage = (chunk) => {
      term.write(new Uint8Array(chunk));
      onActivity(tab.key);
    };

    (async () => {
      const id = await ptySpawn(tab.cwd, tab.command, term.cols, term.rows, onOutput);
      if (disposed) {
        ptyKill(id);
        return;
      }
      ptyIdRef.current = id;
      unlistenExit = await listen<{
        id: number;
        code: number | null;
        signal: string | null;
      }>("pty://exit", (e) => {
        if (e.payload.id === id) {
          onExit(tab.key, e.payload.code ?? null, e.payload.signal ?? null);
        }
      });
      // Roughly how much unsent text is sitting in the running program's input
      // line. Every keystroke passes through here on its way to the PTY, which
      // is enough to answer the only question that matters: is the prompt
      // empty right now?
      //
      // Deliberately approximate. Printable keys and pastes add, backspace
      // removes, Enter and Ctrl+C reset. Cursor keys and word-kill are not
      // modelled, so this can report text pending when the line is actually
      // clear. That is the safe direction to be wrong in — the cost is a
      // confirmation you did not need, never a half-written prompt sent.
      let pending = 0;

      term.onBell(() => onAttention(tab.key, true));
      term.onData((data) => {
        onAttention(tab.key, false);
        if (data === "\r" || data === "\n" || data === "\x03") pending = 0;
        else if (data === "\x7f" || data === "\b") pending = Math.max(0, pending - 1);
        else if (data >= " ") pending += data.length;
        ptyWrite(id, data);
      });
      term.onResize(({ cols, rows }) => ptyResize(id, cols, rows));

      onRegister(tab.key, {
        write: (data) => ptyWrite(id, data),
        redraw,
        paste: (text) => {
          pending += text.length;
          ptyWrite(
            id,
            term.modes.bracketedPasteMode ? `\x1b[200~${text}\x1b[201~` : text,
          );
        },
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
          pending = 0; // this call ends in Enter, so the line is spent
          ptyWrite(id, payload);
        },
        focus: () => term.focus(),
        pendingInput: () => pending > 0,
        screen: () => {
          const buf = term.buffer.active;
          const rows: string[] = [];
          for (let i = 0; i < term.rows; i++) {
            const line = buf.getLine(buf.viewportY + i);
            rows.push(line ? line.translateToString(true) : "");
          }
          return rows;
        },
      });
    })();

    // Debounce resize→fit: splitter drags fire the observer continuously, and
    // a SIGWINCH storm makes TUIs (claude's input box) redraw over half-painted
    // frames. One fit at the end; the accelerated renderer repaints itself.
    let fitTimer: number | null = null;
    const ro = new ResizeObserver(() => {
      if (fitTimer !== null) clearTimeout(fitTimer);
      fitTimer = window.setTimeout(() => {
        fitTimer = null;
        if (elRef.current && elRef.current.offsetWidth > 0) fit.fit();
      }, 120);
    });
    ro.observe(elRef.current);

    return () => {
      disposed = true;
      if (fitTimer !== null) clearTimeout(fitTimer);
      ro.disconnect();
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
