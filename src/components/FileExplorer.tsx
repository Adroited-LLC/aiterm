import { useCallback, useEffect, useRef, useState } from "react";
import { DirEntry, listDir, openPath } from "../ipc";

interface Node extends DirEntry {
  depth: number;
  expanded: boolean;
  children: Node[] | null;
}

function Chevron({ open }: { open: boolean }) {
  return <span className={"chevron" + (open ? " open" : "")}>›</span>;
}

function FolderIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="2" className="tree-icon folder">
      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z" />
    </svg>
  );
}

function FileIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="2" className="tree-icon">
      <path d="M14 3v5h5M6 3h8l5 5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z" />
    </svg>
  );
}

export default function FileExplorer({
  root, refreshKey = 0,
}: { root: string | null; refreshKey?: number }) {
  const [tree, setTree] = useState<Node[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [openErr, setOpenErr] = useState<string | null>(null);
  const treeRef = useRef<Node[]>([]);
  useEffect(() => {
    treeRef.current = tree;
  }, [tree]);

  const openFile = (path: string) => {
    setOpenErr(null);
    openPath(path).catch((e) => {
      setOpenErr(String(e));
      setTimeout(() => setOpenErr(null), 6000);
    });
  };

  const load = useCallback(async (path: string, depth: number): Promise<Node[]> => {
    const entries = await listDir(path).catch(() => [] as DirEntry[]);
    return entries.map((e) => ({ ...e, depth, expanded: false, children: null }));
  }, []);

  useEffect(() => {
    if (!root) {
      setTree([]);
      setError(null);
      return;
    }
    listDir(root)
      .then((entries) => {
        setError(null);
        setTree(entries.map((e) => ({ ...e, depth: 0, expanded: false, children: null })));
      })
      .catch((e) => {
        setTree([]);
        setError(String(e));
      });
  }, [root, load]);

  // File-watcher refresh: re-list the root and every expanded dir, keeping
  // the expansion state intact.
  useEffect(() => {
    if (!refreshKey || !root) return;
    const expanded = new Set<string>();
    const collect = (nodes: Node[]) =>
      nodes.forEach((n) => {
        if (n.expanded) {
          expanded.add(n.path);
          if (n.children) collect(n.children);
        }
      });
    collect(treeRef.current);
    const build = async (path: string, depth: number): Promise<Node[]> => {
      const entries = await load(path, depth);
      return Promise.all(
        entries.map(async (n) =>
          n.is_dir && expanded.has(n.path)
            ? { ...n, expanded: true, children: await build(n.path, depth + 1) }
            : n,
        ),
      );
    };
    let stale = false;
    build(root, 0).then((t) => {
      if (!stale) setTree(t);
    });
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey]);

  const toggleNode = async (target: Node) => {
    if (!target.is_dir) return;
    const children =
      target.children ?? (await load(target.path, target.depth + 1));
    const update = (nodes: Node[]): Node[] =>
      nodes.map((n) =>
        n.path === target.path
          ? { ...n, expanded: !n.expanded, children }
          : n.children
            ? { ...n, children: update(n.children) }
            : n,
      );
    setTree(update);
  };

  const renderNodes = (nodes: Node[]): React.ReactNode =>
    nodes.map((n) => (
      <div key={n.path}>
        <div
          className="tree-row"
          style={{ paddingLeft: 8 + n.depth * 14 }}
          onClick={() => toggleNode(n)}
          onDoubleClick={() => !n.is_dir && openFile(n.path)}
          title={n.path}
        >
          {n.is_dir ? <Chevron open={n.expanded} /> : <span className="chevron-spacer" />}
          {n.is_dir ? <FolderIcon /> : <FileIcon />}
          <span className="tree-name">{n.name}</span>
        </div>
        {n.expanded && n.children && renderNodes(n.children)}
      </div>
    ));

  if (!root) return <div className="empty-note">Select a project to browse files</div>;
  if (error) return <div className="empty-note">Can't read {root}: {error}</div>;
  if (tree.length === 0) return <div className="empty-note">Empty folder</div>;
  return (
    <>
      {openErr && <div className="open-error">{openErr}</div>}
      <div className="file-tree">{renderNodes(tree)}</div>
    </>
  );
}
