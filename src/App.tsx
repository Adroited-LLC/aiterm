import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import SessionsPanel, { SessionDisplayOpts } from "./components/SessionsPanel";
import { StartChoice } from "./components/NewSessionMenu";
import StartControls, { useStartChoice } from "./components/StartControls";
import TerminalView, { TermHandle, TermProgress, TermTab } from "./components/TerminalView";
import AlertBell, { Alert } from "./components/AlertBell";
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
import { UsageBars } from "./components/UsageBars";
import { Clock } from "./components/Clock";
import {
  AppSettings, applySettings, loadSettings, saveSettings, termFontFamily, termTheme,
} from "./settings";
import {
  Caps,
  ProjectInfo, Session,
  TrashedSession,
  UsageBar,
  agentCaps,
  listProjects, listSessions, materializeFork,
  reindexSessions, sessionFork, uiLog, usageLimits,
  resolveResumableId, liveSessionIds, stopSession, unstoppableSessionIds, sessionMovedTo,
  drainSessionEvents,
  sessionDelete, trashDelete, trashEmpty, trashList, trashRestore,
  watchProject,
  adoptAgentSession, resolveLaunch,
  taskbarBadge,
  trayAlerts,
  desktopNotify, desktopNotifyClose,
} from "./ipc";
import "./App.css";

const OPTS_KEY = "aiterm.sessionOpts";
const SIZES_KEY = "aiterm.panelSizes";
const FONT_KEY = "aiterm.fontScale";
const PANELS_KEY = "aiterm.panelToggles";
const USAGE_KEY = "aiterm.usageCache";

// (The constant that used to sit here — claude's own flags, spelled out in the
// renderer as a fallback for the backend's answer — is gone. Every command a
// tab runs now comes from `resolveLaunch`, which is the one place that knows
// what any engine's command line looks like.)

/** What an engine that is not in the registry can do: nothing.
 *
 *  The honest answer for a session row whose engine has since been removed, and
 *  for a tab with no engine at all (a plain shell). Both used to fall through
 *  to claude's affordances by default, which offered a ▶ that could only open a
 *  black pane. */
const NO_CAPS: Caps = {
  fork: false, clear: false, resume: false, tui_drive: false, panels: false,
  delete: false, config: false,
};

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

/** Everything about a tab past "what it is and what it runs".
 *
 *  One object rather than a tail of positional arguments: these are all
 *  optional and mostly unrelated, so the call sites that set the last one were
 *  spelling `undefined` three times to get to it. */
interface OpenTabOpts {
  sessionId?: string;
  fresh?: boolean;
  adopt?: TermTab["adopt"];
  envProvider?: string;
  envModel?: string;
  /** Which engine this tab runs — `LaunchPlan.agent_id`. Omitted for a plain
   *  shell, which runs none. See `TermTab.agentId`. */
  agentId?: string;
  /** The session this tab was opened to reopen — see `TermTab.resumedId`. */
  resumedId?: string;
}

