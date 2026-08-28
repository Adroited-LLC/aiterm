import { useCallback, useEffect, useMemo, useState } from "react";
import {
  BranchInfo, CommitInfo, FileStatus, RepoState, TreeEntry,
  gitBranches, gitBranchFiles, gitBranchLog, gitCommitDiff, gitDiffFile,
  gitLog, gitRepoState, gitStatus, relTime,
} from "../ipc";
import { computeGraph, laneColor } from "../graph";
import Icon from "./Icon";
import { ArrowDown, ArrowUp, CircleDot, GitBranch, X } from "lucide-react";

const COL = 13;
const ROW_H = 30;

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
        <button className="icon-btn" onClick={onClose}><Icon of={X} /></button>
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

interface BranchTreeNode {
  label: string;
  branch?: BranchInfo;
  children: BranchTreeNode[];
}

/** Group "feature/foo" style names into nested folders, GitLens-style. */
function buildBranchTree(branches: BranchInfo[]): BranchTreeNode[] {
  const root: BranchTreeNode[] = [];
  for (const b of branches) {
    const parts = b.name.split("/");
    let level = root;
    for (let i = 0; i < parts.length; i++) {
      if (i === parts.length - 1) {
        level.push({ label: parts[i], branch: b, children: [] });
      } else {
        let node = level.find((n) => n.label === parts[i] && !n.branch);
        if (!node) {
          node = { label: parts[i], children: [] };
          level.push(node);
        }
        level = node.children;
      }
    }
  }
  return root;
}

interface BFNode extends TreeEntry {
  sub: string;
  depth: number;
  expanded: boolean;
  children: BFNode[] | null;
}

function BranchesView({
  root, branches, onCommit,
}: {
  root: string;
  branches: BranchInfo[];
  onCommit: (c: CommitInfo) => void;
}) {
  const tree = useMemo(() => buildBranchTree(branches), [branches]);
  const [closedFolders, setClosedFolders] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<string | null>(null);
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  const [files, setFiles] = useState<BFNode[]>([]);

  useEffect(() => {
    setExpanded(null);
  }, [root]);

  const loadEntries = useCallback(
    async (branch: string, sub: string, depth: number): Promise<BFNode[]> => {
      const entries = await gitBranchFiles(root, branch, sub).catch(() => [] as TreeEntry[]);
      return entries.map((e) => ({
        ...e,
        sub: sub ? `${sub}/${e.name}` : e.name,
        depth,
        expanded: false,
        children: null,
      }));
    },
    [root],
  );

  const toggleBranch = async (name: string) => {
    if (expanded === name) {
      setExpanded(null);
      return;
    }
    setExpanded(name);
    setCommits([]);
    setFiles([]);
    gitBranchLog(root, name, 8).then(setCommits).catch(() => {});
    loadEntries(name, "", 0).then(setFiles);
  };

  const toggleFile = async (target: BFNode) => {
    if (!target.is_dir || !expanded) return;
    const children = target.children ?? (await loadEntries(expanded, target.sub, target.depth + 1));
    const update = (nodes: BFNode[]): BFNode[] =>
      nodes.map((n) =>
        n.sub === target.sub
          ? { ...n, expanded: !n.expanded, children }
          : n.children
            ? { ...n, children: update(n.children) }
            : n,
      );
    setFiles(update);
  };

  const renderFiles = (nodes: BFNode[]): React.ReactNode =>
    nodes.map((n) => (
      <div key={n.sub}>
        <div
          className="tree-row"
          style={{ paddingLeft: 20 + n.depth * 14 }}
          onClick={() => toggleFile(n)}
          title={n.sub}
        >
          {n.is_dir ? <span className={"chevron" + (n.expanded ? " open" : "")}>›</span>
            : <span className="chevron-spacer" />}
          <span className="tree-name">{n.name}</span>
        </div>
        {n.expanded && n.children && renderFiles(n.children)}
      </div>
    ));

  const renderNodes = (nodes: BranchTreeNode[], depth: number, prefix: string): React.ReactNode =>
    nodes.map((n) => {
      const key = prefix ? `${prefix}/${n.label}` : n.label;
      if (!n.branch) {
        const closed = closedFolders.has(key);
        return (
          <div key={key}>
            <div
              className="git-row branch-folder"
              style={{ paddingLeft: 8 + depth * 14 }}
              onClick={() =>
                setClosedFolders((s) => {
                  const next = new Set(s);
                  if (closed) next.delete(key);
                  else next.add(key);
                  return next;
                })}
            >
              <span className={"chevron" + (!closed ? " open" : "")}>›</span>
              <span className="git-row-text">{n.label}/</span>
            </div>
            {!closed && renderNodes(n.children, depth + 1, key)}
          </div>
        );
      }
      const b = n.branch;
      const open = expanded === b.name;
      return (
        <div key={key}>
          <div
            className={"git-row" + (b.is_head ? " head" : "")}
            style={{ paddingLeft: 8 + depth * 14 }}
            onClick={() => toggleBranch(b.name)}
          >
            <span className="st-code">{b.is_head ? <Icon of={CircleDot} size="sm" /> : <Icon of={GitBranch} size="sm" />}</span>
            <span className="git-row-text">{n.label}</span>
            {b.upstream && <span className="upstream">{b.upstream}</span>}
            <span className={"chevron" + (open ? " open" : "")}>›</span>
          </div>
          {open && (
            <div className="branch-detail">
              {commits.map((c) => (
                <div
                  key={c.id}
                  className="git-row branch-commit"
                  onClick={(e) => { e.stopPropagation(); onCommit(c); }}
                >
                  <span className="commit-id">{c.short_id}</span>
                  <span className="git-row-text">{c.summary}</span>
                  <span className="commit-meta">{relTime(c.time * 1000)}</span>
                </div>
              ))}
              <div className="branch-files-label">FILES</div>
              {renderFiles(files)}
            </div>
          )}
        </div>
      );
    });

  return <>{renderNodes(tree, 0, "")}</>;
}

