import { invoke, Channel } from "@tauri-apps/api/core";

export interface Session {
  id: string;
  agent: string;
  title: string;
  /** True cwd — what a resumed tab spawns in. */
  project_path: string;
  /** Project this row groups under; differs from project_path only for
   *  sessions inside a Claude Code worktree, which group under their repo. */
  group_path: string;
  branch: string | null;
  /** Continues a conversation held in another transcript (a /fork child or a
   *  compact continuation) — its parent stays on disk, frozen at the fork. */
  forked: boolean;
  /** Ran as a background agent under the daemon (/fork or --bg). */
  background: boolean;
  /** Session this was forked from, from Claude Code job state — known as soon
   *  as /fork runs, before the fork's transcript has any messages. */
  fork_parent: string | null;
  last_active: number;
}

export interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
}

export interface FileStatus {
  path: string;
  status: string;
  staged: boolean;
}

export interface BranchInfo {
  name: string;
  is_head: boolean;
  upstream: string | null;
}

export interface CommitInfo {
  id: string;
  short_id: string;
  summary: string;
  author: string;
  time: number;
  refs: string[];
  parents: string[];
}

export interface RepoState {
  is_repo: boolean;
  branch: string | null;
  ahead: number;
  behind: number;
}

export interface SessionStatus {
  exists: boolean;
  permission_mode: string | null;
  mode: string | null;
}

export interface SessionTask {
  id: string;
  subject: string;
  status: "pending" | "in_progress" | "completed" | string;
  active_form: string | null;
  blocked_by: string[];
}

export interface Artifact {
  path: string;
  tool: string;
  at: string;
}

export interface AgentRun {
  id: string;
  agent_type: string;
  description: string;
  status: "running" | "done" | string;
  started_at: string | null;
  result: string | null;
}

export interface PreviewMsg {
  role: "user" | "assistant" | string;
  text: string;
  at: string | null;
}

export const listSessions = () => invoke<Session[]>("list_sessions");
export const sessionPreview = (sessionId: string) =>
  invoke<PreviewMsg[]>("session_preview", { sessionId });
export const sessionDelete = (sessionId: string) =>
  invoke<void>("session_delete", { sessionId });

export interface TrashedSession {
  id: string;
  title: string;
  project_path: string;
  deleted_at: number;
}

export const trashList = () => invoke<TrashedSession[]>("trash_list");
export const trashRestore = (sessionId: string) =>
  invoke<void>("trash_restore", { sessionId });
export const trashDelete = (sessionId: string) =>
  invoke<void>("trash_delete", { sessionId });
export const trashEmpty = () => invoke<void>("trash_empty");
export const sessionTasks = (sessionId: string) =>
  invoke<SessionTask[]>("session_tasks", { sessionId });
export const sessionArtifacts = (sessionId: string) =>
  invoke<Artifact[]>("session_artifacts", { sessionId });
export const sessionAgents = (sessionId: string) =>
  invoke<AgentRun[]>("session_agents", { sessionId });
export const runningSessionIds = () =>
  invoke<string[]>("running_session_ids");
// Session UUIDs held by the Claude Code daemon as background agents (incl.
// prompt-less /fork stubs). These can't be resumed as-is — reach them via
// the agent view (`claude agents`).
export const bgAgentSessionIds = () =>
  invoke<string[]>("bg_agent_session_ids");
// Sessions aiterm can't reliably stop — daemon-held with no pid, or background
// agents whose roster pid may be a helper rather than the conversation. The
// resume path asks before it closes anything, so a stop it can't win doesn't
// cost a tab. See `unstoppable_session_ids`.
export const unstoppableSessionIds = () =>
  invoke<string[]>("unstoppable_session_ids");
/** How a conversation left the session id a tab is pinned to.
 *  - `background`: the agents view moved it to the daemon; the old id is a dead
 *    end that nothing will write again.
 *  - `cleared`: `/clear` started a fresh conversation in the same terminal; the
 *    old id is a finished conversation that stays resumable on its own. */
