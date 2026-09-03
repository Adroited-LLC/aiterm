/**
 * The centre pane when nothing is open: a launcher, not a blank.
 *
 * What it answers, in the order someone arriving asks: what do I want done
 * (the prompt box, with the engine, model and effort in view above it — one
 * glance, then Enter, and the session opens already working on it), and what
 * is the fleet doing (`FleetBoard`, below it: who is blocked on you, who is
 * mid-turn, what you were last in). Projects are not here as a list of
 * folders — that was a second way to do what the sidebar tree already does.
 *
 * The one thing this fetches is the spine's snapshot, and only while it is
 * mounted. Everything else — the sessions, the alerts, the usage — is state
 * App already holds and passes down.
 */
import { KeyboardEvent, ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { Session, homeAbbrev } from "../ipc";
import type { TabId } from "../ipc";
import { Alert } from "./AlertBell";
import Icon from "./Icon";
import FleetBoard from "./FleetBoard";
import HomeUsage from "./HomeUsage";
import type { UsageSourceAt } from "./UsagePanel";
import { useSpineOverview } from "../useSpineOverview";
import {
  ChevronDown, CornerDownLeft, Folder, FolderOpen, SlidersHorizontal, TerminalSquare,
} from "lucide-react";

/** How many folders the working-folder menu offers before "Browse…". */
const RECENT_FOLDERS = 8;

/**
 * The working folder, as a control rather than a chip in a footer.
 *
 * Clicking it drops the folders you have actually worked in — taken from the
 * session list, newest first — because nine times in ten the folder you want
 * is one you were in yesterday, and a native directory dialog is a slow way
 * to say that. Browsing is still one item down the list.
 */
function FolderControl({
  cwd, folders, onPick, onSet,
}: {
  cwd: string | null;
  folders: string[];
  onPick: () => void;
  onSet: (path: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    // `KeyboardEvent` here is React's — the import shadows the DOM one — so
    // the listener takes the DOM type by its global name.
    const esc = (e: globalThis.KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <div className="home-folder" ref={box}>
      <button
        className="home-field"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        title={cwd ? `The session opens in ${cwd}` : "Choose the folder the session opens in"}
      >
        <Icon of={FolderOpen} size="sm" className="home-field-lead" />
        <span className="home-field-value">{cwd ? homeAbbrev(cwd) : "Choose a folder…"}</span>
        <Icon of={ChevronDown} size="sm" className="home-field-caret" />
      </button>
      {open && (
        <div className="home-menu" role="menu">
          {folders.map((f) => (
            <button
              key={f}
              role="menuitem"
              className={"home-menu-item" + (f === cwd ? " on" : "")}
              title={f}
              onClick={() => { onSet(f); setOpen(false); }}
            >
              <Icon of={Folder} size="sm" />
              <span>{homeAbbrev(f)}</span>
            </button>
          ))}
          {folders.length > 0 && <div className="home-menu-rule" />}
          <button
            role="menuitem"
            className="home-menu-item"
            onClick={() => { setOpen(false); onPick(); }}
          >
            <Icon of={FolderOpen} size="sm" />
            <span>Browse…</span>
          </button>
        </div>
      )}
    </div>
  );
}

export default function HomeDashboard({
  sessions, liveIds, attentionIds, busyIds, otherAlerts, onSelect, onResume, onGoTab,
  onShowAll, controls, pickers, pickerSummary, usage, ready, cwd, onPickCwd, onSetCwd,
  onLaunch, onOpenTerminal,
}: {
  sessions: Session[];
  /** Session ids with a live terminal right now. */
  liveIds: Set<string>;
  /** Sessions whose tab rang the bell — the board's fallback for needs-you
   *  while the spine has no log for them. */
  attentionIds: Set<string>;
  /** Sessions whose tab is reporting progress — the fallback for working. */
  busyIds: Set<string>;
  /** Waiting tabs that are not sessions (a plain shell), which the board
   *  shows under "Needs you" beside the sessions. */
  otherAlerts: Alert[];
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
  onGoTab: (key: TabId) => void;
  /** Open the sidebar, where the whole list already lives. */
  onShowAll: () => void;
  /** The engine segmented control — `StartControls only="tabs"`. */
  controls: ReactNode;
  /** The model and effort pickers — `StartControls only="selects"`. Behind
   *  the summary row until it is opened. */
  pickers: ReactNode;
  /** Those two in one line, for the collapsed row. */
  pickerSummary: string;
  /** The same reading the top bar's strip draws — passed, never fetched. */
  usage: UsageSourceAt[];
  /** Whether there is anything to start; the box still takes typing. */
  ready: boolean;
  /** The folder the session opens in, or null when none is known yet. */
  cwd: string | null;
  onPickCwd: () => void;
  /** Choose one of the folders already worked in, without a dialog. */
  onSetCwd: (path: string) => void;
  /** Start a session with this as its first message — or an empty one. */
  onLaunch: (prompt: string) => void;
  /** A plain shell in the working folder — no engine, no session. */
  onOpenTerminal: () => void;
}) {
  // Only while this component is mounted, which is only while home is the
  // screen — see `useSpineOverview` on why a poll is the right shape here.
  const overview = useSpineOverview(true);
  const [prompt, setPrompt] = useState("");
  /** Collapsed by default: the two pickers are for the session you want to be
   *  different, and every other session is the engine's own defaults. */
  const [showPickers, setShowPickers] = useState(false);
  /** Distinct project folders, newest first — the working-folder menu. */
  const folders = useMemo(() => {
    const seen: string[] = [];
    for (const s of [...sessions].sort((a, b) => b.last_active - a.last_active)) {
      const p = s.group_path || s.project_path;
      if (p && !seen.includes(p)) seen.push(p);
      if (seen.length >= RECENT_FOLDERS) break;
    }
    return seen;
  }, [sessions]);

  const go = () => {
    onLaunch(prompt);
    setPrompt("");
  };
  // Enter sends, Shift+Enter is a newline — the same gesture as the engines'
  // own composers, so nothing new to learn.
  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      go();
    }
  };

  return (
    <div className="home">
      <div className="home-inner">
      <section className="home-card home-start">
        <FolderControl cwd={cwd} folders={folders} onPick={onPickCwd} onSet={onSetCwd} />

        <div className="home-engines">{controls}</div>

        <button
          className="home-field home-pickers-toggle"
          aria-expanded={showPickers}
          onClick={() => setShowPickers((v) => !v)}
          title="Model and effort for this session"
        >
          <Icon of={SlidersHorizontal} size="sm" className="home-field-lead" />
          <span className="home-field-value">{pickerSummary}</span>
          <Icon of={ChevronDown} size="sm" className={"home-field-caret" + (showPickers ? " up" : "")} />
        </button>
        {showPickers && <div className="home-pickers">{pickers}</div>}

        <div className="home-prompt">
          <textarea
            className="home-prompt-input"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={onKey}
            placeholder={ready
              ? "What should it do?"
              : "Nothing to start yet — set up the API tab, or install claude, codex, grok or antigravity"}
            rows={4}
            autoFocus
            spellCheck={false}
          />
        </div>

        <div className="home-actions">
          {/* One label whether or not anything is typed. The button used to
              rename itself "Start empty" / "Start with this", which made the
              primary action look like two different actions and moved under
              the pointer as you typed. What Enter does is in the tooltip. */}
          <button
            className="tui-pick home-go"
            onClick={go}
            disabled={!ready}
            title="Start the session — Enter starts it, Shift+Enter makes a new line"
          >
            Start <Icon of={CornerDownLeft} size="sm" />
          </button>
          <button
            className="home-quiet"
            onClick={onOpenTerminal}
            title="Open a plain shell in the working folder — no engine, no session"
          >
            <Icon of={TerminalSquare} size="sm" /> Open terminal only
          </button>
        </div>
      </section>

      <HomeUsage sources={usage} />

      <FleetBoard
        sessions={sessions}
        overview={overview}
        liveIds={liveIds}
        attentionIds={attentionIds}
        busyIds={busyIds}
        otherAlerts={otherAlerts}
        onSelect={onSelect}
        onResume={onResume}
        onGoTab={onGoTab}
        onShowAll={onShowAll}
      />

      </div>
    </div>
  );
}
