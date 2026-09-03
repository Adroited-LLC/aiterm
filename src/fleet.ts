/**
 * How the home screen decides what the fleet is doing.
 *
 * Three groups, in the order a person arriving cares about them:
 *
 * 1. **Needs you** — something is blocked on a human. Nothing else on the
 *    screen can be more urgent than this, so it goes first even when it is
 *    empty of everything but one row.
 * 2. **Running** — a turn is open somewhere and an agent is working. Worth
 *    seeing, not worth acting on.
 * 3. **Recent** — everything else, grouped under its project, because "which
 *    of my repos was I in" is the question a flat list of twelve titles
 *    cannot answer.
 *
 * The verdict per session comes from the spine when the spine has a log for
 * it — the same phase the phone sees, so the two can never disagree — and
 * falls back to what the desktop already knows about its own tabs otherwise.
 * The fallback matters for sessions with no tab bound, which have no tail and
 * are therefore idle anyway; it exists so the board is never empty on a cold
 * start, before the first poll has answered.
 *
 * Pure, and separate from the component, so the rules can be tested without a
 * renderer — see `fleet.test.ts`.
 */
import type { Session, SpineOverview, SpinePhase } from "./ipc";

export interface FleetRow {
  session: Session;
  phase: SpinePhase;
  /** One line about what it is doing or waiting on. "" when nothing is known. */
  detail: string;
  /** Turn start in ms, for the elapsed timer. Null when no turn is open. */
  since: number | null;
  /** The spine is tailing this one: the phase is live, not inferred. */
  tailed: boolean;
  /** A terminal is open on it right now. */
  live: boolean;
}

export interface FleetGroup {
  /** The project path these rows share. */
  path: string;
  rows: FleetRow[];
}

export interface Fleet {
  needsYou: FleetRow[];
  running: FleetRow[];
  /** Idle sessions, newest first, grouped by project and capped. */
  recent: FleetGroup[];
  /** How many idle sessions there are in total — the "show all" number. */
  idleTotal: number;
  /** How many of them the groups above actually contain. */
  idleShown: number;
}

export interface FleetInputs {
  sessions: Session[];
  /** By session id, from `spine_overview`. */
  overview: Map<string, SpineOverview>;
  /** Sessions with a terminal open. */
  live: Set<string>;
  /** Sessions whose tab rang the bell — the fallback for `needs_you`. */
  attention: Set<string>;
  /** Sessions whose tab is reporting progress — the fallback for `working`. */
  busy: Set<string>;
  /** How many idle rows to draw before offering "show all". */
  cap?: number;
}

/**
 * What a running row says under its title: the tool it is in the middle of if
 * there is one, else the last thing it said, else the phase's own words.
 *
 * A finished tool call is deliberately not shown — "Read foo.rs (completed)"
 * is a row describing the past, and the point of the running group is the
 * present. When the last card is done, the prose after it is more current.
 */
export function runningDetail(ov: SpineOverview): string {
  const tool = ov.last_tool;
  if (tool && (tool.status === "running" || tool.status === "pending")) return tool.title;
  return ov.last_text || ov.detail || "";
}

/** The phase for one session, spine first, tabs second. */
function verdict(
  s: Session,
  ov: SpineOverview | undefined,
  attention: Set<string>,
  busy: Set<string>,
): { phase: SpinePhase; detail: string; since: number | null } {
  if (ov) {
    const detail = ov.phase === "working" ? runningDetail(ov) : ov.detail || ov.last_text || "";
    return { phase: ov.phase, detail, since: ov.turn_open ? ov.turn_started_ts : null };
  }
  if (attention.has(s.id)) return { phase: "needs_you", detail: "", since: null };
  if (busy.has(s.id)) return { phase: "working", detail: "", since: null };
  return { phase: "idle", detail: "", since: null };
}

/**
 * Build the board.
 *
 * Every group is ordered most-recently-active first. That is one rule rather
 * than three, and it is the one that keeps rows still: sorting the running
 * group by elapsed time would have rows swapping places under the pointer
 * every time a turn started somewhere.
 */
export function buildFleet({
  sessions, overview, live, attention, busy, cap = 12,
}: FleetInputs): Fleet {
  const rows: FleetRow[] = [...sessions]
    .sort((a, b) => b.last_active - a.last_active)
    .map((s) => {
      const ov = overview.get(s.id);
      const { phase, detail, since } = verdict(s, ov, attention, busy);
      return { session: s, phase, detail, since, tailed: !!ov, live: live.has(s.id) };
    });

  const needsYou = rows.filter((r) => r.phase === "needs_you");
  const running = rows.filter((r) => r.phase === "working");
  const idle = rows.filter((r) => r.phase === "idle");

  // Grouped by the path the sidebar groups by, so a worktree's sessions sit
  // under their repo here too rather than under a directory named for a
  // branch. Group order follows the newest session in each — the list is
  // already sorted, so first appearance is that.
  const byPath = new Map<string, FleetRow[]>();
  for (const r of idle.slice(0, cap)) {
    const path = r.session.group_path || r.session.project_path;
    const at = byPath.get(path);
    if (at) at.push(r);
    else byPath.set(path, [r]);
  }
  const recent: FleetGroup[] = [...byPath].map(([path, rs]) => ({ path, rows: rs }));

  return {
    needsYou,
    running,
    recent,
    idleTotal: idle.length,
    idleShown: Math.min(idle.length, cap),
  };
}

/** "4m 12s" / "1h 03m" since `since`, or "" when there is nothing to count. */
export function elapsed(since: number | null, now: number): string {
  if (!since) return "";
  const secs = Math.max(0, Math.round((now - since) / 1000));
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ${String(secs % 60).padStart(2, "0")}s`;
  const hrs = Math.floor(mins / 60);
  return `${hrs}h ${String(mins % 60).padStart(2, "0")}m`;
}