export type MoveKind = "background" | "cleared";
export interface SessionMove {
  id: string;
  kind: MoveKind;
}
// The session that took over this one's conversation, or null — the normal
// answer. A tab pinned to the old id shows live text over dead panels until it
// re-keys, and its live conversation sits unowned in the sidebar.
export const sessionMovedTo = (sessionId: string) =>
  invoke<SessionMove | null>("session_moved_to", { sessionId });
/** A session start one of our claudes reported through its SessionStart hook,
 *  already resolved to the pty it happened in. `source` is claude's own word
 *  for why: "startup", "resume", "clear", "compact". */
export interface SessionEvent {
  ptyId: number;
  sessionId: string;
  source: string;
}
// Collect (and consume) the hook reports since last asked. The exact
// counterpart to the sessionMovedTo heuristic: no inference, just what each
// claude process said about itself, tied to the pty aiterm ran it in.
export const drainSessionEvents = () =>
  invoke<SessionEvent[]>("drain_session_events");
/** Whether verbose trace capture is on (Settings → Diagnostics). */
export const traceStatus = () => invoke<boolean>("trace_status");
/** Toggle verbose trace capture. On: returns the trace.log path (truncated
 *  fresh); off: returns null. Works in release builds — the filter is
 *  runtime-reloadable. */
export const traceSet = (on: boolean) =>
  invoke<string | null>("trace_set", { on });
// Journal-visible logging from the webview. Release builds drop the console,
// so errors the UI catches quietly would otherwise vanish — send the ones
// worth keeping to stderr, where journalctl already collects them.
export const uiLog = (msg: string) =>
  invoke<void>("ui_log", { msg }).catch(() => {});
// Every session the daemon currently holds, background AND interactive — i.e.
// "is this session alive right now". Distinct from "aiterm has a tab open for
// it": after a background-mode resume the tab holds the parent id while the
// conversation runs under a new one, so the two point at different rows.
export const liveSessionIds = () =>
  invoke<string[]>("live_session_ids");
// Stop a running session (SIGTERM its process tree, SIGKILL what survives) so
// `claude --resume` will take it. Resolves only once it's actually dead;
// rejects if it can't be stopped. Same move as quitting claude in a shell
// before resuming it.
export const stopSession = (sessionId: string) =>
  invoke<void>("stop_session", { sessionId });
// Resolve a pinned session id to one `claude --resume` can open now (follows
// the fork family). null → the original was cleared/superseded and nothing
// resumable survives.
export const resolveResumableId = (sessionId: string) =>
  invoke<string | null>("resolve_resumable_id", { sessionId });
// Branch a session on disk: copies its transcript under a fresh id and returns
// that id. Starts nothing — the branch appears as an inactive row, and the
// session you forked from keeps running untouched. Rejects if the transcript
// is gone or was cleared.
export const sessionFork = (sessionId: string) =>
  invoke<string>("session_fork", { sessionId });
// Write the conversation a `/fork` only promised: copies the parent's history
// up to the fork boundary under this session's id. Rejects unless the session
// really is an empty /fork stub with its parent still on disk.
export const materializeFork = (sessionId: string) =>
  invoke<void>("materialize_fork", { sessionId });
// The permission mode Claude Code's own config asks for in this directory.
// null when nothing is configured (or it names a mode the CLI would reject).
export const claudePermissionMode = (projectPath: string) =>
  invoke<string | null>("claude_permission_mode", { projectPath });
// Claude Code's global default model for new sessions (~/.claude/settings.json).
export const claudeModelDefault = () =>
  invoke<string | null>("claude_model_default");
// Put that global default back after a `/model` command changed it. Typing
// `/model <name>` retargets the running session AND rewrites the global
// default; the pill is a per-session control, so it undoes the second half.
export const restoreClaudeModelDefault = (previous: string | null) =>
  invoke<boolean>("restore_claude_model_default", { previous });
