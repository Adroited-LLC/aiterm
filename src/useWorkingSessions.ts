import { useEffect, useState } from "react";
import { spineOverview } from "./ipc";

/** Read the backend's existing phase state, without reading transcripts or
 * starting tails. Only phase membership changes rerender the sidebar. */
export function useWorkingSessions(enabled: boolean): Set<string> {
  const [working, setWorking] = useState<Set<string>>(new Set());
  useEffect(() => {
    if (!enabled) {
      setWorking(previous => previous.size ? new Set() : previous);
      return;
    }
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const read = async () => {
      try {
        const rows = await spineOverview();
        if (stopped) return;
        const next = new Set(rows.filter(row => row.phase === "working").map(row => row.session_id));
        setWorking(previous => previous.size === next.size && [...next].every(id => previous.has(id))
          ? previous : next);
      } catch { /* retain the last confirmed state during a transient failure */ }
      if (!stopped) timer = setTimeout(read, 2000);
    };
    void read();
    return () => { stopped = true; if (timer) clearTimeout(timer); };
  }, [enabled]);
  return working;
}
