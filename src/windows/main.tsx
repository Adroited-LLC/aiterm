import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import App from "../App";
import { invoke, setWorkspace, type WslWorkspace } from "../platform";
import { registerFolderPicker } from "../platformDialog";
import FolderPicker from "./FolderPicker";
import "./windows.css";

function WindowsApp() {
  const [workspace, setReady] = useState<WslWorkspace>();
  const [error, setError] = useState("");
  const [attempt, retry] = useState(0);
  const [picking, setPicking] = useState(false);
  const resolve = useRef<((path: string | null) => void) | null>(null);
  useEffect(() => {
    let alive = true;
    setError("");
    invoke<WslWorkspace>("workspace").then(ws => {
      if (alive) { setWorkspace(ws); setReady(ws); }
    }).catch(e => { if (alive) setError(String(e)); });
    return () => { alive = false; };
  }, [attempt]);
  useEffect(() => {
    registerFolderPicker(() => new Promise(done => {
      resolve.current?.(null); resolve.current = done; setPicking(true);
    }));
    const unlisten = listen("workspace://disconnected", () => setError("The connection to Linux ended. Close and reopen aiterm to reconnect."));
    return () => { void unlisten.then(f => f()); resolve.current?.(null); };
  }, []);
  const pick = (path: string | null) => { resolve.current?.(path); resolve.current = null; setPicking(false); };
  return <>
    {workspace ? <App/> : <div className="workspace-overlay"><h1>aiterm</h1><p>{error || "Opening your Linux workspace…"}</p>{error && <><p>aiterm needs a working WSL distribution. Open Ubuntu from Start to finish its setup, then try again.</p><button className="set-action" onClick={() => retry(n => n + 1)}>Try again</button></>}</div>}
    {workspace && error && <div className="wsl-connection-error" role="alert">{error}</div>}
    {workspace && picking && <FolderPicker initial={workspace.home} onPick={pick} onClose={() => pick(null)}/>}
  </>;
}
document.documentElement.classList.add("wsl-app");
createRoot(document.getElementById("root")!).render(<WindowsApp/>);
