import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import SessionsPanel, { SessionDisplayOpts } from "./components/SessionsPanel";
import { StartChoice } from "./components/NewSessionMenu";
import StartControls, { useStartChoice } from "./components/StartControls";
import TerminalView, { TermHandle, TermTab } from "./components/TerminalView";
import FileExplorer from "./components/FileExplorer";
import GitPanel from "./components/GitPanel";
import Composer from "./components/Composer";
import TuiModelConfirm from "./components/TuiModelConfirm";
import TuiModelPicker from "./components/TuiModelPicker";
import TuiPermission from "./components/TuiPermission";
import TuiRewind from "./components/TuiRewind";
import {
  Detected, PermissionMode, detect, detectAgentsView, detectPermissionMode,
} from "./term/screen";
import { cycleModeTo } from "./term/drive";
import AgentPanel from "./components/AgentPanel";
import SettingsModal from "./components/SettingsModal";
import SessionPreview from "./components/SessionPreview";
import { UsagePanel, UsageSourceAt, mergeUsage } from "./components/UsagePanel";
import { Clock } from "./components/Clock";
import {
  AppSettings, applySettings, loadSettings, saveSettings, termFontFamily, termTheme,
} from "./settings";
import {
  ProjectInfo, Session,
  TrashedSession,
  listProjects, listSessions, materializeFork,
  reindexSessions, sessionFork, uiLog, usageReport,
  resolveResumableId, liveSessionIds, stopSession, unstoppableSessionIds, sessionMigratedTo,
  sessionDelete, trashDelete, trashEmpty, trashList, trashRestore,
  watchProject,
  agentLaunchCommand,
} from "./ipc";
import "./App.css";

const OPTS_KEY = "aiterm.sessionOpts";
const SIZES_KEY = "aiterm.panelSizes";
const FONT_KEY = "aiterm.fontScale";
const PANELS_KEY = "aiterm.panelToggles";
const USAGE_KEY = "aiterm.usageReport";
// The old cache held Anthropic bars only, under a different shape. Drop it
// rather than teach the loader to read a format nothing writes any more.
localStorage.removeItem("aiterm.usageCache");

/**
 * How aiterm starts claude. One place, so every session it opens behaves the
 * same way and nothing depends on a global config that can outrank a session.
 *
 * `--permission-mode auto` asks for the classifier mode, which the CLI calls
 * its own default ("Auto mode is now Claude Code's default permission mode").
 * It needs a background setup, and where that has not happened it falls back
 * to manual — a safe direction to fail, and the pill goes on reporting
 * whatever claude's status line actually says, so it cannot misrepresent
 * which mode you are in.
 *
 * `--allow-dangerously-skip-permissions` *enables* bypass without selecting
 * it, which is what puts the fourth mode in the shift+tab cycle and therefore
 * one click away on the permissions pill. Verified against a live session:
 * without the flag the cycle is manual → accept edits → plan → manual; with
 * it, bypass joins the loop.
 */
const CLAUDE_CMD = "claude --permission-mode auto --allow-dangerously-skip-permissions";

interface PanelToggles {
  sessions: boolean;
  explorer: boolean;
  git: boolean;
  composer: boolean;
  agent: boolean;
}
// Composer starts closed: it is opt-in chrome, not something to force on a
// first run. Everything else matches how the app has always opened.
const DEFAULT_PANELS: PanelToggles = {
  sessions: true, explorer: true, git: true, composer: false, agent: true,
};

// Sessions used to be hidden when a heuristic decided a newly-appeared one
// "superseded" them. That's gone — the list shows what is on disk, always —
// but the hidden set was persisted, so drop it once or those rows stay
// invisible forever in an app that no longer has a way to bring them back.
localStorage.removeItem("aiterm.superseded");

interface PanelSizes {
  left: number;
  right: number;
  explorerFrac: number;
  /** Height fraction of the right column taken by the agent (tasks) panel. */
  agentFrac: number;
}

/** Why a terminal's process is gone. Both halves are needed: portable-pty
 *  reports `code: 1` for *every* signal death, so the code alone cannot tell a
 *  SIGKILL from a plain `exit 1`. */
interface EndedWhy {
  code: number | null;
  signal: string | null;
}

/** Say how a process ended without inventing a reason it didn't have.
 *
 *  The signal comes first when there is one, because it is the true cause and
 *  the accompanying code is a placeholder — a SIGKILLed shell reporting
 *  "exited with status 1" reads as a program that failed, and sends you
 *  looking for a bug instead of for whoever killed it. */
function describeEnd(why: EndedWhy | undefined): string {
  if (!why) return "";
  if (why.signal) return `The process was stopped (${why.signal}).`;
  if (why.code === null) return "The process is gone and its exit status could not be read.";
  return `The process exited with status ${why.code}.`;
}

function loadJSON<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? { ...fallback, ...JSON.parse(raw) } : fallback;
  } catch {
    return fallback;
  }
}

/** The usage reading from the last run, so a cold start has something to show.
 *
 *  Not `loadJSON`: this one is an array, and that helper's object spread would
 *  hand back `{0: …, 1: …}`. Everything it loads comes back stale by
 *  definition — the numbers were true when they were written, and the panel
 *  says how long ago that was until a live read replaces them. */
function loadUsageCache(): UsageSourceAt[] {
  try {
    const raw = localStorage.getItem(USAGE_KEY);
    const v = raw ? JSON.parse(raw) : null;
    if (!Array.isArray(v)) return [];
    return v.map((s: UsageSourceAt) => ({ ...s, stale: true }));
  } catch {
    return [];
  }
}

