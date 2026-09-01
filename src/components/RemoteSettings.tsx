import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Row from "./SettingsRow";
import {
  fingerprintLabel,
  inviteCountdownSeconds,
  inviteToShow,
  lastSeenLabel,
  listenerLabel,
  listenerAddressOptions,
  nextRevokeStep,
  preferredListenerConfig,
  rebindListener,
  type ListenerConfig,
  type PairingInvite,
  type PendingPairing,
  type RemoteStatus,
  type TrustedDevice,
} from "../remoteAccess.ts";
import {
  remoteApproveDevice,
  remoteBeginPairing,
  remoteBeginPairingCombined,
  remoteDenyDevice,
  remoteDevices,
  remoteInterfaces,
  remotePendingPairings,
  remoteRevokeDevice,
  remoteStart,
  remoteStatus,
  remoteStop,
  type PhonePairPayload,
  type PhoneRemoteStatus,
  phoneRemotePairPayload,
  phoneRemoteRotateToken,
  phoneRemoteSetEnabled,
  phoneRemoteSetIroh,
  phoneRemoteSetName,
  phoneRemoteSetPort,
  phoneRemoteStatus,
} from "../ipc";

const DEFAULT_PORT = 8443;
const LISTENER_PREFERENCE_KEY = "aiterm.remote.listener";

function loadListenerPreference(): ListenerConfig | null {
  try {
    const value = JSON.parse(localStorage.getItem(LISTENER_PREFERENCE_KEY) ?? "null");
    if (
      typeof value?.address === "string" &&
      Number.isInteger(value?.port) &&
      value.port >= 1024 &&
      value.port <= 65535
    ) {
      return value;
    }
  } catch { /* A corrupt renderer preference falls back to live discovery. */ }
  return null;
}

function saveListenerPreference(config: ListenerConfig) {
  try {
    localStorage.setItem(LISTENER_PREFERENCE_KEY, JSON.stringify(config));
  } catch { /* Private mode only makes the selection session-local. */ }
}

/**
 * Settings → Remote access: every way a phone reaches this desktop, in one
 * panel.
 *
 * Two independent listeners live behind it: the gateway (structured APIs,
 * per-device trust — the `android/` client) and the phone listener (the
 * `mobile/` client's protocol, token trust, with an optional iroh tunnel for
 * reach-from-anywhere). Either, or both, can be on — the connection-method
 * switches at the top are the whole choice, and the rest of the panel only
 * shows what the enabled methods need.
 *
 * Pairing is one button. With both listeners on it mints a combined QR —
 * the gateway's single-use invite with the phone listener's fields riding
 * behind under their own names — that either phone app can scan; with one
 * on, it shows that listener's own code. Every QR is rendered by the
 * backend: no pairing secret ever exists as a string in this webview.
 *
 * Trust decisions stay where they were: gateway devices are approved and
 * revoked here and nowhere else, and the phone listener's token is rotated
 * here ("forget every phone").
 */
