import { invoke } from "@tauri-apps/api/core";

export interface Session {
  id: string;
  agent: string;
  title: string;
  project_path: string;
  branch: string | null;
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

export const listSessions = () => invoke<Session[]>("list_sessions");
export const sessionTasks = (sessionId: string) =>
  invoke<SessionTask[]>("session_tasks", { sessionId });
export const sessionArtifacts = (sessionId: string) =>
  invoke<Artifact[]>("session_artifacts", { sessionId });
export const sessionStatus = (sessionId: string) =>
  invoke<SessionStatus>("session_status", { sessionId });
export interface ProjectInfo {
  name: string;
  path: string;
  is_git: boolean;
  last_modified: number;
}

export const listDir = (path: string) => invoke<DirEntry[]>("list_dir", { path });
export const openPath = (path: string) => invoke<void>("open_path", { path });
export const listProjects = () => invoke<ProjectInfo[]>("list_projects");
export const searchSessions = (query: string) =>
  invoke<Session[]>("search_sessions", { query });
export const reindexSessions = () =>
  invoke<{ indexed: number; total: number }>("reindex_sessions");

export const ptySpawn = (cwd: string | null, command: string | null, cols: number, rows: number) =>
  invoke<number>("pty_spawn", { cwd, command, cols, rows });
export const ptyWrite = (id: number, data: string) => invoke<void>("pty_write", { id, data });
export const ptyResize = (id: number, cols: number, rows: number) =>
  invoke<void>("pty_resize", { id, cols, rows });
export const ptyKill = (id: number) => invoke<void>("pty_kill", { id });

export const gitRepoState = (path: string) => invoke<RepoState>("git_repo_state", { path });
export const gitStatus = (path: string) => invoke<FileStatus[]>("git_status", { path });
export const gitBranches = (path: string) => invoke<BranchInfo[]>("git_branches", { path });
export const gitLog = (path: string, limit: number) =>
  invoke<CommitInfo[]>("git_log", { path, limit });
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