function loadJSON<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? { ...fallback, ...JSON.parse(raw) } : fallback;
  } catch {
    return fallback;
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
  /** The message behind a badge, when the program sent one (OSC 9). */
  const [notices, setNotices] = useState<Map<number, string>>(new Map());
  /** Long-running work a tab is reporting (OSC 9;4). */
  const [progress, setProgress] = useState<Map<number, TermProgress>>(new Map());
  /** When each waiting tab started waiting — the bell lists oldest first, and
   *  "waiting 20m" is the part that decides which one to open. */
  const [alertAt, setAlertAt] = useState<Map<number, number>>(new Map());
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

  // The tab on screen and the session it is keyed to. Read this high up
  // because everything claude-shaped below gates on which engine is in it.
  const activeTabObj = tabs.find((t) => t.key === activeTab) ?? null;
  const activeSessionId = activeTabObj?.sessionId ?? null;

  // What each engine supports, fetched once on mount. `agent_caps` asks the
  // registry and nothing else — no PATH probe, no `--version` — because these
  // gate what is *drawn*: which buttons a row offers, and whether the screen
  // poll and the transcript panels run at all. Until it answers everything
  // reads as capable of nothing, which is the safe direction: a feature not
  // running for one frame beats one misfiring against the wrong engine.
  const [caps, setCaps] = useState<Record<string, Caps>>({});
  useEffect(() => {
    agentCaps().then(setCaps).catch(() => { /* keep the all-false default */ });
  }, []);
  /** What `agent` supports — nothing, for an id no backend claims. */
  const capsOf = (agent: string) => caps[agent] ?? NO_CAPS;
  /** The engine in the tab on screen. A shell has none, and answers all-false. */
  const activeCaps = capsOf(activeTabObj?.agentId ?? "");
  /** Whether to watch this tab's screen for a TUI.
   *
   *  A tab aiterm launched declares its engine and is taken at its word. A tab
   *  that declares none is a plain shell — and a shell is exactly where someone
   *  types `claude` by hand, which is how the dialogs were reached before there
   *  was a ＋ menu. The poll is a detector, so the undeclared tab is the one
   *  place it still earns its keep; an engine that told us it drives no TUI is
   *  the case worth switching off. */
  const activeDrivesTui = activeTabObj?.agentId ? activeCaps.tui_drive : true;

  // The sessions list, read without putting it in a callback's deps:
  // `newSession` needs to snapshot what already exists, and it must not be
  // rebuilt every time the list refreshes.
  const sessionsRef = useRef<Session[]>(sessions);
  sessionsRef.current = sessions;

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
    // Everything below parses Claude Code's own screens — its `/model` picker,
    // its permission prompts, its `/rewind` steps, its agents view. Against any
    // other engine it is forty lines of somebody else's terminal being matched
    // against claude's phrasing four times a second, and a false positive would
    // draw a dialog that types into a TUI it cannot drive.
    //
    // So a tab that does not drive a TUI schedules no interval at all, and
    // whatever the last tab left behind is cleared on the way out: an open
    // Tui* dialog would otherwise stay on screen over a terminal it no longer
    // belongs to, still sending keystrokes there when answered. The permission
    // mode and the agents-view flag go with it — both describe the tab that has
    // just gone off screen, and both gate other chrome.
    if (!activeDrivesTui) {
      setTui(null);
      setTuiDismissed(false);
      setPermMode(null);
      setAgentsView(false);
      armed.current = null;
      return;
    }
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
  }, [activeTab, activeDrivesTui]);

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
  // Session ids our own terminals have demonstrably moved OFF of — a /clear or
  // an in-place /fork re-keyed the tab away. The roster goes on reporting an
  // interactive entry under the id it *started* with for as long as the client
  // process lives, so the finished conversation's row would wear a green dot
  // indefinitely (Matt hit this on /fork: the marker heuristic that filters a
  // cleared parent from live_session_ids knows nothing about fork children).
  // This is hook evidence, not inference — the pty that owned the id told us
  // it left. An id is taken back off the list the moment anything reopens it:
  // a tab slotted to it, or a hook report of it starting again.
  const [vacated, setVacated] = useState<Set<string>>(new Set());
  const liveShown = useMemo(() => {
    if (vacated.size === 0) return liveSessions;
    const out = new Set(liveSessions);
    for (const id of vacated) out.delete(id);
    return out;
  }, [liveSessions, vacated]);
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

  // Plan usage, fetched once for everything that shows it. `/api/oauth/usage`
  // rate limits, so a second poller is not just waste — a refused request
  // returns [] and that view blanks while the other still shows bars.
  // Keep the last good reading: [] means "couldn't ask", never "zero usage".
  //
  // Seeded from the last reading written to disk, so a cold start shows
  // something immediately instead of an empty strip for up to a minute — the
  // first call often lands on a rate limit, which made the gap longer still.
  // Written on every success rather than on exit: an exit hook does not run
  // when the app is killed, which is exactly when the cache would be wanted.
  const cached = loadJSON(USAGE_KEY, { at: 0, bars: [] as UsageBar[] });
  const [usage, setUsage] = useState<UsageBar[]>(cached.bars);
  const [usageAt, setUsageAt] = useState<number>(cached.at);
  const [usageFresh, setUsageFresh] = useState(false);
  useEffect(() => {
    let alive = true;
    const load = () =>
      usageLimits()
        .then((b) => {
          if (!alive || !b.length) return;
          setUsage(b);
          setUsageAt(Date.now());
          setUsageFresh(true);
          try {
            localStorage.setItem(USAGE_KEY, JSON.stringify({ at: Date.now(), bars: b }));
          } catch { /* quota — the cache is a convenience, not a requirement */ }
        })
        .catch(() => {});
    load();
    const iv = setInterval(load, 60_000);
    return () => { alive = false; clearInterval(iv); };
  }, []);

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
     opts: OpenTabOpts = {}) => {
      setPreviewSession(null);
      // Opening a terminal on a slot is the deliberate act of going back to
      // it — if its id was on the vacated list (green dot suppressed), it has
      // earned the dot again the moment the roster reports it.
      setVacated((prev) => {
        if (!prev.has(slotId)) return prev;
        const next = new Set(prev);
        next.delete(slotId);
        return next;
      });
      setTabs((t) => {
        const existing = t.find((x) => x.slotId === slotId);
        if (existing) {
          setActiveTab(existing.key);
          return t;
        }
        const key = nextKey.current++;
        setActiveTab(key);
        return [...t, { key, title, cwd, command, slotId, ...opts }];
      });
    },
    [],
  );

  const registerHandle = useCallback((key: number, handle: TermHandle | null) => {
    if (handle) handles.current.set(key, handle);
    else handles.current.delete(key);
  }, []);

  /** The last time a tab changed which conversation it holds. The sidebar's
   *  click-selection follows this: without it the selection keeps pointing at
   *  the conversation you left, wearing the loudest highlight of the three
   *  while the live row wears the faintest. Carries a sequence so two
   *  identical moves still read as two events. */
  const [rekey, setRekey] = useState<{ from: string; to: string; seq: number } | null>(null);
  const rekeySeq = useRef(0);
  const noteRekey = useCallback((from: string, to: string) => {
    rekeySeq.current += 1;
    setRekey({ from, to, seq: rekeySeq.current });
  }, []);

  /** Everything waiting on you, oldest first — the bell list and the taskbar
   *  number are two views of this one thing. */
  const alerts: Alert[] = useMemo(
    () =>
      tabs
        .filter((t) => attention.has(t.key))
        .map((t) => ({
          key: t.key,
          title: t.title,
          message: notices.get(t.key),
          at: alertAt.get(t.key) ?? Date.now(),
        }))
        .sort((a, b) => a.at - b.at),
    [tabs, attention, notices, alertAt],
  );

  // The count on the icon, for when aiterm is behind another window — which is
  // the only time it matters, and the only time the in-app bell cannot be seen.
  useEffect(() => {
    taskbarBadge(alerts.length).catch(() => {});
  }, [alerts.length]);

  // The tray menu carries the list itself, so a waiting session can be picked
  // without bringing aiterm forward first to read the bell.
  useEffect(() => {
    trayAlerts(
      alerts.map((a) => ({ key: a.key, title: a.title, message: a.message })),
    ).catch(() => {});
  }, [alerts]);

  /** Daemon ids of popups currently on screen, by tab. A ref, not state: these
   *  are the desktop's business, and re-rendering over them would be noise. */
  const popups = useRef<Map<number, number>>(new Map());

  // A popup only when aiterm is not the window you are looking at — if it is,
  // the bell is right there and a notification would be telling you something
  // you can already see.
  useEffect(() => {
    const waiting = new Set(alerts.map((a) => a.key));
    for (const [key, id] of popups.current) {
      if (!waiting.has(key)) {
        popups.current.delete(key);
        desktopNotifyClose(id).catch(() => {});
      }
    }
    if (document.hasFocus()) return;
    for (const a of alerts) {
      if (popups.current.has(a.key)) continue;
      // Claimed before the await so a second pass cannot post a duplicate
      // while the first is still in flight.
      popups.current.set(a.key, 0);
      desktopNotify(a.title, a.message ?? "Waiting for your input", 0)
        .then((id) => {
          if (id && popups.current.get(a.key) === 0) popups.current.set(a.key, id);
        })
        .catch(() => popups.current.delete(a.key));
    }
  }, [alerts]);

  // Picking one there means going to it; the window was already raised by the
  // backend before this fires.
  useEffect(() => {
    const un = listen<number>("tray-alert", (e) => setActiveTab(e.payload));
    return () => {
      un.then((f) => f()).catch(() => {});
    };
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
    setAlertAt((prev) => {
      if (on === prev.has(key)) return prev;
      const next = new Map(prev);
      if (on) next.set(key, Date.now());
      else next.delete(key);
      return next;
    });
    if (on && !document.hasFocus()) {
      getCurrentWindow()
        .requestUserAttention(UserAttentionType.Informational)
        .catch(() => {});
    }
    // The badge and the sentence behind it clear together — a message left
    // behind after the badge went is a stale answer to "what did it want?".
    if (!on) {
      setNotices((prev) => {
        if (!prev.has(key)) return prev;
        const next = new Map(prev);
        next.delete(key);
        return next;
      });
    }
  }, []);

  /** What a tab asked for, in its own words (OSC 9), keyed like `attention`. */
  const noteNotify = useCallback((key: number, message: string) => {
    setNotices((prev) => {
      if (prev.get(key) === message) return prev;
      return new Map(prev).set(key, message);
    });
  }, []);

  const noteProgress = useCallback((key: number, p: TermProgress | null) => {
    setProgress((prev) => {
      if (p === null) {
        if (!prev.has(key)) return prev;
        const next = new Map(prev);
        next.delete(key);
        return next;
      }
      const had = prev.get(key);
      if (had && had.state === p.state && had.pct === p.pct) return prev;
      return new Map(prev).set(key, p);
    });
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

  // Sessions aiterm has started that are not on disk yet. The sidebar lists
  // transcripts, and claude writes none until the first prompt, so without
  // these a brand-new session is a terminal with no row — unreachable the
  // moment you look at something else, since the sidebar is how you get back
  // to a tab. Each one retires by itself: the id was minted by `newSession`,
  // so when that transcript lands the real row is already the tab's row and
  // this list stops naming it. Nothing guesses, and nothing hides a real row.
  //
  // Keyed by `slotId`, not `sessionId`. For claude the two are the same value —
  // `newSession` mints one id and passes it as both — but an agent that has no
  // `--session-id` gets a slot and no session id at all, and filtering on the
  // session id dropped exactly those tabs: Codex opened a terminal with no row,
  // which is the unreachable-tab bug this list exists to prevent. `slotId` is
  // also what the panel hands back to `onSelectPending`/`onExitPending`, so it
  // is the id this list should have been carrying either way.
  const knownSessionIds = useMemo(() => new Set(sessions.map((s) => s.id)), [sessions]);
  const pendingSessions = useMemo(
    () =>
      tabs
        .filter((t) => t.fresh && !knownSessionIds.has(t.slotId))
        // The engine comes along so the placeholder wears its own mark. Without
        // it every pending row drew claude's starburst, so a fresh Codex or
        // OpenCode tab spent its first minutes claiming to be a claude session.
        .map((t) => ({
          id: t.slotId, title: t.title, cwd: t.cwd ?? "", agent: t.agentId ?? "",
        })),
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

  // Tabs still waiting to learn their session id, as a string that only
  // changes when the set does. Depending on `tabs` here would re-arm the
  // watcher below on every keystroke's worth of tab state.
  const awaitingAdoption = tabs
    .filter((t) => t.adopt && !t.sessionId)
    .map((t) => t.key)
    .join(",");

  // Agents with no `--session-id` name themselves, and only once they start.
  // Until aiterm learns that name the tab is keyed to a handle no session will
  // ever have: the placeholder row stays at the top of the sidebar forever,
  // and the moment the agent writes its transcript the same conversation gets
  // a second, real row under its project. Adopting the id collapses the two.
  //
  // Codex writes at launch rather than at first prompt, so this usually
  // resolves on the first tick.
  useEffect(() => {
    if (!awaitingAdoption) return;
    let stop = false;
    const tick = async () => {
      for (const t of tabsRef.current) {
        if (stop) return;
        if (!t.adopt || t.sessionId || !t.cwd) continue;
        try {
          const id = await adoptAgentSession(
            t.adopt.agentId, t.cwd, t.adopt.since, t.adopt.known,
          );
          if (stop || !id) continue;
          setTabs((list) =>
            list.map((x) =>
              x.key === t.key
                ? { ...x, sessionId: id, slotId: id, fresh: false, adopt: undefined }
                : x,
            ),
          );
          uiLog(`adopted ${t.adopt.agentId} session ${id} for tab ${t.key}`);
          refreshSessionList();
        } catch {
          /* keep waiting — the agent may not have written anything yet */
        }
      }
    };
    tick();
    const every = setInterval(tick, 2000);
    // Give up after a minute. An agent that has written nothing by then is one
    // that does not write transcripts at all, and polling the disk forever on
    // its behalf is worse than leaving the placeholder row in place — which
    // still keeps the tab reachable, which was the point.
    const giveUp = setTimeout(() => {
      stop = true;
      clearInterval(every);
    }, 60_000);
    return () => {
      stop = true;
      clearInterval(every);
      clearTimeout(giveUp);
    };
  }, [awaitingAdoption, refreshSessionList]);

  // A conversation can change its session id while staying in this same pty.
  // Two things do it: moving to the daemon (all that opening the agents view
  // does) and `/clear`. Either way the tab's pinned id was set once when it
  // opened and nothing ever moved it, so from that moment the terminal renders
  // the live conversation while every panel keyed to the tab reads the old
  // transcript: a file nothing is writing. Live text, dead clock — and the live
  // conversation's own sidebar row belongs to no tab, so clicking it opens a
  // SECOND agent on it. Re-key the tab when it happens, and say so, because the
  // row the sidebar shows for this conversation changes underneath the user.
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
    const resumedId = activeTabObj?.resumedId;
    if (key === undefined || !pinned) return;
    // Compaction, `/clear` and the move to the daemon are all things a
    // TUI-driving engine does to its own transcripts. `sessionMovedTo` resolves
    // through the whole registry, so without this an `aiterm chat` tab would
    // have claude's heuristics run over the chats directory and could be
    // re-keyed — and told it had been cleared — on a false positive.
    if (!activeCaps.tui_drive) return;
    // This runs even for tabs the SessionStart hook reports on (drain effect
    // below): the hook cannot see a move to the daemon — that claude is not
    // ours — so the Background kind is still this watcher's alone. For a
    // clear, both mechanisms re-key to the same id, so whichever fires first
    // settles it and the other finds nothing left to do.
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
        const moved = await sessionMovedTo(pinned);
        if (stop || !moved || moved.id === pinned) return;
        // A tab opened to reopen `pinned` is deliberately holding that
        // conversation. Its `/clear` child on disk is history, not news — the
        // last version of this stole the tab the instant Resume was clicked,
        // re-keyed it onto the old clear's successor, and left the actual
        // resume minting a third session (observed 2026-07-29 21:36, three
        // kill_trees of cleanup). "It moved once" is forever true on disk;
        // "this tab wants the original" beats it.
        //
        // Compares what the tab was opened *for*, not what its command string
        // looks like. Sniffing the text for `--resume <id>` was always the weak
        // version — the tab knows its own intent, and the substring stopped
        // matching the moment the backend started shell-quoting the id.
        if (moved.kind === "cleared" && resumedId === pinned) return;
        uiLog(`migrate: re-keying tab ${key} ${pinned} -> ${moved.id} (${moved.kind})`);
        // `slotId` moves with `sessionId`, not just the pinned id. The slot is
        // what links a tab to a sidebar row — the live dot, the highlight, and
        // the click that focuses an existing terminal instead of opening
        // another. Leaving it on the old id was the second half of this bug:
        // the tab kept the frozen row and the live one looked free to open.
        //
        // Nothing else changes: for a clear, the conversation left behind
        // becomes an ordinary stopped row in the sidebar — click for its
        // preview, ▶ to resume — exactly like any session that ended. (An
        // earlier version manufactured a special "historical" tab for it,
        // blank, with its own explanatory buttons. Matt: make it look like a
        // normal stop instead.) `fresh` + retitle for the cleared kind mirror
        // the hook path above, and for the same reasons.
        const cleared = moved.kind === "cleared";
        setTabs((ts) =>
          ts.map((x) =>
            x.key === key
              ? {
                  ...x,
                  sessionId: moved.id,
                  slotId: moved.id,
                  fresh: cleared ? true : x.fresh,
                  title: cleared ? basename(x.cwd ?? "") || x.title : x.title,
                }
              : x,
          ),
        );
        noteRekey(pinned, moved.id);
        setVacated((prev) => new Set(prev).add(pinned));
        setNotice(
          moved.kind === "cleared"
            ? `"${title}" was cleared — this tab is the new conversation. The old one is in the sidebar, resumable.`
            : `"${title}" moved to a background session — its panels now follow the live one.`,
        );
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
  }, [activeTabObj?.key, activeTabObj?.sessionId, activeTabObj?.title,
      activeTabObj?.resumedId, freshUnwritten, activeCaps.tui_drive,
      refreshSessionList, agentsView]);

  // The exact channel: every claude aiterm launches carries a SessionStart
  // hook (see hooklink.rs) that reports its session id, its cause and its pid
  // into a spool. Drained here, each event lands on the tab whose pty owns
  // that pid — so a `/clear` re-keys within two seconds, on any tab, active
  // or not, with no inference anywhere in the path. The watcher above stays:
  // it alone sees daemon moves, and it covers claudes with no hook — one
  // typed into a shell tab, or from before this build.
  useEffect(() => {
    let stop = false;
    const drain = async () => {
      if (tabsRef.current.length === 0) return;
      let events;
      try {
        events = await drainSessionEvents();
      } catch {
        return; // backend unavailable; the spool keeps until it is back
      }
      if (stop) return;
      for (const evt of events) {
        const entry = [...handles.current.entries()].find(
          ([, h]) => h.ptyId === evt.ptyId,
        );
        const tab = entry && tabsRef.current.find((t) => t.key === entry[0]);
        if (!tab) continue;
        // A session starting (again) under an id is the definition of "not
        // vacated" — however it left, it's back.
        setVacated((prev) => {
          if (!prev.has(evt.sessionId)) return prev;
          const next = new Set(prev);
          next.delete(evt.sessionId);
          return next;
        });
        // Only re-key tabs that are keyed. A shell tab someone ran `claude`
        // in, or a project-▶ tab, is slotted by its path — handing it a
        // session slot would break the "one terminal per project door" dedupe
        // without buying anything the panels don't already do.
        if (!tab.sessionId || tab.sessionId === evt.sessionId) continue;
        const old = tab.sessionId;
        // `clear` and `fork` both mean: this terminal now holds a NEW
        // conversation and the old id is finished. (`fork` was discovered
        // live 2026-07-30 — claude's own /fork switches the terminal to the
        // branch, hook source "fork", and nothing on disk links the two.)
        const newConversation = evt.source === "clear" || evt.source === "fork";
        uiLog(`hook: tab ${tab.key} ${old} -> ${evt.sessionId} (${evt.source})`);
        setTabs((ts) =>
          ts.map((x) =>
            x.key === tab.key
              ? {
                  ...x,
                  sessionId: evt.sessionId,
                  slotId: evt.sessionId,
                  // A new conversation has no sidebar row until its first real
                  // prompt lands (meta-only transcripts aren't listed), which
                  // left the highlight orphaned — you looked "still clicked"
                  // on the old row. `fresh` is the existing answer: the
                  // placeholder row names this tab, highlighted, until the
                  // real row appears under the same id and takes over.
                  fresh: newConversation ? true : x.fresh,
                  // And it isn't the old conversation, so it sheds the old
                  // title rather than impersonating it in the sidebar.
                  title: newConversation
                    ? basename(x.cwd ?? "") || x.title
                    : x.title,
                }
              : x,
          ),
        );
        noteRekey(old, evt.sessionId);
        if (newConversation) {
          // The roster keeps listing the old id while the client process
          // lives; only we know the terminal left it behind.
          setVacated((prev) => new Set(prev).add(old));
          setNotice(
            evt.source === "clear"
              ? `"${tab.title}" was cleared — this tab is the new conversation. ` +
                `The old one is in the sidebar, resumable.`
              : `"${tab.title}" forked — this tab is the branch. ` +
                `The original conversation is in the sidebar, resumable.`,
          );
        }
        refreshSessionList();
      }
    };
    drain();
    const iv = setInterval(drain, 2000);
    return () => {
      stop = true;
      clearInterval(iv);
    };
  }, [refreshSessionList]);
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
    async (key: number) => {
      const t = tabsRef.current.find((x) => x.key === key);
      if (!t) return;
      closeTab(key);
      // A tab with no session id has no conversation to continue — a shell, or
      // an engine that never named itself — so it reopens on the command it
      // was launched with. So does one whose engine declines to reopen: the
      // resolver saying no is exactly what that fallback is for.
      let command = t.command;
      let resumedId = t.resumedId;
      // The engine carries over when the plan is declined, because the tab is
      // reopening on the very command it ran before — same engine, so the same
      // capabilities.
      let agentId = t.agentId;
      if (t.sessionId) {
        try {
          const plan = await resolveLaunch({ kind: "restart", sessionId: t.sessionId });
          command = plan.command;
          resumedId = t.sessionId;
          agentId = plan.agent_id;
        } catch {
          /* nothing here can reopen it — keep the tab's own command */
        }
      }
      // The provider environment carries over too: it is what gives the tab
      // its key and its routing, and a restart that dropped it would reopen
      // the same command against no provider at all.
      openTab(t.title, t.cwd, command, t.slotId, {
        sessionId: t.sessionId, resumedId, agentId,
        envProvider: t.envProvider, envModel: t.envModel,
      });
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
    // Ask the pinned id first, because the plan is also how we learn which
    // engine this row belongs to — and everything below is claude's.
    let plan;
    try {
      plan = await resolveLaunch({ kind: "resume", sessionId: s.id });
    } catch (e) {
      setNotice(`${e}`);
      return;
    }
    // The preamble — moved-to resolution, fork redemption, roster stops — is
    // the live-process machinery of an engine that drives a TUI and holds its
    // sessions open. An engine without it (`aiterm chat` reloads a transcript
    // and carries on) must not be put through any of it: the id it was given
    // is the id it resumes. Gated on the capability rather than on the agent's
    // name, which is the check this refactor exists to remove.
    if (!plan.caps.tui_drive) {
      openTab(s.title, s.project_path, plan.command, s.id, {
        sessionId: s.id, resumedId: s.id, agentId: plan.agent_id,
      });
      return;
    }
    // The pinned id can go stale: a compaction can retire the original
    // transcript (Claude Code deletes it or renames it to `<id>.orphaned-…`),
    // so `claude --resume <original-id>` dies with "no conversation found" — a
    // black pane, the core "broken feel" of resume. (`/clear` was listed here
    // too and does not belong: measured 2026-07-29 on Claude Code 2.1.220, it
    // leaves the original named and resumable and simply starts a new session
    // beside it. That is why it needed detecting rather than resolving.)
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
    // Resume the same way we start anything: the engine's own command, which
    // carries the same flags a fresh start gets. (This used to pass
    // `--permission-mode <configured>`, which is what silently lifted a manual
    // session into bypass on resume.) Re-resolve when the id moved — the plan
    // above names the pinned session, and the continuation is a different
    // conversation to open.
    if (liveId !== s.id) {
      try {
        plan = await resolveLaunch({ kind: "resume", sessionId: liveId });
      } catch (e) {
        setNotice(`${e}`);
        return;
      }
    }
    openTab(s.title, s.project_path, plan.command, liveId, {
      sessionId: liveId, resumedId: liveId, agentId: plan.agent_id,
    });
  };
  // Branch a session. You stay exactly where you are: the backend copies the
  // transcript under a fresh id and the branch appears in the sidebar as a
  // stopped row holding the conversation up to this point — the same shape as
  // `/clear`, deliberately. One rule for both: the tab you're in never
  // changes out from under you, and the other conversation (old context for
  // a clear, the copy for a fork) is a normal stopped row — preview on
  // click, ▶ to resume.
  //
  // (Briefly, 0.10.9 opened the branch live and switched to it. Matt: no —
  // stay in session A; the branch is the stopped one.) Rejections are
  // surfaced by the caller (see `onFork` below).
  const forkSession = async (s: Session) => {
    const branchId = await sessionFork(s.id);
    refreshSessionList();
    setNotice(`Branched "${s.title}" — the copy is in the sidebar, stopped at this point.`);
    return branchId;
  };
  // aiterm's own clear, kin to ⑂ and deliberately off claude's machinery:
  // end the running claude and start a fresh one in the same tab on an id the
  // plan names. The old conversation needs nothing done to it — its transcript
  // is already on disk, so it simply becomes a stopped row, exactly the shape
  // a typed /clear settles into. Costs a claude restart where /clear is warm;
  // in exchange there is no hook, no detection and no id change to chase — the
  // id in the command and the id this tab is keyed to came from the same
  // place, so aiterm knows everything from the first frame.
  const clearSession = async (s: Session) => {
    const t = tabs.find((x) => x.slotId === s.id);
    if (!t) return;
    let plan;
    try {
      plan = await resolveLaunch({ kind: "clear", sessionId: s.id });
    } catch (e) {
      setNotice(`${e}`);
      return;
    }
    // No id on the plan means nothing would name the new conversation, and a
    // tab keyed to nothing is the unreachable-tab bug — say so instead.
    if (!plan.session_id) {
      setNotice(`Clearing isn't available for "${s.title}".`);
      return;
    }
    closeTab(t.key);
    // The client process is going down, but the roster reports it for a beat
    // longer — same suppress-until-reopened rule as a hook-observed move.
    setVacated((prev) => new Set(prev).add(s.id));
    openTab(
      basename(s.project_path), s.project_path, plan.command, plan.session_id,
      { sessionId: plan.session_id, fresh: true, agentId: plan.agent_id },
    );
    setNotice(`"${s.title}" is parked in the sidebar — this tab is a fresh conversation.`);
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
  const projectClaude = async (p: ProjectInfo) => {
    setActiveProject(p.path);
    try {
      // Naming claude is honest here: this is the "start claude here" button,
      // not a choice the user made in a picker.
      const plan = await resolveLaunch({
        kind: "agent", agentId: "claude", model: null, effort: null,
      });
      // The plan's minted session id is deliberately dropped. This tab is
      // slotted by its project door — `claude:<path>`, one terminal per
      // project — which is what makes clicking the row focus the terminal that
      // is already open rather than starting a second claude in it. Keying it
      // to the session instead would put a placeholder row in the sidebar for
      // a door that already has one.
      openTab(p.name, p.path, plan.command, `claude:${p.path}`, {
        agentId: plan.agent_id,
      });
    } catch (e) {
      setNotice(`Couldn't start claude in ${p.name}: ${e}`);
    }
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
   * **The id comes from the plan, not from here.** Letting the engine choose it
   * silently is the reason the first version of this was unusable: a session
   * has no transcript until its first prompt, the sidebar lists what is on
   * disk, and the sidebar is the tab list — so a new session opened a terminal
   * that no row named. It took the pane over from whatever you were looking at
   * and there was no way back to it. `--session-id` composes with a fresh start
   * (unlike with `--resume`, where it is rejected without `--fork-session`), so
   * for an engine that takes one the tab is slotted by its session id from the
   * first frame, exactly like a resume — which is also what lets the composer
   * pills and the agents panel key to it before it has said anything. The
   * resolver mints it, because only the backend knows whether an id will mean
   * anything.
   *
   * *Verified 2026-07-27: `claude --session-id <uuid> --permission-mode auto
   * --allow-dangerously-skip-permissions -p …` wrote its transcript to
   * `<uuid>.jsonl` under the launch cwd.*
   *
   * `fresh` covers the gap until that file exists — see `pendingSessions`.
   */
  const newSession = useCallback(async (cwd: string, choice: StartChoice) => {
    // An API-provider model is a request for a model, not for an engine —
    // which one runs it is the resolver's answer. The branch survives because
    // the two are presented differently: a model tab is titled by its model
    // and gets no placeholder row, an agent tab is titled by its directory and
    // does.
    //
    // A `switch` on the kind rather than a test for a field, because the choice
    // is one thing or the other and used to be a shape that could be both: an
    // agent id sat beside an optional `api` object, and "which of these two do
    // I mean" was a runtime question answered by whichever field was checked
    // first. There is nothing left to check.
    switch (choice.kind) {
      case "api": {
        try {
          const title = choice.modelId.split("/").pop() || choice.modelId;
          const plan = await resolveLaunch({
            kind: "apiModel",
            providerId: choice.providerId,
            modelId: choice.modelId,
          });
          setActiveProject(cwd);
          // A tab handle where the plan names no session: an engine that writes
          // no transcript we can read has nothing to key panels to, and the tab
          // is the whole life of that conversation. Where it does name one, the
          // chat becomes a real sidebar row once its first exchange lands.
          //
          // `env_provider` rides along so the spawn can put the key in the
          // tab's environment — set only for an engine that has said it
          // authenticates no other way. `env_model` names the model whose
          // routing goes in beside it; the routing is compiled in Rust.
          openTab(title, cwd, plan.command, plan.session_id ?? crypto.randomUUID(), {
            sessionId: plan.session_id ?? undefined,
            envProvider: plan.env_provider ?? undefined,
            envModel: plan.env_model ?? undefined,
            agentId: plan.agent_id,
          });
        } catch (e) {
          setNotice(`Couldn't start ${choice.modelId}: ${e}`);
        }
        return;
      }

      case "agent": {
        setActiveProject(cwd);
        try {
          const plan = await resolveLaunch({
            kind: "agent",
            agentId: choice.agentId,
            model: choice.model,
            effort: choice.effort,
          });
          // No session id on the plan means the engine would not take one —
          // Codex has no `--session-id` — so the slot is a tab handle: the
          // placeholder row keeps the tab reachable, but nothing is keyed to it
          // as a session, because there is no session by that name and never
          // will be.
          openTab(basename(cwd), cwd, plan.command, plan.session_id ?? crypto.randomUUID(), {
            sessionId: plan.session_id ?? undefined,
            fresh: true,
            agentId: plan.agent_id,
            // An agent we could not name has to be identified after the fact.
            // Snapshot what already exists so adoption can tell its session
            // from one that was open before this tab did anything.
            adopt: plan.session_id
              ? undefined
              : {
                  agentId: plan.agent_id,
                  since: Date.now(),
                  known: sessionsRef.current
                    .filter((s) => s.project_path === cwd)
                    .map((s) => s.id),
                },
          });
        } catch (e) {
          setNotice(`Couldn't start ${choice.agentId}: ${e}`);
        }
        return;
      }
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
      setNotice("Nothing to start yet — add a model under Settings → Model access, or install claude or codex.");
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
          <UsageBars bars={usage} />
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
          <AlertBell alerts={alerts} onGo={(key) => setActiveTab(key)} />
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
                liveSessions={liveShown}
                attentionSlots={new Set(
                  tabs.filter((t) => attention.has(t.key)).map((t) => t.slotId),
                )}
                attentionText={new Map(
                  tabs.flatMap((t) => {
                    const m = notices.get(t.key);
                    return m ? [[t.slotId, m] as [string, string]] : [];
                  }),
                )}
                progressSlots={new Map(
                  tabs.flatMap((t) => {
                    const p = progress.get(t.key);
                    return p ? [[t.slotId, p] as [string, TermProgress]] : [];
                  }),
                )}
                activeSlot={activeTabObj?.slotId ?? null}
                rekey={rekey}
                capsOf={capsOf}
                opts={opts}
                onOptsChange={setOpts}
                onSelect={selectSession}
                onResume={resumeSession}
                onFork={(s) =>
                  forkSession(s).catch((e) =>
                    setNotice(`Couldn't fork "${s.title}": ${e}`),
                  )
                }
                onClear={clearSession}
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
                onNotify={noteNotify}
                onProgress={noteProgress}
                autoFocus
                fontSize={termFont}
                fontFamily={xtermFont}
                lineHeight={settings.termLineHeight}
                fontWeight={settings.termFontWeight}
                renderer={settings.termRenderer}
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
                canResume={capsOf(previewSession.agent).resume}
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
              would be typed into its "describe a task" box instead of run.
              Every pill that speaks Claude Code's vocabulary is now gated on
              the active tab's engine declaring `panels`: `/model`, `/effort`
              and `/rewind` are claude's commands, and Tasks/Artifacts/Agents
              read a claude transcript. Sent into a Codex or `aiterm chat` tab
              they were text typed at a prompt that does not understand them.
              The pills that are about the tab rather than the engine — plan
              usage, the repo — stay, because they are true whatever is running.
              `onSetPermMode` goes with the TUI: it works by reading the status
              line and sending shift+tab. */}
          {showComposer && <Composer
            sessionId={activeCaps.panels ? activeSessionId : null}
            projectRoot={activeProject}
            usage={usage}
            usageAsOf={usageFresh ? null : usageAt || null}
            onCommand={activeTab === null || agentsView || !activeCaps.panels
              ? undefined
              : (text) => handles.current.get(activeTab)?.sendComposed(text)}
            onDismiss={activeTab === null ? undefined : () =>
              handles.current.get(activeTab)?.focus()}
            hasPendingInput={activeTab === null ? undefined : () =>
              handles.current.get(activeTab)?.pendingInput() ?? false}
            onOpenModelPicker={activeTab === null || agentsView || !activeCaps.panels
              ? undefined : openModelPicker}
            onOpenRewind={activeTab === null || agentsView || !activeCaps.panels
              ? undefined : openRewind}
            permMode={permMode}
            onSetPermMode={activeTab === null || agentsView || !activeDrivesTui
              ? undefined : setPermissionMode}
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
                    {/* The panel's prop doc has always said "if it's a claude
                        tab"; this is that assumption enforced instead of
                        assumed. It reads a claude transcript for tasks,
                        artifacts and subagent runs, so an engine that declares
                        no `panels` hands it no session and it says so — the
                        same empty state as a shell tab, which is the truth. */}
                    <AgentPanel sessionId={activeCaps.panels ? activeSessionId : null} />
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
          capsOf={capsOf}
          activeProject={activeProject}
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
