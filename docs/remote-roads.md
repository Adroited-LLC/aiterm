# Remote roads — one desktop, many ways to reach it

A phone reaches a desktop over a *road*. Roads are independent, any set can be
on at once, and the phone tries them in the order its owner prefers. Every road
carries the same pinned-TLS bytes; none of them can read a session.

| Road | What it is | Desktop side | Phone side |
|---|---|---|---|
| `lan` | Direct, same network | advertise RFC1918 / ULA addresses in the QR | dial `https://<h>:<port>` |
| `vpn` | Direct over Tailscale / WireGuard / any VPN | detect `tailscale0`, `wg*`, `100.64/10`, `fc00::/7`; advertise those addresses; show interface + MagicDNS name when `tailscale` CLI answers | dial the same way; 100.64/10 + wg addresses rank as `vpn` |
| `relay` | AITerm Relay (blind TCP relay, SNI-routed) | a second relay route for the phone listener, enrolled the way the gateway's is | dial `https://<tr>:<tq>` with SNI = `tr`, same cert pin |
| `iroh` | peer-to-peer QUIC, relay fallback, no server of ours | iroh tunnel → phone listener; optional custom relay URL | loopback bridge, unchanged |

Two listeners live on the desktop and both keep working:

- **gateway** (`remote/server.rs`, Matt's) — speaks to the Adroited phone app. Roads: lan, vpn, relay. Untouched by this work.
- **phone listener** (`remote_api.rs`, 5lime) — speaks to the 5lime phone app. Roads: lan, vpn, relay, iroh. This document is about it.

## Desktop config (`~/.local/share/aiterm/remote.json`, struct `Config` in remote_api.rs)

New fields, all `#[serde(default)]`:

```
lan_enabled:   bool   default true
vpn_enabled:   bool   default true
relay_enabled: bool   default false   // needs an enrolled route to do anything
iroh_enabled:  bool   default true    // exists today
iroh_relay_url: Option<String>        // None = iroh's default (n0) relays
```

The phone-listener relay route persists at `<remote root>/phone-relay.json`
using `remote::relay::RelayConfig` verbatim (same `load`/`save`, same 0600).
A pending enrollment draft is in-memory only, replaced by each new QR, dropped
when the road is turned off.

## QR (5lime fields, appended to the combined payload by `pair_extension`)

Existing: `&tp=<port>&tt=<token>&tf=<cert sha256 hex>[&z=<iroh node id>]`
New:      `[&tr=<relay public host>&tq=<relay port>][&ta=<digest b64url nopad, 32 bytes>]`

- `tr`/`tq` present when `relay_enabled` and either a route exists or a draft was prepared.
- `ta` present only when a draft is waiting (no route yet). The phone signs it and
  calls the enroll endpoint below. `ta` absent + `tr` present = route already live.
- Hosts (`h`) are filtered by `lan_enabled` / `vpn_enabled`: a road that is off
  contributes no addresses. The combined QR still carries the gateway's own `h`
  list untouched; 5lime hosts are the ones inside `pair_extension` — add them as
  repeated `th=` so the two lists stay independent. `PairLink` reads `th` when
  present, else falls back to `h`.

## Phone listener HTTP (bearer token, existing router in remote_api.rs)

`GET /v1/status` — existing response gains:
```
"relay": {"host": "<tr>", "port": <tq>} | null      // live route only, never a draft
"roads": {"lan": bool, "vpn": bool, "relay": bool, "iroh": bool}
```
`POST /v1/relay/enroll`
```
{"authority_public_key": "<b64url nopad, 33-byte compressed SEC1 P-256>",
 "signature_der": "<b64url nopad DER ECDSA over the 32-byte digest, 8..=80 bytes>"}
→ 200 {"host": "<tr>", "port": <tq>}
→ 409 no pending draft (road off, route already live, or draft never prepared)
→ 400 bad key / signature does not verify against the draft digest
→ 502 relay refused (its message passed through)
```
On 200 the desktop: `RelayEnrollmentDraft::register`, save `phone-relay.json`,
start `RelayConnectorHandle::start(config, 127.0.0.1:<phone port>)`, notify
phones (`Event` status change). The digest and signature are exactly Matt's
(`relay-protocol::enrollment_digest`, `RelayConfig::prepare_enrollment` with the
phone listener's cert SHA-256 raw bytes as `desktop_spki_sha256`).

## Tauri commands (remote_api.rs; frontend `phoneRemote*` in ipc.ts)

`remote_api_status` → `RemoteStatus` gains:
```
roads:  {lan, vpn, relay, iroh}: bool
vpn:    {detected: bool, kind: "tailscale"|"wireguard"|"other"|null, interface: string|null,
         address: string|null, magic_dns: string|null}
relay:  {configured: bool, state: "off"|"connecting"|"connected"|"retrying",
         host: string|null, port: number|null, server: string, pending_enrollment: bool}
iroh_relay_url: string|null
```
New commands:
```
remote_set_road(road: "lan"|"vpn"|"relay"|"iroh", on: bool)   // remote_set_iroh stays, delegates
remote_set_iroh_relay_url(url: string|null)                    // restarts the tunnel when running
remote_phone_relay_clear()                                     // deprovision + delete phone-relay.json + stop connector
```
`remote_set_road("relay", true)` with no route prepares nothing by itself; the
next `remote_pair_payload` / combined QR prepares a draft (GET `<server>/v1/info`
on Matt's `DEFAULT_RELAY_SERVER`, reuse `remote::DEFAULT_RELAY_SERVER`).

## Phone (5lime app)

- `Desktop` gains `relayHost: String = ""`, `relayPort: Int = 0`, `roadOrder: List<String>`
  default `["lan","vpn","relay","iroh"]` (per desktop, editable in settings).
- Candidate URLs are built per road and tried in `roadOrder`; a road with nothing
  to dial is skipped. Classification of an `h`/`th` host: 100.64/10, fc00::/7,
  and any host that is not RFC1918/link-local → `vpn`; RFC1918 → `lan`.
- Pairing: after the first successful status probe, if the link carried `ta`,
  sign it with a P-256 key in AndroidKeyStore (alias `aiterm-relay-authority-p256-v1`,
  `SHA256withECDSA`, no user-auth requirement) and `POST /v1/relay/enroll`; on
  200 store `relayHost/relayPort`. Failure is non-fatal: the desktop is paired,
  the relay road just stays empty and the status poll fills it in later.
- Every status poll refreshes `relayHost/relayPort` from `status.relay`.
- Relay dial = plain `https://<relayHost>:<relayPort>` through the existing
  pinned `Api` (OkHttp sets SNI from the URL host; the pin is the same `tf`).