export default function App() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeProject, setActiveProject] = useState<string | null>(null);
  // Transient bottom toast (e.g. a resume with nothing resumable left).
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 6000);
    return () => clearTimeout(t);
  }, [notice]);
  const [tabs, setTabs] = useState<TermTab[]>([]);
  const [activeTab, setActiveTab] = useState<number | null>(null);
  const [previewSession, setPreviewSession] = useState<Session | null>(null);
  const nextKey = useRef(1);
  const [gitRefresh, setGitRefresh] = useState(0);
  const [explorerRefresh, setExplorerRefresh] = useState(0);

  // Tabs whose terminal rang the bell while not being looked at.
  const [attention, setAttention] = useState<Set<number>>(new Set());
  // Tabs whose process died on its own, and the exit code it died with. These
  // keep their row: the tab going away used to be the *only* sign anything had
  // happened, which is no help at all when the thing that killed it was
  // somewhere else entirely — `claude agents` in another terminal, or a phone.
  const [ended, setEnded] = useState<Map<number, EndedWhy>>(new Map());
  const activeTabRef = useRef<number | null>(null);
  // Latest tabs, read without putting `tabs` in an effect's deps (which would
  // re-run it on every tab change).
  const tabsRef = useRef<TermTab[]>(tabs);
  tabsRef.current = tabs;

  const handles = useRef<Map<number, TermHandle>>(new Map());
  const lastOutput = useRef<Map<number, number>>(new Map());

  // Which panels are open. These were plain state, so every restart threw the
  // layout away and you rebuilt it by hand — the sizes and fonts beside them
  // had always persisted, which made the loss look arbitrary. Saved as one
  // object rather than five keys: it is one decision, "how I have it set up".
  const [panels, setPanels] = useState<PanelToggles>(() =>
    loadJSON(PANELS_KEY, DEFAULT_PANELS),
  );
  useEffect(() => localStorage.setItem(PANELS_KEY, JSON.stringify(panels)), [panels]);
  const { sessions: showSessions, explorer: showExplorer, git: showGit,
          composer: showComposer, agent: showAgent } = panels;
  // Same shape the old `useState` setters had, so every call site is unchanged.
  const setPanel = (k: keyof PanelToggles) => (v: boolean) =>
    setPanels((p) => ({ ...p, [k]: v }));
  const setShowSessions = setPanel("sessions");
  const setShowExplorer = setPanel("explorer");
  const setShowGit = setPanel("git");
  const setShowComposer = setPanel("composer");
  const setShowAgent = setPanel("agent");

  // Screens in the terminal that aiterm can present better than the TUI does.
  // Polled rather than pushed: xterm has no "screen changed" event worth
  // hanging this on, and reading ~40 already-parsed lines four times a second
  // is nothing. `dismissed` remembers that you asked for the raw terminal for
  // *this* appearance, and clears itself once the screen goes away.
  const [tui, setTui] = useState<Detected | null>(null);
  const [permMode, setPermMode] = useState<PermissionMode | null>(null);
  // claude's agents view is on the active terminal's screen. Entering it moves
  // the conversation to the daemon under a new id, so this both hurries the
  // re-key below and stands the pills down — a /model sent now would be typed
  // into the view's "describe a task" box rather than run as a command.
  const [agentsView, setAgentsView] = useState(false);
  /** Last effect run's view state — detects the just-closed edge. */
  const wasAgentsView = useRef(false);
  const [tuiDismissed, setTuiDismissed] = useState(false);
  // Only dress up a screen *we* opened. Typing /model or /rewind yourself is a
  // request for the terminal, and answering it with our own dialog would be
  // taking the terminal away from someone who just asked for it. Records what
  // was asked for and when, so a screen that never appears stops arming us.
  const armed = useRef<{ what: "model" | "rewind"; at: number } | null>(null);
  const openViaPill = useCallback((what: "model" | "rewind", command: string) => {
    if (activeTab === null) return;
    armed.current = { what, at: Date.now() };
    handles.current.get(activeTab)?.sendComposed(command);
  }, [activeTab]);
  const openModelPicker = useCallback(
    () => openViaPill("model", "/model"), [openViaPill]);
  const openRewind = useCallback(
    () => openViaPill("rewind", "/rewind"), [openViaPill]);

  // Closing a dialog — by answering it, cancelling, or asking for the raw
  // terminal — always ends with the keyboard back in the terminal. Whatever
  // happens next is typed there, and leaving focus on a button that just
  // disappeared makes the first keystroke go nowhere.
  const dismissTui = useCallback((tab: number) => {
    setTuiDismissed(true);
    handles.current.get(tab)?.focus();
  }, []);

  const setPermissionMode = useCallback(async (target: PermissionMode) => {
    if (activeTab === null) return;
    const handle = handles.current.get(activeTab);
    if (!handle) return;
    await cycleModeTo(
      () => detectPermissionMode(handle.screen()),
      (d) => handle.write(d),
      target,
    );
    handle.focus();
  }, [activeTab]);

  useEffect(() => {
    const id = window.setInterval(() => {
      const handle = activeTab === null ? undefined : handles.current.get(activeTab);
      const lines = handle ? handle.screen() : null;
      setPermMode(lines ? detectPermissionMode(lines) : null);
      setAgentsView(lines ? detectAgentsView(lines) : false);
      const found = lines ? detect(lines) : null;
      if (!found) {
        // Gone, or not painted yet. Give it a moment before disarming, so
        // arming a beat before claude draws does not cancel itself.
        if (armed.current && Date.now() - armed.current.at > 4000) {
          armed.current = null;
        }
        setTui(null);
        setTuiDismissed(false);
        return;
      }
      // A permission prompt is never something we asked for — it interrupts
      // you, which is exactly when a real dialog earns its place. Everything
      // reached by a command has to be armed, because typing that command
      // yourself is a request for the terminal.
      const needs =
        found.kind === "model-picker" || found.kind === "model-confirm" ? "model"
        : found.kind === "rewind-picker" || found.kind === "rewind-confirm" ? "rewind"
        : null;
      if (needs && armed.current?.what !== needs) return;
      // Showing an armed screen keeps it armed. Both flows have a second step
      // that replaces the first on screen, and the repaint between them can
      // land a tick on nothing — which, more than 4s after the pill click,
      // disarmed us and left step two undressed in the terminal.
      if (needs && armed.current) armed.current.at = Date.now();
      setTui((prev) => {
        // Only replace when something actually changed, so the dialog is not
        // rebuilt four times a second while it sits there.
        if (prev && prev.kind === found.kind && prev.highlighted === found.highlighted) {
          const sameSize =
            "options" in prev && "options" in found
              ? prev.options.length === found.options.length
              : "points" in prev && "points" in found
                ? prev.points.length === found.points.length
                : true;
          if (sameSize) return prev;
        }
        return found;
      });
    }, 250);
    return () => window.clearInterval(id);
  }, [activeTab]);

  const [opts, setOpts] = useState<SessionDisplayOpts>(() =>
    loadJSON(OPTS_KEY, { showPath: true, showBranch: true, showTime: true }),
  );
  const [sizes, setSizes] = useState<PanelSizes>(() =>
    loadJSON(SIZES_KEY, { left: 280, right: 560, explorerFrac: 0.5, agentFrac: 0.3 }),
  );

  const [fontScale, setFontScale] = useState<number>(
    () => loadJSON(FONT_KEY, { scale: 1 }).scale,
  );

  const [settings, setSettings] = useState<AppSettings>(loadSettings);
  const [showSettingsModal, setShowSettingsModal] = useState(false);
  useEffect(() => {
    applySettings(settings);
    saveSettings(settings);
  }, [settings]);
  useEffect(() => {
    if (!showSettingsModal) return;
    const h = (e: KeyboardEvent) => e.key === "Escape" && setShowSettingsModal(false);
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [showSettingsModal]);

  useEffect(() => localStorage.setItem(OPTS_KEY, JSON.stringify(opts)), [opts]);
  useEffect(() => localStorage.setItem(SIZES_KEY, JSON.stringify(sizes)), [sizes]);
  useEffect(() => localStorage.setItem(FONT_KEY, JSON.stringify({ scale: fontScale })), [fontScale]);

  const bumpFont = useCallback((dir: 1 | -1 | 0) => {
    setFontScale((s) =>
      dir === 0 ? 1 : Math.max(0.7, Math.min(1.6, +(s + dir * 0.1).toFixed(2))),
    );
  }, []);

  // Ctrl+= / Ctrl+- / Ctrl+0 font zoom, captured before xterm sees the keys.
  // Ctrl+Shift+L: force a clean repaint of the active terminal.
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.altKey || e.metaKey) return;
      if (e.shiftKey && (e.key === "L" || e.key === "l")) {
        e.preventDefault();
        const key = activeTabRef.current;
        if (key !== null) handles.current.get(key)?.redraw();
        return;
      }
      if (e.shiftKey) return;
      if (e.key === "=" || e.key === "+") { e.preventDefault(); bumpFont(1); }
      else if (e.key === "-") { e.preventDefault(); bumpFont(-1); }
      else if (e.key === "0") { e.preventDefault(); bumpFont(0); }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [bumpFont]);

  const termFont = Math.round(settings.termFontSize * fontScale);
  const xtermTheme = useMemo(() => termTheme(settings), [settings]);
  const xtermFont = useMemo(() => termFontFamily(settings), [settings]);
  const zoomFor = (panel: keyof AppSettings["panelScale"]): React.CSSProperties =>
    ({ zoom: fontScale * settings.panelScale[panel] } as React.CSSProperties);

  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [trashed, setTrashed] = useState<TrashedSession[]>([]);
  // Which sessions the daemon is actually running. Polled rather than derived
  // from tabs, because those are different questions — see SessionsPanel's
  // hasTab/isRunning split.
  //
  // Every tick spawns a whole `claude agents --json`: ~0.26s wall and a peak
  // around 300 MB RSS, measured 2026-07-27. At the old unconditional 6s that
  // was 600 process spawns an hour for the lifetime of the window, running
  // just as hard while the app sat in the background as while it was being
  // looked at.
  //
  // So it only runs while the window has focus, and it stops dead when focus
  // leaves. Nothing is lost by that: the one thing that must reach you while
  // you are elsewhere — a session wanting input — arrives as a terminal bell
  // and an OS attention request, neither of which comes from here. This only
  // feeds dots on rows nobody is looking at.
  //
  // Regaining focus ticks immediately rather than waiting out the interval,
  // which is what makes the pause invisible: by the time your eyes are back on
  // the sidebar the reading is already in flight.
  const [liveSessions, setLiveSessions] = useState<Set<string>>(new Set());
  useEffect(() => {
    let stop = false;
    let timer: number | undefined;
    const tick = () =>
      liveSessionIds()
        .then((ids) => { if (!stop) setLiveSessions(new Set(ids)); })
        .catch(() => { /* keep the last known set rather than blanking dots */ });
    const start = () => {
      if (stop || timer !== undefined) return;
      tick();
      timer = window.setInterval(tick, 15000);
    };
    const halt = () => {
      if (timer === undefined) return;
      window.clearInterval(timer);
      timer = undefined;
    };
    if (document.hasFocus()) start();
    window.addEventListener("focus", start);
    window.addEventListener("blur", halt);
    return () => {
      stop = true;
      halt();
      window.removeEventListener("focus", start);
      window.removeEventListener("blur", halt);
    };
  }, []);

  // Usage across every service, fetched once for everything that shows it.
  // `/api/oauth/usage` rate limits, so a second poller is not just waste — a
  // refused request comes back with no numbers, and that view would blank
  // while the other still showed bars. `mergeUsage` keeps the last good
  // reading per source, because "couldn't ask" must never render as "you have
  // used nothing".
  //
  // Seeded from the last reading written to disk, so a cold start shows
  // something immediately instead of an empty strip for up to a minute — the
  // first call often lands on a rate limit, which made the gap longer still.
  // Written on every success rather than on exit: an exit hook does not run
  // when the app is killed, which is exactly when the cache would be wanted.
  // What comes off disk is marked stale on the way in: it was true when it was
  // written, and the panel says how long ago that was until a live read lands.
  const [usageSources, setUsageSources] = useState<UsageSourceAt[]>(loadUsageCache);
  const [usageBusy, setUsageBusy] = useState(false);
  // The merge needs the reading currently on screen, and reading it out of
  // state inside the updater would mean writing localStorage from a render.
  const usageRef = useRef<UsageSourceAt[]>(usageSources);
  const readUsage = useCallback(() => {
    setUsageBusy(true);
    return usageReport()
      .then((next) => {
        const merged = mergeUsage(usageRef.current, next);
        usageRef.current = merged;
        setUsageSources(merged);
        try {
          localStorage.setItem(USAGE_KEY, JSON.stringify(merged));
        } catch { /* quota — the cache is a convenience, not a requirement */ }
      })
      .catch(() => {})
      .finally(() => setUsageBusy(false));
  }, []);
  useEffect(() => {
    readUsage();
    const iv = setInterval(readUsage, 60_000);
    return () => clearInterval(iv);
  }, [readUsage]);
  // The composer's usage pill shows plan limits for the session it is attached
  // to, and those sessions are claude — so it gets Anthropic's bars, not the
  // whole report.
  const claudeUsage = useMemo(
    () => usageSources.find((s) => s.id === "anthropic"),
    [usageSources],
  );

  // List-only refresh: cheap, safe to run on every fs event.
  const refreshSessionList = useCallback(() => {
    listSessions().then(setSessions).catch(console.error);
    listProjects().then(setProjects).catch(console.error);
    trashList().then(setTrashed).catch(() => setTrashed([]));
  }, []);
  const refreshSessions = useCallback(() => {
    refreshSessionList();
    // Keep the full-text index warm in the background (30s poll only).
    reindexSessions().catch(() => {});
  }, [refreshSessionList]);
  useEffect(() => {
    refreshSessions();
    const iv = setInterval(refreshSessions, 30_000);
    return () => clearInterval(iv);
  }, [refreshSessions]);
  // Event-driven refresh: Claude's transcripts changed (backend debounces).
  useEffect(() => {
    const un = listen("sessions://changed", () => {
      refreshSessionList();
    });
    return () => {
      un.then((f) => f());
    };
  }, [refreshSessionList]);

  // NOTE: there used to be a large effect here that watched for newly-appeared
  // sessions and decided some of them "superseded" older rows — hiding those
  // rows behind six heuristic guards, with an undo toast. It is gone. The list
  // shows what is on disk, and nothing removes a row but you.
  //
  // The tab-rebind it also did (follow a conversation into a new transcript
  // after a compaction) is not needed either: the backend resolves a pinned
  // session id to the newest transcript in its family on read, so the Agents
  // and Tasks panels already follow without anything rewriting the tab.

  const openTab = useCallback(
    (title: string, cwd: string | null, command: string | null, slotId: string,
     sessionId?: string, fresh?: boolean, env?: Record<string, string>) => {
      setPreviewSession(null);
      setTabs((t) => {
        const existing = t.find((x) => x.slotId === slotId);
        if (existing) {
          setActiveTab(existing.key);
          return t;
        }
        const key = nextKey.current++;
        setActiveTab(key);
        return [...t, { key, title, cwd, command, sessionId, slotId, fresh, env }];
      });
    },
    [],
  );

  const registerHandle = useCallback((key: number, handle: TermHandle | null) => {
    if (handle) handles.current.set(key, handle);
    else handles.current.delete(key);
  }, []);

  const noteActivity = useCallback((key: number) => {
    lastOutput.current.set(key, Date.now());
  }, []);

  const noteAttention = useCallback((key: number, on: boolean) => {
    // A bell on the tab you're actively looking at isn't news.
    if (on && key === activeTabRef.current && document.hasFocus()) return;
    setAttention((prev) => {
      if (on === prev.has(key)) return prev;
      const next = new Set(prev);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });
    if (on && !document.hasFocus()) {
      getCurrentWindow()
        .requestUserAttention(UserAttentionType.Informational)
        .catch(() => {});
    }
  }, []);

  // Viewing a tab (with the window focused) clears its badge.
  useEffect(() => {
    activeTabRef.current = activeTab;
    if (activeTab === null) return;
    const clear = () => {
      if (document.hasFocus() && activeTabRef.current !== null) {
        setAttention((prev) => {
          if (!prev.has(activeTabRef.current!)) return prev;
          const next = new Set(prev);
          next.delete(activeTabRef.current!);
          return next;
        });
      }
    };
    clear();
    window.addEventListener("focus", clear);
    return () => window.removeEventListener("focus", clear);
  }, [activeTab]);

  // (Removed the startup OS-window ±1px "jiggle". It forced a Wayland surface
  // reconfigure to fix bottom-edge clipping / stale content — a symptom of the
  // WebKitGTK DMABUF renderer, now disabled at the Rust entry point. No more
  // window growing/shrinking on launch.)

  // Dropping files onto the window pastes their quoted paths into the
  // active terminal (like any terminal emulator) instead of letting the
  // webview navigate to the file.
  const previewRef = useRef<Session | null>(null);
  useEffect(() => {
    previewRef.current = previewSession;
  }, [previewSession]);
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type !== "drop" || e.payload.paths.length === 0) return;
      const key = activeTabRef.current;
      if (key === null || previewRef.current) return;
      const h = handles.current.get(key);
      // One paste per path, like a real terminal drop — pasted (not typed)
      // so claude recognizes image/file paths and shows [Image #N].
      e.payload.paths.forEach((p, i) => {
        if (i > 0) h?.write(" ");
        h?.paste(shellEscape(p));
      });
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // Watch the active project: git changes refresh the repo panel, tree
  // changes refresh the explorer (git status also follows tree edits).
  useEffect(() => {
    if (!activeProject) return;
    watchProject(activeProject).catch(console.error);
  }, [activeProject]);
  useEffect(() => {
    const un = listen<{ git: boolean; tree: boolean }>("fs://changed", (e) => {
      setGitRefresh((n) => n + 1);
      if (e.payload.tree) setExplorerRefresh((n) => n + 1);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const activeTabObj = tabs.find((t) => t.key === activeTab) ?? null;
  const activeSessionId = activeTabObj?.sessionId ?? null;

  // Sessions aiterm has started that are not on disk yet. The sidebar lists
  // transcripts, and claude writes none until the first prompt, so without
  // these a brand-new session is a terminal with no row — unreachable the
  // moment you look at something else, since the sidebar is how you get back
  // to a tab. Each one retires by itself: the id was minted by `newSession`,
  // so when that transcript lands the real row is already the tab's row and
  // this list stops naming it. Nothing guesses, and nothing hides a real row.
  const knownSessionIds = useMemo(() => new Set(sessions.map((s) => s.id)), [sessions]);
  const pendingSessions = useMemo(
    () =>
      tabs
        .filter((t) => t.fresh && t.sessionId && !knownSessionIds.has(t.sessionId))
        .map((t) => ({ id: t.sessionId!, title: t.title, cwd: t.cwd ?? "" })),
    [tabs, knownSessionIds],
  );
  // The migration watcher's guard, reduced to a boolean before it reaches the
  // effect's dependency list. `knownSessionIds` is a new Set on every sessions
  // refresh — which the transcript watcher fires on every write — so depending
  // on it directly re-armed the watcher constantly during any active
  // conversation: a journal line and an immediate re-check per write instead of
  // one per 15s. The boolean only changes when the answer does, and its one
  // flip (the fresh tab's transcript landing) is exactly when re-arming is
  // wanted.
  const freshUnwritten = !!(
    activeTabObj?.fresh &&
    activeTabObj.sessionId &&
    !knownSessionIds.has(activeTabObj.sessionId)
  );

  // A conversation that moves to the daemon — which is all that opening the
  // agents view does — keeps running in this same pty under a NEW session id.
  // The tab's pinned id was set once when it opened and nothing ever moved it,
  // so from that moment the terminal renders the live child while every panel
  // keyed to the tab reads the parent: a file nothing is writing. Live text,
  // dead clock. Re-key the tab when it happens, and say so, because the row
  // the sidebar shows for this conversation changes underneath the user.
  //
  // Only the active tab, and only every 15s: answering this means reading
  // transcripts off disk, and the parent can be tens of megabytes.
  //
  // Except while the agents view is on screen — the migration happens the
  // moment that view opens, so seeing it is the cue to chase the new id now
  // (and every 2s until the child's transcript lands on disk). This is what
  // closes the stale window where the terminal showed the live conversation
  // over panels reading a file nothing was writing.
  useEffect(() => {
    const key = activeTabObj?.key;
    const pinned = activeTabObj?.sessionId;
    const title = activeTabObj?.title ?? "This session";
    if (key === undefined || !pinned) return;
    // A session aiterm started that has not written its transcript yet has
    // nothing to have migrated *from*, so this would poll a file that does not
    // exist until the first prompt lands. Narrow on purpose: only a `fresh`
    // tab whose id has never appeared on disk. A pinned id missing from the
    // list is otherwise the very symptom this watcher exists for — a
    // compaction retires the original transcript — and skipping those would
    // disable it exactly when it is needed.
    if (freshUnwritten) return;
    let stop = false;
    const check = async () => {
      try {
        const moved = await sessionMigratedTo(pinned);
        if (stop || !moved || moved === pinned) return;
        uiLog(`migrate: re-keying tab ${key} ${pinned} -> ${moved}`);
        setTabs((ts) => ts.map((x) => (x.key === key ? { ...x, sessionId: moved } : x)));
        setNotice(`"${title}" moved to a background session — its panels now follow the live one.`);
        refreshSessionList();
      } catch (e) {
        // Not just "backend unavailable": a rejected invoke lands here too,
        // and silently keeping the pinned id is how a dead path looks tested.
        uiLog(`migrate: check for ${pinned} failed: ${String(e)}`);
      }
    };
    uiLog(`migrate: watching ${pinned} (agentsView=${agentsView})`);
    check();
    const id = setInterval(check, agentsView ? 2000 : 15000);
    // Right after ← the child is a two-line stub with no history and no links
    // into the parent — the copy lands with the *next* activity, usually just
    // after the user escapes back and keeps talking (measured 2026-07-27). So
    // the moment the view closes, check again a couple of times instead of
    // leaving the last word to the slow poll.
    const grace: number[] = [];
    if (wasAgentsView.current && !agentsView) {
      grace.push(window.setTimeout(check, 3000), window.setTimeout(check, 8000));
    }
    wasAgentsView.current = agentsView;
    return () => {
      stop = true;
      clearInterval(id);
      grace.forEach(clearTimeout);
    };
  }, [activeTabObj?.key, activeTabObj?.sessionId, activeTabObj?.title, freshUnwritten,
      refreshSessionList, agentsView]);
  // The composer's status line is gone, and with it three pollers that existed
  // only to feed it: a 1s "working" pulse, a 5s `session_status` call, and a
  // `git_repo_state` call per project change. Claude's own footer already says
  // all three things. Removed rather than left running for nobody to read.

  const closeTab = useCallback((key: number) => {
    setEnded((m) => {
      if (!m.has(key)) return m;
      const next = new Map(m);
      next.delete(key);
      return next;
    });
    setTabs((t) => {
      const next = t.filter((x) => x.key !== key);
      setActiveTab((cur) => (cur === key ? (next[next.length - 1]?.key ?? null) : cur));
      return next;
    });
  }, []);

  /** A terminal's process ended. Whether that closes the tab depends entirely
   *  on why.
   *
   *  Exit 0 is someone leaving — `exit` at a shell, `/quit` in claude — and the
   *  tab should go, which is what it has always done. Any other status means
   *  the process died without being asked to, and the most likely cause is now
   *  something outside this window: a session that has moved to the daemon is
   *  listed by `claude agents` everywhere, so it can be killed from another
   *  terminal or from the phone. Closing the tab in that case throws away the
   *  notice that it happened and leaves the transcript — still on disk, still
   *  resumable — to be found by hand. So the tab stays and says so. */
  const handleTermExit = useCallback(
    (key: number, code: number | null, signal: string | null) => {
      if (code === 0) {
        closeTab(key);
        return;
      }
      setEnded((m) => new Map(m).set(key, { code, signal }));
    },
    [closeTab],
  );

  /** Put an ended tab back, resuming its conversation where it stopped. The
   *  slot is reused so the sidebar row it belongs to stays the same row. */
  const restartEnded = useCallback(
    (key: number) => {
      const t = tabsRef.current.find((x) => x.key === key);
      if (!t) return;
      closeTab(key);
      const cmd = t.sessionId ? `${CLAUDE_CMD} --resume ${t.sessionId}` : t.command;
      openTab(t.title, t.cwd, cmd, t.slotId, t.sessionId);
    },
    [closeTab, openTab],
  );

  /** Show the terminal a slot names. Used by the placeholder rows, which have
   *  no session on disk to select in the ordinary way. */
  const focusSlot = useCallback((slotId: string) => {
    const t = tabsRef.current.find((x) => x.slotId === slotId);
    if (!t) return;
    setPreviewSession(null);
    setActiveTab(t.key);
  }, []);
  const closeSlot = useCallback((slotId: string) => {
    const t = tabsRef.current.find((x) => x.slotId === slotId);
    if (t) closeTab(t.key);
  }, [closeTab]);

  const selectSession = (s: Session) => {
    setActiveProject(s.project_path);
    // Warp-style: the sidebar is the tab list — switch to this item's live
    // terminal if it has one (resume first, then a project shell). Without
    // one, show a read-only conversation preview so ▶ can be an informed
    // choice.
    const live =
      tabs.find((t) => t.slotId === s.id) ??
      tabs.find((t) => t.slotId === `shell:${s.project_path}`);
    if (live) {
      setPreviewSession(null);
      setActiveTab(live.key);
    } else {
      setPreviewSession(s);
    }
  };
  const resumeSession = async (s: Session) => {
    setActiveProject(s.project_path);
    // The pinned id can go stale: a `/clear` or compaction retires the
    // original transcript (Claude Code deletes it or renames it to
    // `<id>.orphaned-…`), so `claude --resume <original-id>` dies with "no
    // conversation found" — a black pane, the core "broken feel" of resume.
    // Resolve to the surviving continuation first; if nothing resumable is
    // left, say so instead of launching a doomed resume. (A forked parent is
    // NOT stale — forking leaves it intact, and it resolves to itself.)
    let liveId = s.id;
    try {
      let resolved = await resolveResumableId(s.id);
      // Nothing resumable — but a `/fork` row is a special case worth rescuing.
      // It has no conversation of its own, only a promise in job state to hold
      // the parent's history up to the fork. Redeem it, then ask again. Every
      // other kind of empty session fails this and falls through to the toast.
      if (resolved === null) {
        try {
          await materializeFork(s.id);
          resolved = await resolveResumableId(s.id);
          refreshSessionList();
        } catch {
          /* not a redeemable fork; the toast below is the right answer */
        }
      }
      if (resolved === null) {
        setNotice(`"${s.title}" was cleared or superseded — no resumable transcript remains.`);
        return;
      }
      liveId = resolved;
    } catch {
      liveId = s.id; // resolver unavailable → fall back to the pinned id
    }
    // Resume the way you would from a shell: if the conversation is still
    // running, close it, then `claude --resume <id>`. `--resume` refuses a live
    // session ("…add --fork-session to branch off a copy"), and the old answer
    // to that was to offer ⑂ instead — which made branching a copy the only
    // way back into your own conversation, and minted an immortal fork on
    // every attempt. Stopping first is what the user actually means by "open
    // this session".
    //
    // Our own tab goes through closeTab so React state stays in step; anything
    // else is signalled through the roster.
    //
    // But ask the roster *first*, because closing comes before stopping and
    // the two can disagree. A session the daemon holds has no pid to signal —
    // a conversation moves there on its own the moment you open the agents
    // view — so `stopSession` can only poll for five seconds and give up. It
    // used to give up having already closed the tab it was going to reuse,
    // which turned "resume this" into "lose this", and the resume never
    // happened either. Nothing is touched until we know a stop can succeed.
    try {
      const held = await unstoppableSessionIds();
      if (held.includes(liveId) || held.includes(s.id)) {
        setNotice(
          `"${s.title}" is running under the Claude Code daemon, so aiterm can't stop it. ` +
            `Stop it from \`claude agents\`, then resume.`,
        );
        return;
      }
    } catch {
      /* roster unavailable — fall through and let stopSession be the judge */
    }
    // Match on both ids: a tab opened before a compaction is slotted under the
    // row's pinned id, not the continuation `liveId` resolves to.
    for (const t of tabsRef.current) {
      if (t.slotId === liveId || t.slotId === s.id) closeTab(t.key);
    }
    try {
      await stopSession(liveId);
      if (liveId !== s.id) await stopSession(s.id);
    } catch (e) {
      setNotice(`Couldn't stop "${s.title}" to resume it: ${e}`);
      return;
    }
    // Resume the same way we start anything — see CLAUDE_CMD. This used to
    // pass `--permission-mode <configured>`, which is what silently lifted a
    // manual session into bypass on resume.
    const cmd = `${CLAUDE_CMD} --resume ${liveId}`;
    openTab(s.title, s.project_path, cmd, liveId, liveId);
  };
  // Branch a session into its own tab, resumable later on its own row. The
  // parent is left intact and frozen at its own context — `--fork-session`
  // writes a new transcript and never touches the original.
  // Branch a session. This starts nothing and touches no tab: the backend
  // copies the transcript under a fresh id, so the branch appears as an
  // ordinary inactive row holding the conversation exactly as it stands now,
  // and the session you forked from keeps running, still yours, still green.
  //
  // It used to launch `claude --fork-session --resume` into a new tab and
  // close the parent's. That made forking a lifecycle event — the branch
  // didn't exist on disk until you typed into it, and the tab you forked
  // *from* went away, which is not what branching means. Rejections are
  // surfaced by the caller (see `onFork` below).
  const forkSession = async (s: Session) => {
    const branchId = await sessionFork(s.id);
    refreshSessionList();
    setNotice(`Branched "${s.title}" — the copy is listed, idle, at this point.`);
    return branchId;
  };
  // Exit an active session: close its live terminal tab (ends the running
  // claude process). The transcript stays on disk, so it's resumable later.
  const exitSession = (s: Session) => {
    const live = tabs.find((t) => t.slotId === s.id);
    if (live) closeTab(live.key);
  };
  const newShell = (s: Session) => {
    setActiveProject(s.project_path);
    openTab(basename(s.project_path), s.project_path, null, `shell:${s.project_path}`);
  };
  const deleteSession = async (s: Session) => {
    try {
      await sessionDelete(s.id);
    } catch (e) {
      console.error("delete failed:", e);
    }
    setPreviewSession((p) => (p?.id === s.id ? null : p));
    refreshSessions();
  };
  const restoreTrashed = async (id: string) => {
    try {
      await trashRestore(id);
    } catch (e) {
      console.error("restore failed:", e);
    }
    refreshSessions();
  };
  const deleteTrashed = async (id: string) => {
    try {
      await trashDelete(id);
    } catch (e) {
      console.error("trash delete failed:", e);
    }
    refreshSessions();
  };
  const emptyTrash = async () => {
    try {
      await trashEmpty();
    } catch (e) {
      console.error("empty trash failed:", e);
    }
    refreshSessions();
  };
  const selectProject = (p: ProjectInfo) => {
    setActiveProject(p.path);
    const live =
      tabs.find((t) => t.slotId === `claude:${p.path}`) ??
      tabs.find((t) => t.slotId === `shell:${p.path}`);
    if (live) setActiveTab(live.key);
  };
  const projectShell = (p: ProjectInfo) => {
    setActiveProject(p.path);
    openTab(p.name, p.path, null, `shell:${p.path}`);
  };
  const projectClaude = (p: ProjectInfo) => {
    setActiveProject(p.path);
    openTab(p.name, p.path, CLAUDE_CMD, `claude:${p.path}`);
  };

  /**
   * Start a fresh claude session in `cwd`.
   *
   * The one thing aiterm could not do: every other door into a terminal needs
   * a row to open it from — resume a session, branch one, open a shell beside
   * one, or ▶ a project that has no sessions yet. A conversation that has
   * never existed has no row, so beginning one meant opening a shell and
   * typing `claude` by hand, and on a machine with no `~/Projects` there was
   * no project row either.
   *
   * **The id is minted here rather than discovered.** Letting claude choose it
   * looks simpler and is the reason the first version of this was unusable: a
   * session has no transcript until its first prompt, the sidebar lists what
   * is on disk, and the sidebar is the tab list — so a new session opened a
   * terminal that no row named. It took the pane over from whatever you were
   * looking at and there was no way back to it. `--session-id` composes with a
   * fresh start (unlike with `--resume`, where it is rejected without
   * `--fork-session`), so the tab can be slotted by its session id from the
   * first frame, exactly like a resume — which is also what lets the composer
   * pills and the agents panel key to it before it has said anything.
   *
   * *Verified 2026-07-27: `claude --session-id <uuid> --permission-mode auto
   * --allow-dangerously-skip-permissions -p …` wrote its transcript to
   * `<uuid>.jsonl` under the launch cwd.*
   *
   * `fresh` covers the gap until that file exists — see `pendingSessions`.
   */
  const newSession = useCallback(async (cwd: string, choice: StartChoice) => {
    setActiveProject(cwd);
    // Mint an id only where the agent will accept one. Codex has no
    // --session-id, so for it this is a tab handle: the placeholder row keeps
    // the tab reachable, but nothing is keyed to it as a session, because
    // there is no session by that name and never will be.
    const id = crypto.randomUUID();
    try {
      const plan = await agentLaunchCommand(choice.agentId, {
        model: choice.model,
        effort: choice.effort,
        sessionId: choice.mintsSessionId ? id : null,
      });
      openTab(
        basename(cwd), cwd, plan.command, id,
        choice.mintsSessionId ? id : undefined, true, plan.env,
      );
    } catch (e) {
      setNotice(`Couldn't start ${choice.agentId}: ${e}`);
    }
  }, [openTab]);

  /** The empty pane's own source/model/effort, so its button starts the same
   *  session the ＋ menu would. It used to take the first installed agent on
   *  its defaults — a different session from the one the menu offers, with
   *  nothing on the button to say so. */
  const emptyCtl = useStartChoice();
  /** Same as the menu, but choose the directory first. The empty pane's copy is
   *  the only instruction a first run gets, and it used to name two buttons
   *  that live in a panel you can close — and that on a fresh machine has
   *  nothing in it to press them on. */
  const browseNewSession = useCallback(async () => {
    if (!emptyCtl.ready) {
      setNotice("No agent CLI found on this machine.");
      return;
    }
    try {
      const picked = await openDialog({ directory: true, title: "Start a session in…" });
      if (typeof picked !== "string") return;
      newSession(picked, emptyCtl.choice());
    } catch {
      /* cancelled, or no chooser available */
    }
  }, [newSession, emptyCtl]);

  // --- splitter dragging ---
  const dragging = useRef<null | "left" | "right" | "rightsplit" | "agentsplit">(null);
  const rightColRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const move = (e: MouseEvent) => {
      if (!dragging.current) return;
      e.preventDefault();
      if (dragging.current === "left") {
        setSizes((s) => ({
          ...s,
          left: Math.max(140, Math.min(window.innerWidth - 260, e.clientX)),
        }));
      } else if (dragging.current === "right") {
        setSizes((s) => ({
          ...s,
          right: Math.max(150, Math.min(window.innerWidth - 260, window.innerWidth - e.clientX)),
        }));
      } else if (dragging.current === "rightsplit" && rightColRef.current) {
        const r = rightColRef.current.getBoundingClientRect();
        const frac = (e.clientX - r.left) / r.width;
        setSizes((s) => ({ ...s, explorerFrac: Math.max(0.15, Math.min(0.85, frac)) }));
      } else if (dragging.current === "agentsplit" && rightColRef.current) {
        const r = rightColRef.current.getBoundingClientRect();
        const frac = (r.bottom - e.clientY) / r.height;
        setSizes((s) => ({ ...s, agentFrac: Math.max(0.12, Math.min(0.7, frac)) }));
      }
    };
    const up = () => {
      dragging.current = null;
      document.body.classList.remove("dragging");
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, []);

  const startDrag = (which: "left" | "right" | "rightsplit" | "agentsplit") => {
    dragging.current = which;
    document.body.classList.add("dragging");
  };

  const showRight = showExplorer || showGit;

  return (
    <div className="app">
      {notice && (
        <div className="app-toast" role="status" onClick={() => setNotice(null)}>
          {notice}
        </div>
      )}
      <div className="topbar">
        <div className="topbar-left">
          <button
            className={"icon-btn" + (showSessions ? " on" : "")}
            title="Toggle sessions panel"
            onClick={() => setShowSessions(!showSessions)}
          >▤</button>
          <button
            className={"icon-btn" + (showExplorer ? " on" : "")}
            title="Toggle file explorer"
            onClick={() => setShowExplorer(!showExplorer)}
          >🗀</button>
          <button
            className={"icon-btn" + (showGit ? " on" : "")}
            title="Toggle repository panel"
            onClick={() => setShowGit(!showGit)}
          >⎇</button>
          <button
            className={"icon-btn" + (showAgent ? " on" : "")}
            title="Toggle tasks panel"
            onClick={() => setShowAgent(!showAgent)}
          >☑</button>
          <button
            className={"icon-btn" + (showComposer ? " on" : "")}
            title="Toggle input composer"
            onClick={() => setShowComposer(!showComposer)}
          >⌨</button>
          <UsagePanel sources={usageSources} onRefresh={readUsage} refreshing={usageBusy} />
          <Clock />
        </div>
        <div className="topbar-title">
          {activeProject ? activeProject.replace(/^\/home\/[^/]+/, "~") : "aiterm"}
        </div>
        <div className="topbar-right">
          <button className="icon-btn" title="Smaller fonts (Ctrl+-)" onClick={() => bumpFont(-1)}>A−</button>
          <button
            className="icon-btn"
            title="Reset font size (Ctrl+0)"
            onClick={() => bumpFont(0)}
          >{Math.round(fontScale * 100)}%</button>
          <button className="icon-btn" title="Larger fonts (Ctrl+=)" onClick={() => bumpFont(1)}>A+</button>
          <button
            className={"icon-btn" + (showSettingsModal ? " on" : "")}
            title="Settings"
            onClick={() => setShowSettingsModal(!showSettingsModal)}
          >⚙</button>
        </div>
      </div>
      <div className="main">
        {showSessions && (
          <>
            <div className="panel sessions" style={{ width: sizes.left, ...zoomFor("sessions") }}>
              <SessionsPanel
                sessions={sessions}
                projects={projects}
                activeProject={activeProject}
                liveSlots={new Set(tabs.map((t) => t.slotId))}
                liveSessions={liveSessions}
                attentionSlots={new Set(
                  tabs.filter((t) => attention.has(t.key)).map((t) => t.slotId),
                )}
                activeSlot={activeTabObj?.slotId ?? null}
                opts={opts}
                onOptsChange={setOpts}
                onSelect={selectSession}
                onResume={resumeSession}
                onFork={(s) =>
                  forkSession(s).catch((e) =>
                    setNotice(`Couldn't fork "${s.title}": ${e}`),
                  )
                }
                onExit={exitSession}
                onNewShell={newShell}
                onDelete={deleteSession}
                onSelectProject={selectProject}
                onProjectShell={projectShell}
                onProjectClaude={projectClaude}
                onNewSession={newSession}
                pending={pendingSessions}
                onSelectPending={focusSlot}
                onExitPending={closeSlot}
                onRefresh={refreshSessions}
                trashed={trashed}
                onRestore={restoreTrashed}
                onTrashDelete={deleteTrashed}
                onTrashEmpty={emptyTrash}
              />
            </div>
            <div className="splitter v" onMouseDown={() => startDrag("left")} />
          </>
        )}

        <div className="panel terminal-panel">
          <div className="term-stack">
            {tabs.map((t) => (
              <TerminalView
                key={t.key}
                tab={t}
                active={t.key === activeTab}
                onExit={handleTermExit}
                onRegister={registerHandle}
                onActivity={noteActivity}
                onAttention={noteAttention}
                autoFocus
                fontSize={termFont}
                fontFamily={xtermFont}
                theme={xtermTheme}
              />
            ))}
            {activeTab !== null && ended.has(activeTab) && (
              <div className="term-ended">
                <div className="term-ended-box">
                  {/* A shell tab has no conversation behind it, so it gets none
                      of the claude wording. Promising that "the transcript is
                      still on disk" to someone who just typed `exit 3` in a
                      shell is a reassurance about something that never
                      existed — and a dialog that says one false thing is not
                      worth trusting about the true ones. */}
                  <div className="term-ended-title">
                    {activeTabObj?.sessionId
                      ? "This session ended on its own"
                      : "This shell ended on its own"}
                  </div>
                  <div className="term-ended-sub">
                    {describeEnd(ended.get(activeTab))}
                    {activeTabObj?.sessionId
                      ? " Nothing was lost — the transcript is still on disk."
                      : ""}
                  </div>
                  {activeTabObj?.sessionId && (
                    <div className="term-ended-sub dim">
                      A session listed by <code>claude agents</code> can be stopped from
                      any terminal, or from your phone. That looks exactly like this.
                    </div>
                  )}
                  <div className="term-ended-acts">
                    <button className="tui-pick" onClick={() => restartEnded(activeTab)}>
                      {activeTabObj?.sessionId ? "Resume it" : "Open a new shell"}
                    </button>
                    <button className="tui-plain" onClick={() => closeTab(activeTab)}>
                      Close tab
                    </button>
                  </div>
                </div>
              </div>
            )}
            {tabs.length === 0 && !previewSession && (
              <div className="empty-note big empty-start">
                <div>Pick a session on the left — ▶ resumes it, ＋ opens a shell</div>
                <div className="empty-start-controls">
                  <StartControls ctl={emptyCtl} />
                  <button className="tui-pick" onClick={browseNewSession}>
                    Start a new session…
                  </button>
                </div>
              </div>
            )}
            {previewSession && (
              <SessionPreview
                session={previewSession}
                onResume={resumeSession}
                onClose={() => setPreviewSession(null)}
              />
            )}
            {tui && !tuiDismissed && activeTab !== null && (
              tui.kind === "rewind-picker" || tui.kind === "rewind-confirm" ? (
                <TuiRewind
                  step={tui}
                  write={(d) => handles.current.get(activeTab)?.write(d)}
                  screen={() => handles.current.get(activeTab)?.screen() ?? []}
                  onDismiss={() => dismissTui(activeTab)}
                />
              ) : tui.kind === "model-picker" ? (
                <TuiModelPicker
                  picker={tui}
                  write={(d) => handles.current.get(activeTab)?.write(d)}
                  screen={() => handles.current.get(activeTab)?.screen() ?? []}
                  onDismiss={() => dismissTui(activeTab)}
                />
              ) : tui.kind === "model-confirm" ? (
                <TuiModelConfirm
                  confirm={tui}
                  write={(d) => handles.current.get(activeTab)?.write(d)}
                  screen={() => handles.current.get(activeTab)?.screen() ?? []}
                  onDismiss={() => dismissTui(activeTab)}
                />
              ) : (
                <TuiPermission
                  request={tui}
                  write={(d) => handles.current.get(activeTab)?.write(d)}
                  screen={() => handles.current.get(activeTab)?.screen() ?? []}
                  onDismiss={() => dismissTui(activeTab)}
                />
              )
            )}
          </div>
          {/* onCommand goes to the focused terminal, so the pills only offer
              model/effort when there is a live session to run them in — and
              not while the agents view has the screen, where a slash command
              would be typed into its "describe a task" box instead of run. */}
          {showComposer && <Composer
            sessionId={activeSessionId}
            projectRoot={activeProject}
            usage={claudeUsage?.bars ?? []}
            usageAsOf={claudeUsage?.stale ? claudeUsage.at : null}
            onCommand={activeTab === null || agentsView ? undefined : (text) =>
              handles.current.get(activeTab)?.sendComposed(text)}
            onDismiss={activeTab === null ? undefined : () =>
              handles.current.get(activeTab)?.focus()}
            hasPendingInput={activeTab === null ? undefined : () =>
              handles.current.get(activeTab)?.pendingInput() ?? false}
            onOpenModelPicker={activeTab === null || agentsView ? undefined : openModelPicker}
            onOpenRewind={activeTab === null || agentsView ? undefined : openRewind}
            permMode={permMode}
            onSetPermMode={activeTab === null || agentsView ? undefined : setPermissionMode}
          />}
        </div>

        {showRight && (
          <>
            <div className="splitter v" onMouseDown={() => startDrag("right")} />
            <div className="right-col" ref={rightColRef} style={{ width: sizes.right }}>
              <div
                className="right-top"
                style={{ height: showAgent ? `${(1 - sizes.agentFrac) * 100}%` : "100%" }}
              >
                {showExplorer && (
                  <div
                    className="panel explorer"
                    style={{ width: showGit ? `${sizes.explorerFrac * 100}%` : "100%", ...zoomFor("explorer") }}
                  >
                    <div className="panel-header">
                      <span>EXPLORER</span>
                      <button className="icon-btn" onClick={() => setShowExplorer(false)}>✕</button>
                    </div>
                    <FileExplorer root={activeProject} refreshKey={explorerRefresh} />
                  </div>
                )}
                {showExplorer && showGit && (
                  <div className="splitter v" onMouseDown={() => startDrag("rightsplit")} />
                )}
                {showGit && (
                  <div className="panel git" style={{ flex: 1, minWidth: 0, ...zoomFor("git") }}>
                    <div className="panel-header">
                      <span>REPOSITORY</span>
                      <div>
                        <button className="icon-btn" title="Refresh"
                          onClick={() => setGitRefresh((n) => n + 1)}>⟳</button>
                        <button className="icon-btn" onClick={() => setShowGit(false)}>✕</button>
                      </div>
                    </div>
                    <GitPanel root={activeProject} refreshKey={gitRefresh} />
                  </div>
                )}
              </div>
              {showAgent && (
                <>
                  <div className="splitter h" onMouseDown={() => startDrag("agentsplit")} />
                  <div className="panel agent" style={{ flex: 1, minHeight: 0, ...zoomFor("agent") }}>
                    <div className="panel-header">
                      <span>AGENT{activeTabObj?.title ? ` — ${activeTabObj.title}` : ""}</span>
                      <button className="icon-btn" onClick={() => setShowAgent(false)}>✕</button>
                    </div>
                    <AgentPanel sessionId={activeSessionId} />
                  </div>
                </>
              )}
            </div>
          </>
        )}
      </div>
      {showSettingsModal && (
        <SettingsModal
          settings={settings}
          onChange={setSettings}
          onClose={() => setShowSettingsModal(false)}
        />
      )}
    </div>
  );
}

function basename(p: string): string {
  return p.split("/").filter(Boolean).pop() ?? p;
}

// Backslash-escape (the way terminals escape dropped paths) — claude's
// pasted-path detection understands this form, unlike single quotes.
function shellEscape(p: string): string {
  return p.replace(/[^A-Za-z0-9_\-./~+:@%=]/g, (c) => "\\" + c);
}
