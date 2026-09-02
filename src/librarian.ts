/**
 * The librarian, from the frontend's side: what it has named, whether a run
 * is going, and the two ways to start one — by hand, or on its own a little
 * after sessions go quiet.
 *
 * Which sessions to look at is decided here, not in Rust: the panel already
 * holds the list, and "quiet for a few minutes" is a judgement about the
 * user's activity that the backend has no view of. The names themselves
 * reach the list through the backend, the same way a name set by hand does.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  EMPTY_LIB, LibEngine, LibRunReport, LibStore, Session, librarianForget, librarianRun, librarianState,
} from "./ipc";
import { LibrarianSettings } from "./settings";

/** How long a session must have been quiet before the auto run reads it —
 *  a live conversation would be re-read every few minutes otherwise. */
const QUIET_MS = 3 * 60_000;
/** How often the auto run looks for quiet sessions. */
const SETTLE_MS = 45_000;
/** Sessions per backend call. A run is a loop of these, and the list is
 *  re-read after each, so names appear as they are written rather than
 *  after the whole backlog; a stop lands between calls. */
const STEP = 3;

/** Whether an entry still describes the session, or activity has moved on. */
export function isCurrent(store: LibStore, s: Session): boolean {
  const e = store.sessions[s.id];
  return !!e && s.last_active <= e.seen + 60_000;
}

export function useLibrarian(cfg: LibrarianSettings, sessions: Session[]) {
  const [store, setStore] = useState<LibStore>(EMPTY_LIB);
  const [running, setRunning] = useState(false);
  const [report, setReport] = useState<LibRunReport | null>(null);
  /** How far the current run has got: sessions decided so far, of those it
   *  set out to look at. */
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const runningRef = useRef(false);
  const stopRef = useRef(false);

  const reload = useCallback(() => librarianState().then(setStore).catch(() => {}), []);
  useEffect(() => { void reload(); }, [reload]);

  const engine: LibEngine = cfg.engine === "api"
    ? { kind: "api", providerId: cfg.providerId, model: cfg.model.trim() }
    : { kind: "cli", agent: cfg.engine, model: cfg.model.trim() || null };
  const ready = cfg.enabled && (cfg.engine !== "api" || (cfg.providerId !== "" && cfg.model.trim() !== ""));
  const pending = useMemo(() => sessions.filter((s) => !isCurrent(store, s)), [sessions, store]);

  const run = useCallback(async (only?: Session[]) => {
    if (runningRef.current || !ready) return;
    const list = (only ?? pending).map((s) => ({ id: s.id, lastActive: s.last_active }));
    if (list.length === 0) return;
    runningRef.current = true;
    stopRef.current = false;
    setRunning(true);
    setProgress({ done: 0, total: list.length });
    const total: LibRunReport = { done: 0, skipped: 0, remaining: list.length, cost: 0, errors: [] };
    setReport(null);
    try {
      // A few at a time, so what is named shows up while the rest is being
      // read. The backend picks the next unread ones each call; a call that
      // decides nothing means it is stuck, and the loop stops rather than
      // spins.
      for (;;) {
        if (stopRef.current) break;
        const r = await librarianRun(engine, list, STEP);
        total.done += r.done;
        total.skipped += r.skipped;
        total.cost += r.cost;
        total.errors.push(...r.errors);
        total.remaining = r.remaining;
        setProgress({ done: total.done + total.skipped, total: list.length });
        await reload();
        if (r.remaining === 0 || r.done + r.skipped === 0) break;
      }
      setReport(total);
    } catch (e) {
      setReport({ ...total, errors: [...total.errors, String(e)] });
    } finally {
      runningRef.current = false;
      setRunning(false);
      setProgress(null);
      await reload();
    }
  }, [ready, pending, engine, reload]);
  const stop = useCallback(() => { stopRef.current = true; }, []);

  // The auto run: on a clock, name whatever has been quiet for a while. A
  // clock rather than a timer re-armed by list changes — the list changes
  // every time any live session writes a line, so a timer that waited for
  // it to hold still never fired while anything was running.
  const runRef = useRef(run);
  runRef.current = run;
  const pendingRef = useRef(pending);
  pendingRef.current = pending;
  useEffect(() => {
    if (!ready || !cfg.auto) return;
    const tick = () => {
      if (runningRef.current) return;
      const quiet = pendingRef.current.filter((s) => Date.now() - s.last_active > QUIET_MS);
      if (quiet.length) void runRef.current(quiet);
    };
    const t = window.setInterval(tick, SETTLE_MS);
    const first = window.setTimeout(tick, 5_000);
    return () => { clearInterval(t); clearTimeout(first); };
  }, [ready, cfg.auto]);

  const forget = useCallback(async () => {
    await librarianForget();
    setReport(null);
    await reload();
  }, [reload]);

  /** Sessions with a name from the librarian, as against ones it looked at
   *  and left to their engine. */
  const named = useMemo(() => Object.values(store.sessions).filter((e) => e.name).length, [store]);

  return { store, named, running, report, progress, pending, ready, run, stop, forget, reload };
}

export type LibrarianCtl = ReturnType<typeof useLibrarian>;
