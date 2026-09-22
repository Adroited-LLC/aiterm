/**
 * The spine's snapshot, kept fresh while the home screen is on screen.
 *
 * There is no renderer-side spine socket — the WebSocket the phone reads is
 * served over HTTP to devices, not plumbed back into this window — so this
 * polls. That is fine here and would not be anywhere else: `spine_overview`
 * reads memory the registry already keeps, touches no file and starts no
 * tail, so a 2 s tick costs a `Vec` clone and an IPC hop.
 *
 * Three triggers, deliberately: mount (so the board is populated in the first
 * frame after paint), `sessions://changed` (a new session, a rename — the same
 * event the sidebar reloads on), and the tick (a turn opening or a tool card
 * moving, neither of which raises an event this window can hear).
 *
 * The hook stops polling when `active` goes false and keeps the last answer,
 * so switching to a tab and back does not blank the board for a tick.
 */
import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { spineOverview, type SpineOverview } from "./ipc";

const EVERY_MS = 2000;

function sameOverview(previous: SpineOverview[], next: SpineOverview[]): boolean {
  return previous.length === next.length && next.every((row, index) => {
    const old = previous[index];
    return old.session_id === row.session_id && old.agent === row.agent
      && old.phase === row.phase && old.detail === row.detail
      && old.turn_open === row.turn_open && old.turn_started_ts === row.turn_started_ts
      && old.last_text === row.last_text
      && old.last_tool?.title === row.last_tool?.title
      && old.last_tool?.status === row.last_tool?.status;
  });
}

export function useSpineOverview(active: boolean): Map<string, SpineOverview> {
  const [rows, setRows] = useState<SpineOverview[]>([]);

  useEffect(() => {
    if (!active) return;
    let stopped = false;
    const read = () => {
      spineOverview()
        .then((r) => {
          if (!stopped) setRows(previous => sameOverview(previous, r) ? previous : r);
        })
        // A failed read leaves the last answer standing: the board falls back
        // to the sessions list per row anyway, and blanking it on one missed
        // IPC would flicker the whole page.
        .catch(() => {});
    };
    read();
    const timer = window.setInterval(read, EVERY_MS);
    const un = listen("sessions://changed", read);
    return () => {
      stopped = true;
      window.clearInterval(timer);
      un.then((f) => f()).catch(() => {});
    };
  }, [active]);

  return useMemo(() => new Map(rows.map((r) => [r.session_id, r])), [rows]);
}
