/**
 * The centre pane when nothing is open: a launcher, not a blank.
 *
 * What it answers, in the order someone arriving asks: what do I want done
 * (the prompt box, with the engine, model and effort in view above it — one
 * glance, then Enter, and the session opens already working on it); what was
 * I doing (recent sessions, one click back in); is anything waiting on me
 * (sessions blocked on input). Usage is not here — the strip in the top bar
 * already says it. Projects are not here either: a list of folders was a
 * second way to do what the sidebar tree already does.
 *
 * Everything here is already in memory — the session list, the alerts. This
 * draws; it fetches nothing.
 */
import { KeyboardEvent, ReactNode, useState } from "react";
import { Session, homeAbbrev } from "../ipc";
import type { TabId } from "../ipc";
import { fmtTime, fullTime, useTimeFormat } from "../timefmt";
import { Alert } from "./AlertBell";
import AgentIcon from "./AgentIcon";
import Icon from "./Icon";
import { agentTint } from "../brand";
import { Bell, CornerDownLeft, FolderOpen, GitBranch, History, Play } from "lucide-react";

export default function HomeDashboard({
  sessions, liveIds, alerts, onSelect, onResume, onGoTab, controls, ready, cwd, onPickCwd, onLaunch,
}: {
  sessions: Session[];
  /** Session ids with a live terminal right now. */
  liveIds: Set<string>;
  alerts: Alert[];
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
  onGoTab: (key: TabId) => void;
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
  const { format: timeFormat } = useTimeFormat();
  const when = (ms: number) => fmtTime(ms, timeFormat);
  const recent = [...sessions].sort((a, b) => b.last_active - a.last_active);
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
            placeholder={ready ? "What should it do?  Enter starts the session, Shift+Enter for a new line" : "Nothing to start yet — set up the API tab, or install claude, codex or grok"}
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

      {alerts.length > 0 && (
        <section className="home-card home-waiting">
          <div className="home-card-head">
            <Icon of={Bell} /> <h2>Waiting for you</h2>
            <span className="home-count">{alerts.length}</span>
          </div>
          <div className="home-list">
            {alerts.slice(0, 5).map((a) => (
              <div key={a.key} className="home-row" onClick={() => onGoTab(a.key)}>
                <span className="home-row-text">
                  <span className="home-row-title">{a.title}</span>
                  {a.message && <span className="home-row-sub">{a.message}</span>}
                </span>
                <span className="home-row-age" title={fullTime(a.at)}>{when(a.at)}</span>
              </div>
            ))}
          </div>
        </section>
      )}

      <section className="home-card home-recent">
        <div className="home-card-head">
          <Icon of={History} /> <h2>Pick up where you left off</h2>
        </div>
        {recent.length === 0 ? (
          <div className="home-empty">No sessions yet — start one above.</div>
        ) : (
          <div className="home-list">
            {recent.slice(0, 6).map((s) => {
              const tint = agentTint(s.agent);
              const live = liveIds.has(s.id);
              return (
                <div key={s.id} className="home-row" onClick={() => onSelect(s)} title={s.title}>
                  <span className={"home-badge" + tint.className} style={tint.style}>
                    <AgentIcon agent={s.agent} size={13} />
                    {live && <span className="live-dot badge-dot" />}
                  </span>
                  <span className="home-row-text">
                    <span className="home-row-title">{s.title}</span>
                    <span className="home-row-sub">
                      {homeAbbrev(s.project_path)}
                      {s.branch && <span className="home-branch"><Icon of={GitBranch} size="sm" />{s.branch}</span>}
                    </span>
                  </span>
                  <span className="home-row-age" title={fullTime(s.last_active)}>{live ? "live" : when(s.last_active)}</span>
                  <button
                    className="home-row-go"
                    title={live ? "Switch to it" : "Resume it"}
                    onClick={(e) => { e.stopPropagation(); live ? onSelect(s) : onResume(s); }}
                  ><Icon of={Play} size="sm" /></button>
                </div>
              );
            })}
          </div>
        )}
      </section>
      </div>
    </div>
  );
}
