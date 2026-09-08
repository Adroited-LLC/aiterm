import { useEffect, useState } from "react";
import Row from "./SettingsRow";
import SettingsSwitch from "./SettingsSwitch";
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
    <button className="set-done" disabled={busy} onClick={() => { setBusy(true); setError(""); void invoke("app_update_install", { version: update.version }).then(() => setUpdate(null)).catch(e => setError(String(e))).finally(() => setBusy(false)); }}>{busy ? "Downloading and verifying…" : "Download and install"}</button>
    <button className="act-btn" disabled={busy} onClick={() => setUpdate(null)}>Later</button>
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
  return <>
    <section className="sgroup update-access">
      <div className="update-summary">
        <div>
          <h3 className="update-version">{status ? `AITerm ${status.currentVersion}` : "AITerm updates"}</h3>
          <div className="set-hint">{status?.connected ? "Connected to private updates" : "Connect once to receive private updates on this device."}</div>
        </div>
        <button className="set-recheck" disabled={busy} onClick={() => run(async () => {
          const s = await check();
          setMessage(!s.connected ? "Connect your update access first." : s.available ? `Version ${s.available.version} is available.` : "You’re up to date.");
          if (s.available) window.dispatchEvent(new Event(EVENT));
        })}>Check for updates</button>
      </div>
      {status?.available && <div className="set-notice">Version {status.available.version} is available.</div>}
    </section>
    <section className="sgroup">
      <div className="sgroup-title">Update access</div>
      {status?.connected ? (
        <Row label="Updates enabled" desc="Your invite is saved privately on this computer.">
          <button className="set-recheck" disabled={busy} onClick={() => run(async () => {
            await invoke("app_update_connect", { token: "" }); await check(); setMessage("Disconnected.");
          })}>Disconnect</button>
        </Row>
      ) : (
        <div className="update-access">
          <label className="prov-field">Invite code
            <input className="set-input" type="password" autoComplete="off" value={token} onChange={e => setToken(e.target.value)} placeholder="Enter your personal invite code" />
          </label>
          <div className="set-hint">Use the code provided by your AITerm administrator.</div>
          <div className="update-actions">
            <button className="set-done" disabled={busy || !token.trim()} onClick={() => run(async () => {
              await invoke("app_update_connect", { token }); setToken(""); await check(); setMessage("Connected to private updates.");
            })}>Connect</button>
          </div>
        </div>
      )}
    </section>
    <section className="sgroup">
      <div className="sgroup-title">Installation</div>
      <Row label="Automatically install updates" desc="Windows installs when you close AITerm. Linux updates without restarting your sessions; an administrator prompt may appear.">
        <SettingsSwitch checked={automatic} disabled={busy} label="Automatically install updates" onChange={value => run(async () => {
          await invoke("app_update_settings", { automatic: value }); setAutomatic(value); window.dispatchEvent(new Event(EVENT));
        })} />
      </Row>
    </section>
    {(busy || message) && <p className="set-notice update-status" role="status">{busy ? "Checking…" : message}</p>}
  </>;
}
