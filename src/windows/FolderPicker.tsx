import { useEffect, useState } from "react";
import { ArrowLeft, Folder, X } from "lucide-react";
import Icon from "../components/Icon";
import { listDir, type DirEntry } from "../ipc";
export default function FolderPicker({ initial, onPick, onClose }: { initial: string; onPick: (path: string) => void; onClose: () => void }) {
  const [path, setPath] = useState(initial);
  const [draft, setDraft] = useState(initial);
  const [dirs, setDirs] = useState<DirEntry[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(true);
  useEffect(() => {
    let alive = true; setBusy(true); setError(""); setDraft(path);
    listDir(path).then(entries => { if (alive) setDirs(entries.filter(e => e.is_dir)); }).catch(e => { if (alive) setError(String(e)); }).finally(() => { if (alive) setBusy(false); });
    return () => { alive = false; };
  }, [path]);
  return <div className="modal-backdrop" onMouseDown={e => { if (e.target === e.currentTarget) onClose(); }}>
    <section className="wsl-dialog" role="dialog" aria-modal="true" aria-label="Choose a Linux folder">
      <div className="panel-header"><span>CHOOSE A FOLDER IN LINUX</span><button className="icon-btn" title="Close" onClick={onClose}><Icon of={X}/></button></div>
      <form className="panel-toolbar" onSubmit={e => { e.preventDefault(); if (draft.startsWith("/")) setPath(draft); else setError("Enter an absolute Linux path, starting with /."); }}>
        <button type="button" className="icon-btn" title="Parent folder" disabled={path === "/"} onClick={() => setPath(path.replace(/\/+$/, "").split("/").slice(0, -1).join("/") || "/")}><Icon of={ArrowLeft}/></button>
        <input className="wsl-path" aria-label="Linux folder path" value={draft} onChange={e => setDraft(e.target.value)}/><button className="btn" type="submit">Go</button>
      </form>
      <div className="wsl-folder-list">{busy ? <div className="empty-note">Loading folders…</div> : error ? <div className="empty-note" role="alert">{error}</div> : dirs.map(d => <button className="tree-row wsl-folder-row" key={d.path} onClick={() => setPath(d.path)}><Icon of={Folder}/><span>{d.name}</span></button>)}</div>
      <div className="wsl-dialog-actions"><button className="btn" onClick={onClose}>Cancel</button><button className="btn primary" disabled={busy || !!error} onClick={() => onPick(path)}>Use this folder</button></div>
    </section>
  </div>;
}
