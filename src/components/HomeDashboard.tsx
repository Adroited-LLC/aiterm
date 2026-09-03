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
import { KeyboardEvent, ReactNode, useState } from "react";
import { Session, homeAbbrev } from "../ipc";
import type { TabId } from "../ipc";
import { Alert } from "./AlertBell";
import Icon from "./Icon";
import FleetBoard from "./FleetBoard";
import { useSpineOverview } from "../useSpineOverview";
import { CornerDownLeft, FolderOpen } from "lucide-react";

export default function HomeDashboard({
  sessions, liveIds, attentionIds, busyIds, otherAlerts, onSelect, onResume, onGoTab,
  onShowAll, controls, ready, cwd, onPickCwd, onLaunch,
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
  /** The engine / model / effort pickers — shared with the ＋ menu. */
  controls: ReactNode;
  /** Whether there is anything to start; the box still takes typing. */
  ready: boolean;
  /** The folder the session opens in, or null when none is known yet. */
  cwd: string | null;
  onPickCwd: () => void;
  /** Start a session with this as its first message — or an empty one. */
  onLaunch: (prompt: string) => void;
}) {
  // Only while this component is mounted, which is only while home is the
  // screen — see `useSpineOverview` on why a poll is the right shape here.
  const overview = useSpineOverview(true);
  const [prompt, setPrompt] = useState("");

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
        <div className="empty-start-controls">{controls}</div>
        <div className="home-prompt">
          <textarea
            className="home-prompt-input"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={onKey}
            placeholder={ready ? "What should it do?  Enter starts the session, Shift+Enter for a new line" : "Nothing to start yet — set up the API tab, or install claude, codex, grok or antigravity"}
            rows={3}
            autoFocus
            spellCheck={false}
          />
          <div className="home-prompt-foot">
            <button
              className="home-cwd"
              onClick={onPickCwd}
              title={cwd ? `Session opens in ${cwd} — click to change` : "Choose the folder the session opens in"}
            >
              <Icon of={FolderOpen} size="sm" />
              <span>{cwd ? homeAbbrev(cwd) : "Choose a folder…"}</span>
            </button>
            <button className="tui-pick home-go" onClick={go} disabled={!ready} title="Start the session (Enter)">
              {prompt.trim() ? "Start with this" : "Start empty"} <Icon of={CornerDownLeft} size="sm" />
            </button>
          </div>
        </div>
      </section>

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