export interface ModelChoice {
  /** Full id as recorded, e.g. "claude-opus-5". null before the first reply. */
  model: string | null;
  effort: string | null;
  /** Timestamp of the record these came from — tells a pending request from a
   *  settled fact. Unchanged means no turn has run since you clicked. */
  at: string | null;
  /** Context-window fill after the last main-chain reply, in tokens. */
  context_tokens: number | null;
}
// What the session last actually ran with, read from its transcript — so it
// stays right whether the pill, a typed /model, or a launch flag changed it.
export const sessionModel = (sessionId: string) =>
  invoke<ModelChoice>("session_model", { sessionId });
export interface UsageBar {
  kind: string;
  label: string;
  percent: number;
  severity: string;
  resets_at: string;
}
export const usageLimits = () => invoke<UsageBar[]>("usage_limits");

export interface FontFamily {
  name: string;
  /** Fixed-pitch per fontconfig — the set worth offering for a terminal. */
  mono: boolean;
}
export interface FontPackage {
  name: string;
  package: string;
  note: string;
  installed: boolean;
}
// Every installed font family, straight from fontconfig. Replaces the old
// canvas width-probe, which could only find fonts we had thought to name.
export const listFonts = () => invoke<FontFamily[]>("list_fonts");
// Coding fonts installable from the distro repos, each flagged with whether
// it is already present.
export const fontPackages = () => invoke<FontPackage[]>("font_packages");
// Install one of those packages. Only names from `font_packages` are accepted;
// the backend refuses anything else rather than passing it to dnf.
export const installFontPackage = (pkg: string) =>
  invoke<string>("install_font_package", { package: pkg });
// Copy font files into ~/.local/share/fonts — no privileges, no network.
// Returns how many were installed.
export const installFontFiles = (paths: string[]) =>
  invoke<number>("install_font_files", { paths });
export const sessionStatus = (sessionId: string) =>
  invoke<SessionStatus>("session_status", { sessionId });
export interface ProjectInfo {
  name: string;
  path: string;
  is_git: boolean;
  last_modified: number;
}

export const watchProject = (path: string) => invoke<void>("watch_project", { path });
export const listDir = (path: string) => invoke<DirEntry[]>("list_dir", { path });
export const openPath = (path: string) => invoke<void>("open_path", { path });
export const listProjects = () => invoke<ProjectInfo[]>("list_projects");
export const searchSessions = (query: string) =>
  invoke<Session[]>("search_sessions", { query });
export const reindexSessions = () =>
  invoke<{ indexed: number; total: number }>("reindex_sessions");

// PTY output streams over a binary Channel (raw bytes → ArrayBuffer), not the
// JSON event bus. Caller passes a Channel whose onmessage receives each chunk.
export const ptySpawn = (
  cwd: string | null, command: string | null, cols: number, rows: number,
  onOutput: Channel<ArrayBuffer>,
) => invoke<number>("pty_spawn", { cwd, command, cols, rows, onOutput });
export const ptyWrite = (id: number, data: string) => invoke<void>("pty_write", { id, data });
export const ptyResize = (id: number, cols: number, rows: number) =>
  invoke<void>("pty_resize", { id, cols, rows });
export const ptyKill = (id: number) => invoke<void>("pty_kill", { id });

export const gitRepoState = (path: string) => invoke<RepoState>("git_repo_state", { path });
export const gitStatus = (path: string) => invoke<FileStatus[]>("git_status", { path });
export const gitBranches = (path: string) => invoke<BranchInfo[]>("git_branches", { path });
export const gitLog = (path: string, limit: number) =>
  invoke<CommitInfo[]>("git_log", { path, limit });
export interface TreeEntry {
  name: string;
  is_dir: boolean;
}

export const gitBranchFiles = (path: string, branch: string, subpath: string) =>
  invoke<TreeEntry[]>("git_branch_files", { path, branch, subpath });
export const gitBranchLog = (path: string, branch: string, limit: number) =>
  invoke<CommitInfo[]>("git_branch_log", { path, branch, limit });
