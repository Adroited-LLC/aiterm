import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import SessionsPanel, { SessionDisplayOpts } from "./components/SessionsPanel";
import TerminalView, { TermHandle, TermTab } from "./components/TerminalView";
import FileExplorer from "./components/FileExplorer";
import GitPanel from "./components/GitPanel";
import Composer from "./components/Composer";
import TuiModelPicker from "./components/TuiModelPicker";
import TuiPermission from "./components/TuiPermission";
import TuiRewind from "./components/TuiRewind";
import {
  Detected, PermissionMode, detect, detectPermissionMode,
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
  ProjectInfo, Session,
  TrashedSession,
  UsageBar,
  listProjects, listSessions, materializeFork,
  reindexSessions, sessionFork, usageLimits,
  resolveResumableId, liveSessionIds, stopSession,
  sessionDelete, trashDelete, trashEmpty, trashList, trashRestore,
  watchProject,
} from "./ipc";
import "./App.css";

const OPTS_KEY = "aiterm.sessionOpts";
const SIZES_KEY = "aiterm.panelSizes";
const FONT_KEY = "aiterm.fontScale";
const PANELS_KEY = "aiterm.panelToggles";
const USAGE_KEY = "aiterm.usageCache";

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
        found.kind === "model-picker" ? "model"
        : found.kind === "rewind-picker" || found.kind === "rewind-confirm" ? "rewind"
        : null;
      if (needs && armed.current?.what !== needs) return;
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
  // hasTab/isRunning split. Costs one `claude agents --json` (~200ms) per tick,
  // so it runs on its own slow timer instead of on every filesystem event.
  const [liveSessions, setLiveSessions] = useState<Set<string>>(new Set());
  useEffect(() => {
    let stop = false;
    const tick = () =>
      liveSessionIds()
        .then((ids) => { if (!stop) setLiveSessions(new Set(ids)); })
        .catch(() => { /* keep the last known set rather than blanking dots */ });
    tick();
    const t = setInterval(tick, 6000);
    return () => { stop = true; clearInterval(t); };
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
    (title: string, cwd: string | null, command: string | null, slotId: string, sessionId?: string) => {
      setPreviewSession(null);
      setTabs((t) => {
        const existing = t.find((x) => x.slotId === slotId);
        if (existing) {
          setActiveTab(existing.key);
          return t;
        }
        const key = nextKey.current++;
        setActiveTab(key);
        return [...t, { key, title, cwd, command, sessionId, slotId }];
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
  // The composer's status line is gone, and with it three pollers that existed
  // only to feed it: a 1s "working" pulse, a 5s `session_status` call, and a
  // `git_repo_state` call per project change. Claude's own footer already says
  // all three things. Removed rather than left running for nobody to read.

  const closeTab = useCallback((key: number) => {
    setTabs((t) => {
      const next = t.filter((x) => x.key !== key);
      setActiveTab((cur) => (cur === key ? (next[next.length - 1]?.key ?? null) : cur));
      return next;
    });
  }, []);

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
                onExit={closeTab}
                onRegister={registerHandle}
                onActivity={noteActivity}
                onAttention={noteAttention}
                autoFocus
                fontSize={termFont}
                fontFamily={xtermFont}
                theme={xtermTheme}
              />
            ))}
            {tabs.length === 0 && !previewSession && (
              <div className="empty-note big">Pick a session on the left — ▶ resumes claude, ＋ opens a shell</div>
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
              model/effort when there is a live session to run them in. */}
          {showComposer && <Composer
            sessionId={activeSessionId}
            projectRoot={activeProject}
            usage={usage}
            usageAsOf={usageFresh ? null : usageAt || null}
            onCommand={activeTab === null ? undefined : (text) =>
              handles.current.get(activeTab)?.sendComposed(text)}
            onDismiss={activeTab === null ? undefined : () =>
              handles.current.get(activeTab)?.focus()}
            hasPendingInput={activeTab === null ? undefined : () =>
              handles.current.get(activeTab)?.pendingInput() ?? false}
            onOpenModelPicker={activeTab === null ? undefined : openModelPicker}
            onOpenRewind={activeTab === null ? undefined : openRewind}
            permMode={permMode}
            onSetPermMode={activeTab === null ? undefined : setPermissionMode}
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
