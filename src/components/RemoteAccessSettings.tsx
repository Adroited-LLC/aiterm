import { useCallback, useEffect, useState } from "react";
import Row from "./SettingsRow";
import {
  fingerprintLabel,
  inviteCountdownSeconds,
  inviteToShow,
  lastSeenLabel,
  listenerLabel,
  nextRevokeStep,
  type PairingInvite,
  type PendingPairing,
  type RemoteStatus,
  type TrustedDevice,
} from "../remoteAccess.ts";
import {
  remoteApproveDevice,
  remoteBeginPairing,
  remoteDenyDevice,
  remoteDevices,
  remoteInterfaces,
  remotePendingPairings,
  remoteRevokeDevice,
  remoteStart,
  remoteStatus,
  remoteStop,
} from "../ipc";

const DEFAULT_PORT = 8443;

/**
 * Remote Access: the desktop side of phone pairing.
 *
 * Every decision that grants or removes trust is made here and nowhere else.
 * A paired phone cannot enable the listener, approve another phone, or revoke
 * one — so this panel is the whole trust boundary, and it is built to be read
 * rather than clicked through: the fingerprint is grouped for comparison, the
 * QR shows its own expiry, and revoking asks twice.
 *
 * The decisions themselves live in `src/remoteAccess.ts`, under test. This
 * file only draws them.
 */
