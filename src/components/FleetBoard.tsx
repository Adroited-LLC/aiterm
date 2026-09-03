/**
 * Every session at once, sorted by how much it wants from you.
 *
 * This replaced "Pick up where you left off", which was the sidebar's list a
 * second time and said nothing the sidebar did not. The board says the one
 * thing the sidebar cannot: what each session is *doing right now* — the
 * permission it is blocked on, the tool it is in the middle of, how long the
 * turn has been open — because the spine now knows, and nothing on the
 * desktop was showing it.
 *
 * The grouping rules live in `../fleet.ts` and are tested there; this file
 * draws them. Rows are buttons: they are the primary way into a session from
 * here, and a div with an onClick is not reachable from a keyboard.
 */
import { useEffect, useState } from "react";
import { Session, homeAbbrev } from "../ipc";
import type { TabId } from "../ipc";
import { fmtTime, fullTime, useTimeFormat } from "../timefmt";
import { Alert } from "./AlertBell";
import AgentIcon from "./AgentIcon";
import Icon from "./Icon";
import { agentTint } from "../brand";
import { buildFleet, elapsed, type FleetRow } from "../fleet";
import type { SpineOverview } from "../ipc";
import { Bell, ChevronRight, GitBranch, History, Loader, Play } from "lucide-react";

/** How many idle rows before the board stops and offers the sidebar instead. */
const RECENT_CAP = 12;

interface Props {
  sessions: Session[];
  overview: Map<string, SpineOverview>;
  /** Sessions with a terminal open. */
  liveIds: Set<string>;
  /** Sessions whose tab rang the bell — the fallback for needs-you. */
  attentionIds: Set<string>;
  /** Sessions whose tab reports progress — the fallback for working. */
  busyIds: Set<string>;
  /** Waiting tabs that are not sessions at all (a plain shell). They belong
   *  in "Needs you" as much as any session does; they just have no row to
   *  hang off, so they get their own. */
  otherAlerts: Alert[];
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
  onGoTab: (key: TabId) => void;
  /** Open the sidebar on everything — where the full list already lives. */
  onShowAll: () => void;
}

/** A second hand, running only while something is actually being timed. */
function useSecond(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [on]);
  return now;
}

function Row({
  row, now, onSelect, onResume,
}: {
  row: FleetRow;
  now: number;
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
}) {
  const { format } = useTimeFormat();
  const s = row.session;
  const tint = agentTint(s.agent);
  const age = elapsed(row.since, now);
  return (
    <div className="fleet-row-wrap">
      <button className="fleet-row" onClick={() => onSelect(s)} title={s.title}>
        <span className={"home-badge" + tint.className} style={tint.style}>
          <AgentIcon agent={s.agent} size={14} />
          {row.phase !== "idle" && <span className={"fleet-dot " + row.phase} />}
        </span>
        <span className="fleet-row-text">
          <span className="fleet-row-title">{s.title}</span>
          <span className="fleet-row-sub">
            <span className="fleet-row-where">{homeAbbrev(s.group_path || s.project_path)}</span>
            {s.branch && (
              <span className="fleet-branch"><Icon of={GitBranch} size="sm" />{s.branch}</span>
            )}
            {row.detail && <span className="fleet-row-detail">{row.detail}</span>}
          </span>
        </span>
        <span className="fleet-row-when" title={fullTime(s.last_active)}>
          {age || (row.live ? "live" : fmtTime(s.last_active, format))}
        </span>
      </button>
      <button
        className="fleet-row-go"
        title={row.live ? "Switch to it" : "Resume it"}
        onClick={() => (row.live ? onSelect(s) : onResume(s))}
      ><Icon of={Play} size="sm" /></button>
    </div>
  );
}

export default function FleetBoard({
  sessions, overview, liveIds, attentionIds, busyIds, otherAlerts,
  onSelect, onResume, onGoTab, onShowAll,
}: Props) {
  const { format } = useTimeFormat();
  const fleet = buildFleet({
    sessions,
    overview,
    live: liveIds,
    attention: attentionIds,
    busy: busyIds,
    cap: RECENT_CAP,
  });
  const now = useSecond(fleet.running.some((r) => r.since !== null));
  const waiting = fleet.needsYou.length + otherAlerts.length;
  const nothing =
    waiting === 0 && fleet.running.length === 0 && fleet.recent.length === 0;

  if (nothing) {
    return (
      <section className="fleet">
        <div className="fleet-empty">No sessions yet — start one above.</div>
      </section>
    );
  }

  return (
    <section className="fleet">
      {waiting > 0 && (
        <div className="fleet-group needs">
          <h2 className="fleet-head">
            <Icon of={Bell} size="sm" /> Needs you
            <span className="fleet-count">{waiting}</span>
          </h2>
          <div className="fleet-rows">
            {fleet.needsYou.map((r) => (
              <Row key={r.session.id} row={r} now={now} onSelect={onSelect} onResume={onResume} />
            ))}
            {otherAlerts.map((a) => (
              <div className="fleet-row-wrap" key={a.key}>
                <button className="fleet-row" onClick={() => onGoTab(a.key)} title={a.title}>
                  <span className="home-badge">
                    <Icon of={Bell} size="sm" />
                    <span className="fleet-dot needs_you" />
                  </span>
                  <span className="fleet-row-text">
                    <span className="fleet-row-title">{a.title}</span>
                    <span className="fleet-row-sub">
                      <span className="fleet-row-detail">{a.message ?? "Waiting for your input"}</span>
                    </span>
                  </span>
                  <span className="fleet-row-when" title={fullTime(a.at)}>{fmtTime(a.at, format)}</span>
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {fleet.running.length > 0 && (
        <div className="fleet-group running">
          <h2 className="fleet-head">
            <Icon of={Loader} size="sm" /> Running
            <span className="fleet-count">{fleet.running.length}</span>
          </h2>
          <div className="fleet-rows">
            {fleet.running.map((r) => (
              <Row key={r.session.id} row={r} now={now} onSelect={onSelect} onResume={onResume} />
            ))}
          </div>
        </div>
      )}

      {fleet.recent.length > 0 && (
        <div className="fleet-group recent">
          <h2 className="fleet-head">
            <Icon of={History} size="sm" /> Recent
            {fleet.idleTotal > fleet.idleShown && (
              <button className="fleet-all" onClick={onShowAll}>
                Show all {fleet.idleTotal} <Icon of={ChevronRight} size="sm" />
              </button>
            )}
          </h2>
          {fleet.recent.map((g) => (
            <div className="fleet-proj" key={g.path}>
              <div className="fleet-proj-head" title={g.path}>{homeAbbrev(g.path)}</div>
              <div className="fleet-rows">
                {g.rows.map((r) => (
                  <Row key={r.session.id} row={r} now={now} onSelect={onSelect} onResume={onResume} />
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
