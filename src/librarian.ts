/**
 * The librarian, from the frontend's side: the store it has written, whether
 * a run is going, and the two ways to start one — by hand, or on its own a
 * little after sessions go quiet.
 *
 * Which sessions to look at is decided here, not in Rust: the panel already
 * holds the list, and "quiet for a few minutes" is a judgement about the
 * user's activity that the backend has no view of.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  EMPTY_LIB, LibEngine, LibRunReport, LibStore, Session, librarianForget, librarianRenameThread,
  librarianRun, librarianState,
} from "./ipc";
import { LibrarianSettings } from "./settings";

/** How long a session must have been quiet before the auto run reads it —
 *  a live conversation would be re-read every few minutes otherwise. */
const QUIET_MS = 3 * 60_000;
/** How long after the session list last changed the auto run waits. */
const SETTLE_MS = 45_000;
/** Sessions per model call — one batch. A run is a loop of these, and the
 *  store is re-read after each, so threads appear as they are written rather
 *  than after the whole backlog; a stop lands between batches. */
const BATCH = 8;

/** Whether an entry still describes the session, or activity has moved on. */
export function isCurrent(store: LibStore, s: Session): boolean {
  const e = store.sessions[s.id];
  return !!e && s.last_active <= e.seen + 60_000;
}

export function useLibrarian(cfg: LibrarianSettings, sessions: Session[]) {
  const [store, setStore] = useState<LibStore>(EMPTY_LIB);
  const [running, setRunning] = useState(false);
  const [report, setReport] = useState<LibRunReport | null>(null);
  /** How far the current run has got: sessions written so far, of those it
   *  set out to read. */
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
    const total: LibRunReport = { done: 0, remaining: list.length, cost: 0, errors: [] };
    setReport(null);
    try {
      // Batch by batch, so what is read shows up while the rest is being read.
      // The backend picks the next unread ones each time; a batch that reads
      // nothing means it is stuck, and the loop stops rather than spins.
      for (;;) {
        if (stopRef.current) break;
        const r = await librarianRun(engine, list, BATCH);
        total.done += r.done;
        total.cost += r.cost;
        total.errors.push(...r.errors);
        total.remaining = r.remaining;
        setProgress({ done: total.done, total: list.length });
        await reload();
        if (r.remaining === 0 || r.done === 0) break;
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

  // The auto run: once the list has been still for a while, catalogue what
  // has been quiet for a while. Re-armed by every change to the list, so a
  // busy afternoon is read in one go when it ends rather than piecemeal.
  const runRef = useRef(run);
  runRef.current = run;
  const pendingRef = useRef(pending);
  pendingRef.current = pending;
  useEffect(() => {
    if (!ready || !cfg.auto) return;
    const t = window.setTimeout(() => {
      const quiet = pendingRef.current.filter((s) => Date.now() - s.last_active > QUIET_MS);
      if (quiet.length) void runRef.current(quiet);
    }, SETTLE_MS);
    return () => clearTimeout(t);
  }, [ready, cfg.auto, sessions]);

  // While a run is going, re-read the store on a clock too: a batch lands
  // every couple of minutes, and a run that overlapped from before a reload
  // writes on its own schedule.
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => { void reload(); }, 15_000);
    return () => clearInterval(t);
  }, [running, reload]);

  const forget = useCallback(async () => {
    await librarianForget();
    setReport(null);
    await reload();
  }, [reload]);

  const renameThread = useCallback(async (id: string, name: string) => {
    await librarianRenameThread(id, name);
    await reload();
  }, [reload]);

  return { store, running, report, progress, pending, ready, run, stop, forget, renameThread, reload };
}

export type LibrarianCtl = ReturnType<typeof useLibrarian>;