export const gitDiffFile = (path: string, file: string) =>
  invoke<string>("git_diff_file", { path, file });
export const gitCommitDiff = (path: string, commitId: string) =>
  invoke<string>("git_commit_diff", { path, commitId });

export function homeAbbrev(p: string): string {
  return p.replace(/^\/home\/[^/]+/, "~");
}

export function relTime(ms: number): string {
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

/** What aiterm found for one agent on this machine — see `agents.rs`. */
export interface AgentDetection {
  id: string;
  display_name: string;
  /** Usable here. For a CLI agent, its binary is on PATH. */
  available: boolean;
  /** First line of `--version`, when it answered. Absent does not imply
   *  unavailable — some tools just don't report one. */
  version: string | null;
  path: string | null;
}

/** Every agent aiterm knows about, present or not. Spawns at most one process
 *  per installed agent, so call it when the answer is wanted (opening
 *  settings) rather than on a timer. */
export const detectAgents = () => invoke<AgentDetection[]>("detect_agents");

/** A configured API provider, as the UI sees it. The key never crosses this
 *  boundary — there is no command that returns it. */
export interface ProviderView {
  id: string;
  name: string;
  base_url: string;
  has_key: boolean;
  /** Last four characters, for telling two keys apart. Empty when there is no
   *  key or it is too short to redact meaningfully. */
  key_hint: string;
}

export const providersList = () => invoke<ProviderView[]>("providers_list");
/** Empty `apiKey` on an existing provider keeps the stored one. */
export const providerSave = (
  id: string | null, name: string, baseUrl: string, apiKey: string,
) => invoke<ProviderView[]>("provider_save", { id, name, baseUrl, apiKey });
export const providerDelete = (id: string) =>
  invoke<ProviderView[]>("provider_delete", { id });
/** Ask the provider for its model list — proves the key and URL actually work. */
export const providerModels = (id: string) =>
  invoke<string[]>("provider_models", { id });

/** A model a backend can start on, with the effort levels *that model* takes. */
export interface ModelOption {
  id: string;
  display_name: string;
  efforts: string[];
  default_effort: string | null;
}

/** An agent that is actually installed, and what it can be started as. */
export interface AgentChoice {
  id: string;
  display_name: string;
  models: ModelOption[];
  /** Whether aiterm can pre-mint the session id. Where false, the id it
   *  generates is a tab handle only and no panel should be keyed to it. */
  mints_session_id: boolean;
}

export const agentChoices = () => invoke<AgentChoice[]>("agent_choices");

/** The shell command that starts a session. Built in Rust so command-line
 *  syntax — the one thing that is certainly per-agent — stays with the agent. */
export const agentLaunchCommand = (
  agentId: string,
  spec: { model?: string | null; effort?: string | null; sessionId?: string | null },
) => invoke<string>("agent_launch_command", { agentId, spec });

/** The id of the session an agent just started in `cwd`, once it exists.
 *
 *  For an agent with no `--session-id` the id cannot be minted up front, so
 *  the tab starts with none and finds out by watching: the session that turns
 *  up in the directory we launched in, that was not there before, is ours.
 *  `known` is what the sidebar already listed, so adoption never steals a
 *  conversation that was already open. */
export const adoptAgentSession = (
  agentId: string,
  cwd: string,
  sinceMs: number,
  known: string[],
) => invoke<string | null>("adopt_agent_session", { agentId, cwd, sinceMs, known });

/** Where aiterm writes its diagnostics. */
export const diagLogPath = () => invoke<string | null>("diag_log_path");

/** The tail of that log, for pasting into a bug report. */
export const diagLogTail = (lines: number) => invoke<string>("diag_log_tail", { lines });

/** Build, desktop and which agents aiterm can see — the first questions of any
 *  "it is behaving oddly" conversation, answered without a scavenger hunt. */
export const diagEnvironment = () => invoke<[string, string][]>("diag_environment");
