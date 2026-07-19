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
}

interface Props {
  tab: TermTab;
  active: boolean;
  onExit: (key: number) => void;
}

export default function TerminalView({ tab, active, onExit }: Props) {
  const elRef = useRef<HTMLDivElement>(null);
  const started = useRef(false);
  const fitRef = useRef<FitAddon | null>(null);
  const ptyIdRef = useRef<number | null>(null);

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
      unlistenOut = await listen<string>(`pty://output/${id}`, (e) => term.write(e.payload));
      unlistenExit = await listen<{ id: number }>("pty://exit", (e) => {
        if (e.payload.id === id) onExit(tab.key);
      });
      term.onData((data) => ptyWrite(id, data));
      term.onResize(({ cols, rows }) => ptyResize(id, cols, rows));
      term.focus();
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
      if (ptyIdRef.current !== null) ptyKill(ptyIdRef.current);
      term.dispose();
      started.current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (active) {
      fitRef.current?.fit();
      const term = elRef.current?.querySelector<HTMLElement>(".xterm-helper-textarea");
      term?.focus();
    }
  }, [active]);

  return <div ref={elRef} className="term-host" style={{ display: active ? "block" : "none" }} />;
}