export default function RemoteAccessSettings() {
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [addresses, setAddresses] = useState<string[]>([]);
  const [address, setAddress] = useState<string>("");
  const [port, setPort] = useState(DEFAULT_PORT);
  const [invite, setInvite] = useState<PairingInvite | null>(null);
  const [pending, setPending] = useState<PendingPairing[]>([]);
  const [devices, setDevices] = useState<TrustedDevice[]>([]);
  /** Device the revoke button is armed for; a second click on the same row commits. */
  const [armed, setArmed] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Drives the QR countdown and expiry. One timer for the panel, not one per
  // element, and it only ticks while an invite is outstanding.
  const [now, setNow] = useState(() => Date.now());

  const refresh = useCallback(() => {
    remoteStatus().then(setStatus).catch(() => setStatus(null));
    remoteDevices().then(setDevices).catch(() => setDevices([]));
    remotePendingPairings().then(setPending).catch(() => setPending([]));
  }, []);

  useEffect(() => {
    refresh();
    remoteInterfaces()
      .then((found) => {
        setAddresses(found);
        setAddress((current) => current || found[0] || "");
      })
      .catch(() => setAddresses([]));
  }, [refresh]);

  // A phone that scans the QR appears here only once the desktop notices it,
  // so poll while a pairing is actually in flight — and only then.
  useEffect(() => {
    if (!invite) return;
    const timer = setInterval(() => {
      setNow(Date.now());
      remotePendingPairings().then(setPending).catch(() => {});
    }, 1000);
    return () => clearInterval(timer);
  }, [invite]);

  const shownInvite = status ? inviteToShow(status, invite, now) : null;
  // Drop a spent invite from state as well as from the screen, so the next
  // "Pair phone" starts clean rather than flashing the dead one.
  useEffect(() => {
    if (invite && !shownInvite) setInvite(null);
  }, [invite, shownInvite]);

  const run = (work: Promise<unknown>) => {
    setError(null);
    work.then(refresh).catch((cause) => setError(String(cause)));
  };

  if (!status) {
    return <div className="sgroup-foot">Looking…</div>;
  }

  return (
    <>
      <div className="sgroup">
        <div className="sgroup-title">Listener</div>
        <div className="sgroup-rows">
          <Row
            label="Remote access"
            desc="Lets a paired phone use this desktop over your LAN or VPN. Off until you turn it on, and never reachable from the internet."
          >
            <button
              className="set-recheck"
              onClick={() =>
                run(status.enabled ? remoteStop() : remoteStart(address, port))
              }
              disabled={!status.enabled && !address}
            >
              {status.enabled ? "Turn off" : "Turn on"}
            </button>
          </Row>
          <Row label="Address" desc="Loopback is not offered: a phone cannot reach it.">
            <select
              className="set-select"
              value={address}
              disabled={status.enabled}
              onChange={(e) => setAddress(e.target.value)}
            >
              {addresses.length === 0 && <option value="">No LAN or VPN address</option>}
              {addresses.map((candidate) => (
                <option key={candidate} value={candidate}>{candidate}</option>
              ))}
            </select>
          </Row>
          <Row label="Port">
            <input
              className="set-input"
              type="number"
              min={1024}
              max={65535}
              value={port}
              disabled={status.enabled}
              onChange={(e) => setPort(Number(e.target.value) || DEFAULT_PORT)}
            />
          </Row>
          <Row label="Status">
            <span className="diag-val">{listenerLabel(status)}</span>
          </Row>
          <Row
            label="Certificate fingerprint"
            desc="Your phone pins this. If it ever shows a different one, do not continue."
            wide
          >
            <code className="diag-val">{fingerprintLabel(status.fingerprint ?? "")}</code>
          </Row>
        </div>
      </div>

      <div className="sgroup">
        <div className="sgroup-title">Pair a phone</div>
        <div className="sgroup-rows">
          <Row
            label="Pairing code"
            desc="Single use, and it stops working after five minutes."
          >
            <button
              className="set-recheck"
              disabled={!status.enabled}
              onClick={() => {
                setError(null);
                setNow(Date.now());
                remoteBeginPairing()
                  .then(setInvite)
                  .catch((cause) => setError(String(cause)));
              }}
            >Pair phone</button>
          </Row>
          {shownInvite && (
            <Row label="Scan this in AITerm on your phone" wide>
              <div className="remote-qr">
                {/* The backend rendered this; the payload it encodes never
                    became a string in the renderer. */}
                <div dangerouslySetInnerHTML={{ __html: shownInvite.svg }} />
                <div className="sgroup-foot">
                  Expires in {inviteCountdownSeconds(shownInvite, now)}s
                </div>
              </div>
            </Row>
          )}
          {pending.length > 0 && (
            <div className="agent-list">
              {pending.map((request) => (
                <div key={request.id} className="agent-row">
                  <div className="agent-text">
                    <div className="agent-name">{request.name} wants to pair</div>
                    <div className="srow-desc">
                      Key {fingerprintLabel(request.fingerprint)}
                    </div>
                  </div>
                  <button
                    className="set-recheck"
                    onClick={() => run(remoteApproveDevice(request.id).then(() => setInvite(null)))}
                  >Approve</button>
                  <button
                    className="set-recheck"
                    onClick={() => run(remoteDenyDevice(request.id))}
                  >Deny</button>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="sgroup">
        <div className="sgroup-title">Paired phones</div>
        <div className="sgroup-rows">
          {devices.length === 0 ? (
            <div className="sgroup-foot">No phones paired.</div>
          ) : (
            <div className="agent-list">
              {devices.map((device) => (
                <div key={device.id} className="agent-row">
                  <div className="agent-text">
                    <div className="agent-name">{device.name}</div>
                    <div className="srow-desc">
                      {lastSeenLabel(device, now)} — key {fingerprintLabel(device.fingerprint)}
                    </div>
                  </div>
                  <button
                    className="set-recheck"
                    onClick={() => {
                      const step = nextRevokeStep(armed, device.id);
                      if (step === "confirmed") {
                        setArmed(null);
                        run(remoteRevokeDevice(device.id));
                      } else {
                        setArmed(step);
                      }
                    }}
                    onBlur={() => setArmed((current) => (current === device.id ? null : current))}
                  >
                    {armed === device.id ? "Confirm revoke" : "Revoke"}
                  </button>
                </div>
              ))}
            </div>
          )}
          <div className="sgroup-foot">
            Revoking forgets the phone's key and drops its connection. Turning
            remote access off does not — the phone stays trusted for next time.
          </div>
        </div>
      </div>

      {error && <div className="set-notice">{error}</div>}
    </>
  );
}
