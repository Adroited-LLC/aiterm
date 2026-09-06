import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Status = { currentVersion: string; connected: boolean; available: { version: string; notes: string } | null };
const EVENT = "aiterm-update-check";
export function UpdateNotice() {
  const [update, setUpdate] = useState<Status["available"]>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [info, setInfo] = useState("");
  useEffect(() => {
    let active = true;
    let checking = false;
    const check = () => {
      if (checking) return;
      checking = true;
      void invoke<Status>("app_update_check").then(async s => {
        if (!active) return;
        setUpdate(s.available);
        if (s.available && await invoke<boolean>("app_update_settings")) {
          setBusy(true);
          try {
            const message = await invoke<string>("app_update_install", { version: s.available.version, automatic: true });
            if (active) { setUpdate(null); setInfo(message); }
          } catch (e) { if (active) setError(String(e)); }
          finally { if (active) setBusy(false); }
        }
      }).catch(() => {}).finally(() => { checking = false; });
    };
    const timeout = setTimeout(check, 15000);
    const timer = setInterval(check, 6 * 60 * 60 * 1000);
    window.addEventListener(EVENT, check);
    return () => { active = false; clearTimeout(timeout); clearInterval(timer); window.removeEventListener(EVENT, check); };
  }, []);
  if (!update) return info ? <div className="app-toast" role="status" onClick={() => setInfo("")}>{info}</div> : null;
  return <div className="app-toast" role="status" style={{ cursor: "default", maxWidth: 460 }}>
    <strong>AITerm {update.version} is available</strong>
    <p>Save your work before installing. The system installer will guide you.</p>
    {error && <p role="alert">{error}</p>}
    <button disabled={busy} onClick={() => { setBusy(true); setError(""); void invoke("app_update_install", { version: update.version }).then(() => setUpdate(null)).catch(e => setError(String(e))).finally(() => setBusy(false)); }}>{busy ? "Downloading and verifying…" : "Download and install"}</button>
    <button disabled={busy} onClick={() => setUpdate(null)}>Later</button>
  </div>;
}
export default function AppUpdates() {
  const [status, setStatus] = useState<Status | null>(null);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [automatic, setAutomatic] = useState(false);
  const check = async () => { const s = await invoke<Status>("app_update_check"); setStatus(s); return s; };
  const run = (action: () => Promise<void>) => { setBusy(true); setMessage(""); void action().catch(e => setMessage(String(e))).finally(() => setBusy(false)); };
  useEffect(() => { run(async () => { setAutomatic(await invoke<boolean>("app_update_settings")); await check(); }); }, []);
  return <section className="sgroup">
    <h3>App updates</h3>
    <p>{status ? `Installed version ${status.currentVersion}` : "Private updates for AITerm"}</p>
    <p>Use a personal invite code from your AITerm administrator. You only need to connect once on each device.</p>
    <label>Invite code <input type="password" autoComplete="off" value={token} onChange={e => setToken(e.target.value)} placeholder="aiterm_…" /></label>
    <button disabled={busy || !token.trim()} onClick={() => run(async () => { await invoke("app_update_connect", { token }); setToken(""); await check(); setMessage("Connected to private updates."); })}>Connect</button>
    <button disabled={busy} onClick={() => run(async () => { const s = await check(); setMessage(!s.connected ? "Connect your update access first." : s.available ? `Version ${s.available.version} is available.` : "You’re up to date."); if (s.available) window.dispatchEvent(new Event(EVENT)); })}>Check for updates</button>
    {status?.connected && <button disabled={busy} onClick={() => run(async () => { await invoke("app_update_connect", { token: "" }); await check(); setMessage("Disconnected."); })}>Disconnect</button>}
    <p role="status">{busy ? "Checking…" : message}</p>
    <label><input type="checkbox" checked={automatic} onChange={e => { const value = e.target.checked; run(async () => { await invoke("app_update_settings", { automatic: value }); setAutomatic(value); window.dispatchEvent(new Event(EVENT)); }); }} /> Automatically install updates</label>
    <p>Off by default. Windows installs when you close AITerm. Linux applies updates without restarting your sessions; an administrator prompt may appear.</p>
  </section>;
}
