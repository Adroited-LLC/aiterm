import { useCallback, useEffect, useState } from "react";
import {
  BranchInfo, CommitInfo, FileStatus, RepoState,
  gitBranches, gitCommitDiff, gitDiffFile, gitLog, gitRepoState, gitStatus, relTime,
} from "../ipc";

type Tab = "changes" | "branches" | "log";

function statusClass(code: string): string {
  if (code === "??") return "st-untracked";
  if (code.includes("D")) return "st-deleted";
  if (code.includes("A")) return "st-added";
  return "st-modified";
}

function DiffView({ text, onClose, title }: { text: string; onClose: () => void; title: string }) {
  return (
    <div className="diff-view">
      <div className="diff-header">
        <span className="diff-title">{title}</span>
        <button className="icon-btn" onClick={onClose}>✕</button>
      </div>
      <pre className="diff-body">
        {text.split("\n").map((line, i) => {
          let cls = "";
          if (line.startsWith("+") && !line.startsWith("+++")) cls = "add";
          else if (line.startsWith("-") && !line.startsWith("---")) cls = "del";
          else if (line.startsWith("@@")) cls = "hunk";
          else if (line.startsWith("diff ") || line.startsWith("+++") || line.startsWith("---")) cls = "meta";
          return (
            <div key={i} className={"diff-line " + cls}>{line || " "}</div>
          );
        })}
      </pre>
    </div>
  );
}

export default function GitPanel({ root, refreshKey }: { root: string | null; refreshKey: number }) {
  const [tab, setTab] = useState<Tab>("changes");
  const [state, setState] = useState<RepoState | null>(null);
  const [status, setStatus] = useState<FileStatus[]>([]);
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [log, setLog] = useState<CommitInfo[]>([]);
  const [diff, setDiff] = useState<{ title: string; text: string } | null>(null);

  const refresh = useCallback(async () => {
    setDiff(null);
    if (!root) {
      setState(null);
      return;
    }
    const st = await gitRepoState(root);
    setState(st);
    if (!st.is_repo) return;
    gitStatus(root).then(setStatus).catch(() => setStatus([]));
    gitBranches(root).then(setBranches).catch(() => setBranches([]));
    gitLog(root, 100).then(setLog).catch(() => setLog([]));
  }, [root]);

  useEffect(() => {
    refresh();
  }, [refresh, refreshKey]);

  if (!root) return <div className="empty-note">Select a project to see its repository</div>;
  if (state && !state.is_repo) {
    return (
      <div className="git-empty">
        <div className="git-empty-icon">⌀</div>
        <div className="git-empty-title">Cannot detect diffs for this folder</div>
        <div className="git-empty-sub">Diffs only work for git repositories.</div>
      </div>
    );
  }

  const showFileDiff = async (f: FileStatus) => {
    const text = await gitDiffFile(root, f.path).catch((e) => String(e));
    setDiff({ title: f.path, text });
  };
  const showCommitDiff = async (c: CommitInfo) => {
    const text = await gitCommitDiff(root, c.id).catch((e) => String(e));
    setDiff({ title: `${c.short_id} ${c.summary}`, text });
  };

  return (
    <div className="git-panel">
      <div className="git-head">
        <span className="branch-chip">⎇ {state?.branch ?? "—"}</span>
        {state && (state.ahead > 0 || state.behind > 0) && (
          <span className="sync-chip">↑{state.ahead} ↓{state.behind}</span>
        )}
        <div className="git-tabs">
          {(["changes", "branches", "log"] as Tab[]).map((t) => (
            <button
              key={t}
              className={"git-tab" + (tab === t ? " on" : "")}
              onClick={() => { setTab(t); setDiff(null); }}
            >
              {t === "changes" ? `Changes${status.length ? ` (${status.length})` : ""}` :
                t.charAt(0).toUpperCase() + t.slice(1)}
            </button>
          ))}
        </div>
      </div>
      <div className="git-body">
        {diff ? (
          <DiffView text={diff.text} title={diff.title} onClose={() => setDiff(null)} />
        ) : tab === "changes" ? (
          status.length === 0 ? (
            <div className="empty-note">Working tree clean</div>
          ) : (
            status.map((f) => (
              <div key={f.path} className="git-row" onClick={() => showFileDiff(f)} title={f.path}>
                <span className={"st-code " + statusClass(f.status)}>{f.status}</span>
                <span className="git-row-text">{f.path}</span>
              </div>
            ))
          )
        ) : tab === "branches" ? (
          branches.map((b) => (
            <div key={b.name} className={"git-row" + (b.is_head ? " head" : "")}>
              <span className="st-code">{b.is_head ? "●" : " "}</span>
              <span className="git-row-text">{b.name}</span>
              {b.upstream && <span className="upstream">{b.upstream}</span>}
            </div>
          ))
        ) : (
          log.map((c) => (
            <div key={c.id} className="git-row commit" onClick={() => showCommitDiff(c)}>
              <span className="commit-id">{c.short_id}</span>
              <span className="git-row-text">
                {c.refs.map((r) => (
                  <span key={r} className="ref-badge">{r}</span>
                ))}
                {c.summary}
              </span>
              <span className="commit-meta">{c.author} · {relTime(c.time * 1000)}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
