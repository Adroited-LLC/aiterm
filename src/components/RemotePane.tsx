import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Row from "./SettingsRow";
import {
  PairPayload, RemoteStatus,
  remotePairPayload, remoteRotateToken, remoteSetEnabled, remoteSetName, remoteSetPort, remoteStatus,
} from "../ipc";

/** Settings → Remote: the phone side of aiterm. One switch, one QR, a port,
 *  and a way to forget every phone. Everything else is decided for you —
 *  the point of the phone app is that it needs no settings. */
export default function RemotePane() {
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [pair, setPair] = useState<PairPayload | null>(null);
  const [pairError, setPairError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [port, setPort] = useState("");
  const [portError, setPortError] = useState<string | null>(null);
  const [confirmForget, setConfirmForget] = useState(false);
  const [now, setNow] = useState(Date.now());

  const load = () => remoteStatus().then((s) => { setStatus(s); setName(s.name); setPort(String(s.port)); });
  useEffect(() => { load(); }, []);
  // Connections come and go on their own schedule; the router answers late.
  useEffect(() => {
    const un = listen("remote://clients", () => load());
    const t = setInterval(() => { setNow(Date.now()); load(); }, 5000);
    return () => { un.then((f) => f()); clearInterval(t); };
  }, []);

  const toggle = async (on: boolean) => {
    const s = await remoteSetEnabled(on);
    setStatus(s);
    if (!on) setPair(null);
  };
  const showQr = async () => {
    setPairError(null);
    try { setPair(await remotePairPayload()); }
    catch (e) { setPair(null); setPairError(`${e}`); }
  };
  const forget = async () => {
    setStatus(await remoteRotateToken());
    setPair(null);
    setConfirmForget(false);
  };
  const commitName = async () => {
    if (!status || name.trim() === status.name) return;
    setStatus(await remoteSetName(name));
    setPair(null);
  };
  const commitPort = async () => {
    if (!status) return;
    const p = Number(port);
    if (!Number.isInteger(p) || p < 1024 || p > 65535) { setPortError("Pick a port from 1024 to 65535"); return; }
    if (p === status.port) { setPortError(null); return; }
    try { setStatus(await remoteSetPort(p)); setPortError(null); setPair(null); }
    catch (e) { setPortError(`${e}`); }
  };
  const blurOnEnter = (e: React.KeyboardEvent<HTMLInputElement>) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); };

  if (!status) return null;
  const allAddresses = [...status.addresses, ...(status.public_address ? [status.public_address] : [])];
  return (
    <>
      <Row label="Remote access" desc={status.running
        ? `Listening on port ${status.port}. On this network the phone reaches this machine at ${status.addresses[0] ?? "no address found"}.`
        : "Off. Nothing listens until you turn this on."}>
        <label className="sw" aria-label="Remote access">
          <input type="checkbox" checked={status.enabled} onChange={(e) => toggle(e.target.checked)} />
          <span className="sw-track"><span className="sw-knob" /></span>
        </label>
      </Row>
      {status.error && (
        <Row label="Problem" desc={status.error} />
      )}
      <Row label="Connected now" desc={status.clients.length === 0
        ? (status.running ? "No phone is connected." : "—")
        : `${status.clients.length} ${status.clients.length === 1 ? "phone" : "phones"} holding a live connection.`} wide={status.clients.length > 0}>
        {status.clients.length > 0 && (
          <div className="remote-clients">
            {status.clients.map((c) => (
              <div key={c.id} className="remote-client">
                <strong>{c.device || "Unknown device"}</strong>
                <span className="dim"> · {c.os || "?"} · app {c.app || "?"} · from {c.address} · {sinceLabel(c.since, now)}</span>
              </div>
            ))}
          </div>
        )}
      </Row>
      {status.running && (
        <Row label="From outside the network" desc={
          status.upnp === "mapped" && status.public_address
            ? `The router is forwarding port ${status.port}. The phone reaches this machine at ${status.public_address} from any network — no relay, no VPN.`
            : status.upnp === "searching" ? "Asking the router to forward the port…"
            : status.upnp === "no_router" ? "No UPnP router answered. The phone works on this network; from outside it will need the port forwarded by hand."
            : status.upnp === "refused" ? "The router refused to forward the port. Turn on UPnP in its settings, or forward the port by hand."
            : "Off"
        }>
          <button className="act-btn" onClick={load}>Check again</button>
        </Row>
      )}
      <Row label="Port" desc={portError ?? "Changing it moves the listener and the router mapping at once. Phones must scan a new QR afterwards — the port is in it."}>
        <input
          type="number" min={1024} max={65535} value={port}
          onChange={(e) => setPort(e.target.value)}
          onBlur={commitPort} onKeyDown={blurOnEnter}
          style={{ width: 90 }}
        />
      </Row>
      <Row label="This machine's name" desc="What the phone shows for this desktop">
        <input
          type="text" value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={commitName} onKeyDown={blurOnEnter}
          style={{ width: 180 }}
        />
      </Row>
      {status.fingerprint && (
        <Row label="Identity" desc="The listener's certificate. A phone trusts this and nothing else; it changes only if remote-cert.pem is deleted.">
          <code className="diag-val" style={{ fontSize: 11 }}>{status.fingerprint.match(/.{1,4}/g)?.join(" ")}</code>
        </Row>
      )}
      {status.running && (
        <Row label="Pair a phone" desc="Open AITerm on the phone, tap Scan, and scan. The QR carries every address, this network first, so a phone paired here keeps working from outside." wide>
          <div style={{ display: "flex", gap: 16, alignItems: "flex-start", flexWrap: "wrap" }}>
            {pair && <div className="remote-qr" dangerouslySetInnerHTML={{ __html: pair.svg }} />}
            <div style={{ display: "flex", flexDirection: "column", gap: 8, maxWidth: 360 }}>
              <div><button className="act-btn" onClick={showQr}>{pair ? "Refresh QR" : "Show QR"}</button></div>
              {pairError && <span className="srow-value">{pairError}</span>}
              <div className="srow-desc">Addresses in the QR: {allAddresses.join(", ") || "none"}.</div>
              <div className="srow-desc">Can't scan? The same link can be opened on the phone: copy it from the QR with any reader, or type it — it is one line.</div>
            </div>
          </div>
        </Row>
      )}
      <Row label="Forget every phone" desc="Rotates the secret. Each phone must scan a new QR to reconnect.">
        {confirmForget ? (
          <span style={{ display: "inline-flex", gap: 8 }}>
            <button className="act-btn danger" onClick={forget}>Forget</button>
            <button className="act-btn" onClick={() => setConfirmForget(false)}>Keep</button>
          </span>
        ) : (
          <button className="act-btn" onClick={() => setConfirmForget(true)}>Forget…</button>
        )}
      </Row>
      <Row label="What the phone can do" desc="See the session list, read any session as a conversation, send it a message, interrupt it, open or stop it, and start a new one. It never receives terminal output, and it cannot change settings or touch files." />
    </>
  );
}

function sinceLabel(since: number, now: number): string {
  const s = Math.max(0, Math.round(now / 1000 - since));
  if (s < 60) return "connected just now";
  if (s < 3600) return `connected ${Math.floor(s / 60)}m ago`;
  return `connected ${Math.floor(s / 3600)}h ago`;
}