export default function RemoteSettings() {
  // ---- gateway (LAN / VPN) state
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
  const [now, setNow] = useState(() => Date.now());

  // ---- phone listener state
  const [pstatus, setPstatus] = useState<PhoneRemoteStatus | null>(null);
  const [pair, setPair] = useState<PhonePairPayload | null>(null);
  const [pairError, setPairError] = useState<string | null>(null);
  const [pname, setPname] = useState("");
  const [pport, setPport] = useState("");
  const [pportError, setPportError] = useState<string | null>(null);
  const [confirmForget, setConfirmForget] = useState(false);
  // The 5s poll must not type over the person: while a field has focus,
  // its value belongs to them, not to the status refresh.
  const editing = useRef(false);

  const refresh = useCallback(async () => {
    const nextStatus = await remoteStatus();
    setStatus(nextStatus);
    remoteDevices().then(setDevices).catch(() => setDevices([]));
    remotePendingPairings().then(setPending).catch(() => setPending([]));
    return nextStatus;
  }, []);

  const loadPhone = useCallback(() => phoneRemoteStatus().then((s) => {
    setPstatus(s);
    if (!editing.current) { setPname(s.name); setPport(String(s.port)); }
  }), []);

  useEffect(() => {
    Promise.all([refresh(), remoteInterfaces()])
      .then(([currentStatus, found]) => {
        setAddresses(found);
        const initial = preferredListenerConfig(
          currentStatus,
          found,
          loadListenerPreference(),
        );
        setAddress(initial.address);
        setPort(initial.port);
      })
      .catch(() => {
        setStatus(null);
        setAddresses([]);
      });
    loadPhone().catch(() => setPstatus(null));
  }, [refresh, loadPhone]);

  // Phone connections come and go on their own schedule.
  useEffect(() => {
    const un = listen("remote://clients", () => loadPhone().catch(() => {}));
    const t = setInterval(() => { setNow(Date.now()); loadPhone().catch(() => {}); }, 5000);
    return () => { un.then((f) => f()); clearInterval(t); };
  }, [loadPhone]);

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
  const addressOptions = listenerAddressOptions(address, addresses);
  // Drop a spent invite from state as well as from the screen, so the next
  // "Pair phone" starts clean rather than flashing the dead one.
  useEffect(() => {
    if (invite && !shownInvite) setInvite(null);
  }, [invite, shownInvite]);

  const run = (work: Promise<unknown>, onSuccess?: () => void) => {
    setError(null);
    work
      .then(() => onSuccess?.())
      .catch((cause) => setError(String(cause)))
      .finally(() => { refresh().catch(() => setStatus(null)); });
  };

  const gatewayOn = !!status?.enabled;
  const phoneOn = !!pstatus?.enabled;

  const togglePhone = async (on: boolean) => {
    setPstatus(await phoneRemoteSetEnabled(on));
    if (!on) setPair(null);
  };
  const toggleIroh = async (on: boolean) => {
    setPstatus(await phoneRemoteSetIroh(on));
    setPair(null);
  };
  // One button. Both listeners on → the combined QR either app scans;
  // one on → that listener's own code.
  const showQr = async () => {
    setPairError(null);
    setError(null);
    setNow(Date.now());
    try {
      if (gatewayOn && phoneOn) {
        setPair(null);
        setInvite(await remoteBeginPairingCombined());
      } else if (gatewayOn) {
        setPair(null);
        setInvite(await remoteBeginPairing());
      } else if (phoneOn) {
        setInvite(null);
        setPair(await phoneRemotePairPayload());
      } else {
        setPairError("Turn a connection method on first.");
      }
    } catch (e) { setPair(null); setPairError(`${e}`); }
  };
  const forget = async () => {
    setPstatus(await phoneRemoteRotateToken());
    setPair(null);
    setConfirmForget(false);
  };
  const commitName = async () => {
    if (!pstatus || pname.trim() === pstatus.name) return;
    setPstatus(await phoneRemoteSetName(pname));
    setPair(null);
  };
  const commitPort = async () => {
    if (!pstatus) return;
    const p = Number(pport);
    if (!Number.isInteger(p) || p < 1024 || p > 65535) { setPportError("Pick a port from 1024 to 65535"); return; }
    if (p === pstatus.port) { setPportError(null); return; }
    try { setPstatus(await phoneRemoteSetPort(p)); setPportError(null); setPair(null); }
    catch (e) { setPportError(`${e}`); }
  };
  const blurOnEnter = (e: React.KeyboardEvent<HTMLInputElement>) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); };

  if (!status && !pstatus) {
    return <div className="sgroup-foot">Looking…</div>;
  }

  const qrAudience = gatewayOn && phoneOn
    ? "Either AITerm phone app can scan this — each reads its own half."
    : gatewayOn
      ? "For the gateway phone app."
      : "For the AITerm phone app.";

  return (
    <>
      <div className="sgroup">
        <div className="sgroup-title">Connection methods</div>
        <div className="sgroup-rows">
          <Row
            label="Gateway (LAN / VPN)"
            desc="The structured-API listener with per-device approval. Reachable on your LAN or over a VPN such as WireGuard; never from the open internet."
          >
            <label className="sw" aria-label="Gateway">
              <input
                type="checkbox"
                checked={gatewayOn}
                disabled={!status || (!gatewayOn && !address)}
                onChange={() =>
                  run(
                    gatewayOn ? remoteStop() : remoteStart(address, port),
                    () => saveListenerPreference({ address, port }),
                  )
                }
              />
              <span className="sw-track"><span className="sw-knob" /></span>
            </label>
          </Row>
          <Row
            label="Phone listener (LAN)"
            desc={pstatus?.running
              ? `Listening on port ${pstatus.port}. On this network the phone reaches this machine at ${pstatus.addresses[0] ?? "no address found"}.`
              : "The phone app's own listener. Off: nothing listens until you turn it on."}
          >
            <label className="sw" aria-label="Phone listener">
              <input type="checkbox" checked={phoneOn} disabled={!pstatus} onChange={(e) => togglePhone(e.target.checked)} />
              <span className="sw-track"><span className="sw-knob" /></span>
            </label>
          </Row>
          <Row
            label="Reach from anywhere (iroh)"
            desc={pstatus?.iroh_enabled && pstatus?.iroh_node && phoneOn
              ? "The phone dials this desktop's iroh node id from any network — no port forward, no VPN. End-to-end encrypted; relays see ciphertext."
              : "Rides the phone listener through an encrypted iroh tunnel, so the phone works from any network. Needs the phone listener on."}
          >
            <label className="sw" aria-label="iroh">
              <input
                type="checkbox"
                checked={!!pstatus?.iroh_enabled}
                disabled={!pstatus || !phoneOn}
                onChange={(e) => toggleIroh(e.target.checked)}
              />
              <span className="sw-track"><span className="sw-knob" /></span>
            </label>
          </Row>
          {pstatus?.error && <Row label="Problem" desc={pstatus.error} />}
        </div>
      </div>

      <div className="sgroup">
        <div className="sgroup-title">Pair a phone</div>
        <div className="sgroup-rows">
          <Row
            label="Pairing code"
            desc={gatewayOn
              ? "Single use, and it stops working after five minutes."
              : "Open AITerm on the phone, tap Scan, and scan."}
          >
            <button
              className="set-recheck"
              disabled={!gatewayOn && !phoneOn}
              onClick={showQr}
            >Pair phone</button>
          </Row>
          {shownInvite && (
            <Row label="Scan this in AITerm on your phone" wide>
              <div className="remote-qr">
                {/* The backend rendered this; the payload it encodes never
                    became a string in the renderer. */}
                <div dangerouslySetInnerHTML={{ __html: shownInvite.svg }} />
                <div className="sgroup-foot">
                  {qrAudience} Expires in {inviteCountdownSeconds(shownInvite, now)}s
                </div>
              </div>
            </Row>
          )}
          {!shownInvite && pair && (
            <Row label="Scan this in AITerm on your phone" wide>
              <div className="remote-qr" dangerouslySetInnerHTML={{ __html: pair.svg }} />
              <div className="sgroup-foot">{qrAudience}</div>
            </Row>
          )}
          {pairError && <div className="set-notice">{pairError}</div>}
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

      {status && (
        <div className="sgroup">
          <div className="sgroup-title">Gateway</div>
          <div className="sgroup-rows">
            <Row
              label="Address"
              desc="Choose a LAN or VPN address. Applying a live change briefly reconnects phones."
            >
              <div className="remote-listener-control">
                <select
                  className="set-select mono"
                  value={address}
                  onChange={(e) => {
                    const next = e.target.value;
                    setAddress(next);
                    if (!status.enabled) saveListenerPreference({ address: next, port });
                  }}
                >
                  {addressOptions.length === 0 && <option value="">No LAN or VPN address</option>}
                  {addressOptions.map((candidate) => (
                    <option key={candidate} value={candidate}>{candidate}</option>
                  ))}
                </select>
                {status.enabled && status.address && status.port !== null &&
                  (address !== status.address || port !== status.port) && (
                    <button
                      className="set-recheck"
                      disabled={!address}
                      onClick={() => {
                        const current = { address: status.address!, port: status.port! };
                        const target = { address, port };
                        setError(null);
                        rebindListener(
                          current,
                          target,
                          remoteStop,
                          (config) => remoteStart(config.address, config.port),
                        )
                          .then(() => saveListenerPreference(target))
                          .catch((cause) => {
                            setAddress(current.address);
                            setPort(current.port);
                            saveListenerPreference(current);
                            setError(String(cause));
                          })
                          .finally(() => { refresh().catch(() => setStatus(null)); });
                      }}
                    >Apply</button>
                  )}
              </div>
            </Row>
            <Row label="Port">
              <input
                className="set-input"
                type="number"
                min={1024}
                max={65535}
                value={port}
                onChange={(e) => {
                  const next = Number(e.target.value) || DEFAULT_PORT;
                  setPort(next);
                  if (!status.enabled) saveListenerPreference({ address, port: next });
                }}
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
      )}

      {pstatus && (
        <div className="sgroup">
          <div className="sgroup-title">Phone listener</div>
          <div className="sgroup-rows">
            <Row label="Connected now" desc={pstatus.clients.length === 0
              ? (pstatus.running ? "No phone is connected." : "—")
              : `${pstatus.clients.length} ${pstatus.clients.length === 1 ? "phone" : "phones"} holding a live connection.`} wide={pstatus.clients.length > 0}>
              {pstatus.clients.length > 0 && (
                <div className="remote-clients">
                  {pstatus.clients.map((c) => (
                    <div key={c.id} className="remote-client">
                      <strong>{c.device || "Unknown device"}</strong>
                      <span className="dim"> · {c.os || "?"} · app {c.app || "?"} · from {c.address} · {sinceLabel(c.since, now)}</span>
                    </div>
                  ))}
                </div>
              )}
            </Row>
            {pstatus.running && (
              <Row label="From outside the network" desc={
                pstatus.iroh_enabled && pstatus.iroh_node
                  ? "iroh is on: the phone reaches this desktop from any network by its node id — no relay of ours, no port forward, no VPN."
                  : pstatus.upnp === "mapped" && pstatus.public_address
                    ? `The router is forwarding port ${pstatus.port}. The phone reaches this machine at ${pstatus.public_address} from any network.`
                    : pstatus.upnp === "searching" ? "Asking the router to forward the port…"
                    : pstatus.upnp === "no_router" ? "No UPnP router answered. The phone works on this network; from outside it needs iroh, a VPN, or the port forwarded by hand."
                    : pstatus.upnp === "refused" ? "The router refused to forward the port. Turn on iroh, or UPnP in the router's settings."
                    : "Off"
              }>
                <button className="act-btn" onClick={() => loadPhone().catch(() => {})}>Check again</button>
              </Row>
            )}
            <Row label="Port" desc={pportError ?? "Changing it moves the listener and the router mapping at once. Phones must scan a new QR afterwards — the port is in it."}>
              <input
                type="number" min={1024} max={65535} value={pport}
                onChange={(e) => setPport(e.target.value)}
                onFocus={() => { editing.current = true; }}
                onBlur={() => { editing.current = false; commitPort(); }} onKeyDown={blurOnEnter}
                style={{ width: 90 }}
              />
            </Row>
            <Row label="This machine's name" desc="What the phone shows for this desktop">
              <input
                type="text" value={pname}
                onChange={(e) => setPname(e.target.value)}
                onFocus={() => { editing.current = true; }}
                onBlur={() => { editing.current = false; commitName(); }} onKeyDown={blurOnEnter}
                style={{ width: 180 }}
              />
            </Row>
            {pstatus.fingerprint && (
              <Row label="Identity" desc="The listener's certificate. A phone trusts this and nothing else; it changes only if remote-cert.pem is deleted.">
                <code className="diag-val" style={{ fontSize: 11 }}>{pstatus.fingerprint.match(/.{1,4}/g)?.join(" ")}</code>
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
          </div>
        </div>
      )}

      {status && (
        <div className="sgroup">
          <div className="sgroup-title">Paired phones (gateway)</div>
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
                        {lastSeenLabel(device, now)} — IP {device.last_ip ?? "not recorded"}
                      </div>
                      <div className="srow-desc">
                        Key {fingerprintLabel(device.fingerprint)}
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
              the gateway off does not — the phone stays trusted for next time.
            </div>
          </div>
        </div>
      )}

      {error && <div className="set-notice">{error}</div>}
    </>
  );
}

function sinceLabel(since: number, now: number): string {
  const s = Math.max(0, Math.floor(now / 1000 - since));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}
