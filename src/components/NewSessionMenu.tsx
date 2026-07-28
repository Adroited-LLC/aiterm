import { useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { homeAbbrev } from "../ipc";

/** A directory offered as somewhere to start a session. */
export interface StartPoint {
  path: string;
  /** Directory name, for the row's title line. */
  name: string;
  /** Last activity — a session's, or the directory's own mtime for somewhere
   *  no session has run. Newest first, so the menu opens on where you were. */
  at: number;
}

interface Props {
  /** Where sessions have run before, plus known project directories. */
  places: StartPoint[];
  /** Start claude in this directory. */
  onPick: (path: string) => void;
  onClose: () => void;
}

/**
 * Pick a directory to start a fresh claude session in.
 *
 * Two ways in, because the two cases are genuinely different. Almost always
 * you want somewhere you have worked before, and typing a path for that is
 * absurd — so the list is every directory aiterm already knows about, filtered
 * as you type. The first time you work somewhere new there is nothing to pick,
 * so the last row hands off to the OS folder chooser.
 *
 * The list is not restricted to Claude *projects*: `list_projects` only reads
 * `~/Projects`, and a machine where the work lives elsewhere would get an
 * empty menu. Anywhere a session has ever run counts, which is the same set
 * the sidebar is already showing rows for.
 */
export default function NewSessionMenu({ places, onPick, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const popRef = useRef<HTMLDivElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  // Click anywhere else closes. Except on the ＋ itself, which owns the
  // toggle: closing here as well would close the menu on pointerdown and let
  // the button's click reopen it a moment later, so the button that opens
  // this menu could never be the button that shuts it.
  useEffect(() => {
    const away = (e: PointerEvent) => {
      const el = e.target as HTMLElement | null;
      if (popRef.current?.contains(el)) return;
      if (el?.closest?.("[data-new-session-trigger]")) return;
      onClose();
    };
    window.addEventListener("pointerdown", away, true);
    return () => window.removeEventListener("pointerdown", away, true);
  }, [onClose]);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q
      ? places.filter(
          (p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q),
        )
      : places;
    // Long enough to be worth scrolling is long enough to be worth trimming:
    // the folder chooser is right there for anything further down.
    return list.slice(0, 12);
  }, [places, query]);

  useEffect(() => setCursor(0), [query]);

  const browse = async () => {
    // Close first: the native chooser is modal, and leaving a popover
    // hovering behind it looks like the app forgot to.
    onClose();
    try {
      const picked = await open({ directory: true, title: "Start a session in…" });
      if (typeof picked === "string") onPick(picked);
    } catch {
      /* the chooser was cancelled or unavailable — nothing to report */
    }
  };

  // `matches.length` is the Browse row: one past the end of the list.
  const choose = (i: number) => (i >= matches.length ? browse() : onPick(matches[i].path));

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, matches.length));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(cursor);
    }
  };

  return (
    <div className="new-session-pop" ref={popRef} onKeyDown={onKeyDown}>
      <input
        ref={inputRef}
        className="new-session-input"
        placeholder="Start a session in…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      <div className="new-session-list">
        {matches.map((p, i) => (
          <div
            key={p.path}
            className={"new-session-row" + (i === cursor ? " on" : "")}
            onPointerEnter={() => setCursor(i)}
            onClick={() => onPick(p.path)}
          >
            <span className="new-session-name">{p.name}</span>
            <span className="new-session-path">{homeAbbrev(p.path)}</span>
          </div>
        ))}
        {matches.length === 0 && (
          <div className="empty-note">
            {query.trim() ? "No matching directory" : "No directories known yet"}
          </div>
        )}
        <div
          className={"new-session-row browse" + (cursor === matches.length ? " on" : "")}
          onPointerEnter={() => setCursor(matches.length)}
          onClick={browse}
        >
          <span className="new-session-name">Browse for a folder…</span>
        </div>
      </div>
    </div>
  );
}