export default function GitPanel({ root, refreshKey }: { root: string | null; refreshKey: number }) {
  const [tab, setTab] = useState<Tab>("changes");
  const [state, setState] = useState<RepoState | null>(null);
  const [status, setStatus] = useState<FileStatus[]>([]);
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [log, setLog] = useState<CommitInfo[]>([]);
  const [diff, setDiff] = useState<{ title: string; text: string } | null>(null);
  const graph = useMemo(() => computeGraph(log), [log]);
  const graphWidth = useMemo(
    () => (Math.max(0, ...graph.map((r) => r.maxLane)) + 1) * COL + 4,
    [graph],
  );

  // Switching projects closes any open diff; background refreshes (the
  // file watcher fires constantly while claude works) must NOT — they'd
  // yank the view away milliseconds after the user opened it.
  useEffect(() => {
    setDiff(null);
  }, [root]);

  const refresh = useCallback(async () => {
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
        <span className="branch-chip"><Icon of={GitBranch} size="sm" /> {state?.branch ?? "—"}</span>
        {state && (state.ahead > 0 || state.behind > 0) && (
          <span className="sync-chip"><Icon of={ArrowUp} size="sm" />{state.ahead} <Icon of={ArrowDown} size="sm" />{state.behind}</span>
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
          <BranchesView root={root} branches={branches} onCommit={showCommitDiff} />
        ) : (
          graph.map((row) => {
            const c = row.commit;
            const cx = (l: number) => l * COL + 7;
            return (
              <div
                key={c.id}
                className="git-row commit graph-row"
                style={{ height: ROW_H }}
                onClick={() => showCommitDiff(c)}
              >
                <svg width={graphWidth} height={ROW_H} className="graph-svg">
                  {row.inputs.map(([f, t], i) =>
                    f === t ? (
                      <line key={`i${i}`} x1={cx(f)} y1={0} x2={cx(f)} y2={ROW_H / 2}
                        stroke={laneColor(f)} strokeWidth="2" />
                    ) : (
                      <path key={`i${i}`}
                        d={`M ${cx(f)} 0 C ${cx(f)} ${ROW_H / 3}, ${cx(t)} ${ROW_H / 6}, ${cx(t)} ${ROW_H / 2}`}
                        stroke={laneColor(f)} strokeWidth="2" fill="none" />
                    ),
                  )}
                  {row.outputs.map(([f, t], i) =>
                    f === t ? (
                      <line key={`o${i}`} x1={cx(f)} y1={ROW_H / 2} x2={cx(f)} y2={ROW_H}
                        stroke={laneColor(f)} strokeWidth="2" />
                    ) : (
                      <path key={`o${i}`}
                        d={`M ${cx(f)} ${ROW_H / 2} C ${cx(f)} ${ROW_H * 0.83}, ${cx(t)} ${ROW_H * 0.66}, ${cx(t)} ${ROW_H}`}
                        stroke={laneColor(t)} strokeWidth="2" fill="none" />
                    ),
                  )}
                  <circle cx={cx(row.lane)} cy={ROW_H / 2} r="4"
                    fill={laneColor(row.lane)} stroke="var(--bg-panel)" strokeWidth="1.5" />
                </svg>
                <span className="commit-id">{c.short_id}</span>
                <span className="git-row-text">
                  {c.refs.map((r) => (
                    <span key={r} className="ref-badge">{r}</span>
                  ))}
                  {c.summary}
                </span>
                <span className="commit-meta">{c.author} · {relTime(c.time * 1000)}</span>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
