import { useEffect, useState } from "react";
import Row from "./SettingsRow";
import {
  PairPayload, RemoteStatus,
  remotePairPayload, remoteRotateToken, remoteSetEnabled, remoteSetName, remoteStatus,
} from "../ipc";

/** Settings → Remote: the phone side of aiterm. One switch, one QR, and a
 *  way to forget every phone. Everything else is decided for you — the
 *  point of the phone app is that it needs no settings. */
export default function RemotePane() {
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [pair, setPair] = useState<PairPayload | null>(null);
  const [pairError, setPairError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [confirmForget, setConfirmForget] = useState(false);

  const load = () => remoteStatus().then((s) => { setStatus(s); setName(s.name); });
  useEffect(() => { load(); }, []);

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

  if (!status) return null;
  return (
    <>
      <Row label="Remote access" desc={status.running
        ? `Listening on port ${status.port}. The phone app reaches this machine at ${status.addresses[0] ?? "no address found"}.`
        : "Off. Nothing listens until you turn this on."}>
        <label className="sw" aria-label="Remote access">
          <input type="checkbox" checked={status.enabled} onChange={(e) => toggle(e.target.checked)} />
          <span className="sw-track"><span className="sw-knob" /></span>
        </label>
      </Row>
      {status.error && (
        <Row label="Problem" desc={status.error} />
      )}
      <Row label="This machine's name" desc="What the phone shows for this desktop">
        <input
          type="text" value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={commitName}
          onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }}
          style={{ width: 180 }}
        />
      </Row>
      {status.running && (
        <Row label="Pair a phone" desc="Open AITerm on the phone, tap Pair, and scan. Both devices must be on the same network or the same Tailscale." wide>
          <div style={{ display: "flex", gap: 16, alignItems: "flex-start", flexWrap: "wrap" }}>
            <button className="act-btn" onClick={showQr}>{pair ? "Refresh QR" : "Show QR"}</button>
            {pairError && <span className="srow-value">{pairError}</span>}
            {pair && (
              <div
                style={{ background: "#fff", padding: 8, borderRadius: 6, width: 256, height: 256 }}
                dangerouslySetInnerHTML={{ __html: pair.svg }}
              />
            )}
          </div>
          {status.addresses.length > 1 && (
            <div className="srow-desc" style={{ marginTop: 8 }}>
              The QR carries every address, best first: {status.addresses.join(", ")}.
            </div>
          )}
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
      <Row label="What the phone can do" desc="See the session list, read any session as a conversation, send it a message, open or stop it, and start a new one. It never receives terminal output, and it cannot change settings or touch files." />
    </>
  );
}
