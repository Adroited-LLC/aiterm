import { useCallback, useEffect, useState } from "react";
import Row from "./SettingsRow";
import SettingsSwitch from "./SettingsSwitch";
import {
  fingerprintLabel,
  inviteCountdownSeconds,
  inviteToShow,
  lastSeenLabel,
  listenerLabel,
  relayLabel,
  relayServerFromConnectorUrl,
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
  remoteDenyDevice,
  remoteDevices,
  remoteInterfaces,
  remotePendingPairings,
  remoteRevokeDevice,
  remoteRelayClear,
  remoteRelayConfigure,
  remoteRelayServerSet,
  remoteStart,
  remoteStartOnLaunchSet,
  remoteStatus,
  remoteStop,
} from "../ipc";

const DEFAULT_PORT = 8443;
const LISTENER_PREFERENCE_KEY = "aiterm.remote.listener";
const DEFAULT_RELAY_SERVER = "https://control.34-23-107-73.sslip.io:8443";

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
  const [relayServer, setRelayServer] = useState(DEFAULT_RELAY_SERVER);
  const [relayConnectorUrl, setRelayConnectorUrl] = useState("");
  const [relayPublicHost, setRelayPublicHost] = useState("");
  const [relayPublicPort, setRelayPublicPort] = useState(443);
  const [relayRouteId, setRelayRouteId] = useState("");
  const [relayToken, setRelayToken] = useState("");
  const [invite, setInvite] = useState<PairingInvite | null>(null);
  const [pending, setPending] = useState<PendingPairing[]>([]);
  const [devices, setDevices] = useState<TrustedDevice[]>([]);
  /** Device the revoke button is armed for; a second click on the same row commits. */
  const [armed, setArmed] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Drives the QR countdown and expiry. One timer for the panel, not one per
  // element, and it only ticks while an invite is outstanding.
  const [now, setNow] = useState(() => Date.now());

  const refresh = useCallback(async () => {
    const nextStatus = await remoteStatus();
    setStatus(nextStatus);
    remoteDevices().then(setDevices).catch(() => setDevices([]));
    remotePendingPairings().then(setPending).catch(() => setPending([]));
    return nextStatus;
  }, []);

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
        setRelayServer(currentStatus.relay_server || DEFAULT_RELAY_SERVER);
        setRelayConnectorUrl(currentStatus.relay?.connector_url ?? "");
        setRelayPublicHost(currentStatus.relay?.public_host ?? "");
        setRelayPublicPort(currentStatus.relay?.public_port ?? 443);
        setRelayRouteId(currentStatus.relay?.route_id ?? "");
      })
      .catch(() => {
        setStatus(null);
        setAddresses([]);
      });
  }, [refresh]);

  useEffect(() => {
    if (!status?.enabled || !status.relay?.configured) return;
    const timer = setInterval(() => {
      remoteStatus().then(setStatus).catch(() => {});
    }, 2000);
    return () => clearInterval(timer);
  }, [status?.enabled, status?.relay?.configured]);

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

  if (!status) {
    return <div className="sgroup-foot">Looking…</div>;
  }
  const relayServerChanged = relayServer.trim() !== status.relay_server;

  return (
    <>
      <div className="sgroup">
        <div className="sgroup-title">Remote connection</div>
        <div className="sgroup-rows">
          <Row
            label="Relay server"
            desc="The first approved phone authorizes this desktop. AITerm creates the private route automatically as part of pairing."
            wide
          >
            <div className="remote-managed-relay">
              <input
                className="set-input mono"
                value={relayServer}
                list="aiterm-relay-servers"
                placeholder={DEFAULT_RELAY_SERVER}
                aria-label="Relay server"
                spellCheck={false}
                onChange={(event) => setRelayServer(event.target.value)}
              />
              <datalist id="aiterm-relay-servers">
                <option value={DEFAULT_RELAY_SERVER}>AITerm Relay</option>
                {status.relay_server !== DEFAULT_RELAY_SERVER && (
                  <option value={status.relay_server}>Saved relay server</option>
                )}
              </datalist>
              <div className="remote-relay-actions">
                <button
                  className="set-recheck"
                  disabled={status.enabled || status.relay?.configured || !relayServer.trim() || !relayServerChanged}
                  onClick={() => run(remoteRelayServerSet(relayServer))}
                >Save server</button>
                {status.relay?.configured && (
                  <button
                    className="set-recheck"
                    disabled={status.enabled}
                    onClick={() => run(remoteRelayClear())}
                  >Remove relay</button>
                )}
              </div>
              <div className="sgroup-foot">
                {status.relay?.configured && relayServerChanged
                  ? `Current route remains on ${relayServerFromConnectorUrl(status.relay.connector_url) ?? "its existing server"}. Turn remote access off, remove it, then save this server.`
                  : status.relay?.configured
                    ? relayLabel(status)
                    : relayServerChanged
                      ? "Save this server before turning remote access on. AITerm verifies its control identity and public domain first."
                      : "Pair and approve a phone. LAN, VPN, and relay setup complete together."}
              </div>
            </div>
          </Row>
          <Row
            label="Remote access"
            desc="Automatically tries LAN, then VPN, then your AITerm Relay. One switch controls the same encrypted desktop gateway on every route."
          >
            <button
              className="set-recheck"
              onClick={() =>
                run(
                  status.enabled ? remoteStop() : remoteStart(address, port),
                  () => saveListenerPreference({ address, port }),
                )
              }
              disabled={!status.enabled && (!address || relayServerChanged)}
            >
              {status.enabled ? "Turn off" : "Turn on"}
            </button>
          </Row>
          <Row
            label="Start relay when AITerm opens"
            desc="Restores remote access with this address and port, then reconnects the saved private relay route. Off by default."
          >
            <SettingsSwitch
              checked={status.start_on_launch}
              disabled={!status.relay?.configured || !address || relayServerChanged}
              label="Start relay when AITerm opens"
              onChange={(enabled) => run(
                remoteStartOnLaunchSet(enabled, address, port),
                () => saveListenerPreference({ address, port }),
              )}
            />
          </Row>
          <Row
            label="Address"
            desc="Preferred local address. AITerm listens on the other shareable LAN/VPN addresses too, so phones can switch routes automatically."
          >
            <div className="remote-listener-control">
              <select
                className="set-select mono"
                value={address}
                onChange={(e) => {
                  const next = e.target.value;
                  setAddress(next);
                  if (!status.enabled) {
                    saveListenerPreference({ address: next, port });
                    if (status.start_on_launch) {
                      run(remoteStartOnLaunchSet(true, next, port));
                    }
                  }
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
                        .then(async () => {
                          saveListenerPreference(target);
                          if (status.start_on_launch) {
                            await remoteStartOnLaunchSet(true, target.address, target.port);
                          }
                        })
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
                if (!status.enabled) {
                  saveListenerPreference({ address, port: next });
                  if (status.start_on_launch) {
                    run(remoteStartOnLaunchSet(true, address, next));
                  }
                }
              }}
            />
          </Row>
          <Row label="Status">
            <span className="diag-val">{listenerLabel(status)}</span>
          </Row>
          <Row
            label="AITerm Relay"
            desc="The relay only forwards opaque TLS bytes. It cannot read device keys, sessions, terminal data, or files."
          >
            <span className="diag-val">{relayLabel(status)}</span>
          </Row>
          {!status.relay?.configured && (
          <Row label="Advanced manual route" desc="For self-hosted relays that do not support phone-authorized setup." wide>
            <details className="remote-relay-advanced">
              <summary>Enter route details manually</summary>
              <div className="remote-relay-grid">
              <input
                className="set-input mono"
                value={relayConnectorUrl}
                disabled={status.enabled}
                placeholder="wss://control.relay.example.com/v1/connect"
                aria-label="Relay connector URL"
                onChange={(event) => setRelayConnectorUrl(event.target.value)}
              />
              <input
                className="set-input mono"
                value={relayPublicHost}
                disabled={status.enabled}
                placeholder="route-id.relay.example.com"
                aria-label="Relay public host"
                onChange={(event) => setRelayPublicHost(event.target.value)}
              />
              <input
                className="set-input"
                type="number"
                min={1}
                max={65535}
                value={relayPublicPort}
                disabled={status.enabled}
                aria-label="Relay public port"
                onChange={(event) => setRelayPublicPort(Number(event.target.value) || 443)}
              />
              <input
                className="set-input mono"
                value={relayRouteId}
                disabled={status.enabled}
                placeholder="route id"
                aria-label="Relay route ID"
                onChange={(event) => setRelayRouteId(event.target.value)}
              />
              <input
                className="set-input mono"
                type="password"
                value={relayToken}
                disabled={status.enabled}
                placeholder={status.relay?.configured ? "Stored — leave blank to keep" : "Connector token"}
                aria-label="Relay connector token"
                onChange={(event) => setRelayToken(event.target.value)}
              />
              <div className="remote-relay-actions">
                <button
                  className="set-recheck"
                  disabled={status.enabled || !relayConnectorUrl || !relayPublicHost || !relayRouteId}
                  onClick={() => run(
                    remoteRelayConfigure(
                      relayConnectorUrl,
                      relayPublicHost,
                      relayPublicPort,
                      relayRouteId,
                      relayToken || null,
                    ),
                    () => setRelayToken(""),
                  )}
                >Save relay</button>
              </div>
              </div>
            </details>
          </Row>
          )}
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
              disabled={!status.enabled || relayServerChanged}
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
            remote access off does not — the phone stays trusted for next time.
          </div>
        </div>
      </div>

      {error && <div className="set-notice">{error}</div>}
    </>
  );
}
