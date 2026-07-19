import { useCallback, useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { DirEntry, listDir } from "../ipc";

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

export default function FileExplorer({ root }: { root: string | null }) {
  const [tree, setTree] = useState<Node[]>([]);

  const load = useCallback(async (path: string, depth: number): Promise<Node[]> => {
    const entries = await listDir(path).catch(() => [] as DirEntry[]);
    return entries.map((e) => ({ ...e, depth, expanded: false, children: null }));
  }, []);

  useEffect(() => {
    if (!root) {
      setTree([]);
      return;
    }
    load(root, 0).then(setTree);
  }, [root, load]);

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
          onDoubleClick={() => !n.is_dir && openPath(n.path).catch(() => {})}
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
  return <div className="file-tree">{renderNodes(tree)}</div>;
}
