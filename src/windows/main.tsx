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
  const [settingUp, setSettingUp] = useState(false);
  const [setupNote, setSetupNote] = useState("");
  const [picking, setPicking] = useState(false);
  const resolve = useRef<((path: string | null) => void) | null>(null);
  useEffect(() => {
    if (settingUp) return;
    let alive = true;
    setError("");
    invoke<WslWorkspace>("workspace").then(ws => {
      if (alive) { setWorkspace(ws); setReady(ws); }
    }).catch(e => { if (alive) setError(String(e)); });
    return () => { alive = false; };
  }, [attempt, settingUp]);
  useEffect(() => {
    registerFolderPicker(() => new Promise(done => {
      resolve.current?.(null); resolve.current = done; setPicking(true);
    }));
    const unlisten = listen("workspace://disconnected", () => setError("The connection to Linux ended. Close and reopen aiterm to reconnect."));
    return () => { void unlisten.then(f => f()); resolve.current?.(null); };
  }, []);
  const pick = (path: string | null) => { resolve.current?.(path); resolve.current = null; setPicking(false); };
  const setup = async () => {
    if (settingUp) return;
    setSettingUp(true);
    setSetupNote("Follow the setup window. It will help you install WSL and create your Linux account.");
    try {
      const result = await invoke<"ready" | "restart_required" | "cancelled">("setup_wsl");
      setSetupNote(result === "restart_required"
        ? "Save your work and restart Windows. Open AITerm afterward and choose Set up WSL to continue."
        : result === "cancelled" ? "Setup was closed. You can continue whenever you are ready." : "Your Linux workspace is ready. Connecting…");
    } catch (e) { setSetupNote(String(e)); }
    finally { setSettingUp(false); }
  };
  return <>
    {workspace ? <App/> : <div className="workspace-overlay"><h1>aiterm</h1>
      {(error || settingUp || setupNote) ? <>
        <h2>Let’s get your Linux workspace ready</h2>
        <p>AITerm runs your terminals in Linux through WSL, built into Windows. Setup can install WSL and Ubuntu and help you create a Linux username and password.</p>
        <p>You may need administrator approval and a Windows restart. After restarting, open AITerm to continue.</p>
        <div className="wsl-setup-actions">
          <button className="btn primary" disabled={settingUp} onClick={() => void setup()}>{settingUp ? "Setup is open…" : "Set up WSL"}</button>
          <button className="btn" disabled={settingUp} onClick={() => { setSetupNote(""); retry(n => n + 1); }}>Try again</button>
        </div>
        {setupNote && <p role="status">{setupNote}</p>}
        {error && <details className="wsl-setup-details"><summary>Connection details</summary><p>{error}</p></details>}
      </> : <p role="status">Opening your Linux workspace…</p>}
    </div>}
    {workspace && error && <div className="wsl-connection-error" role="alert">{error}</div>}
    {workspace && picking && <FolderPicker initial={workspace.home} onPick={pick} onClose={() => pick(null)}/>}
  </>;
}
document.documentElement.classList.add("wsl-app");
createRoot(document.getElementById("root")!).render(<WindowsApp/>);
