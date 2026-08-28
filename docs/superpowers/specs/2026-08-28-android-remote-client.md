# Android Remote Client Design

## Goal

Add a native Android companion app that lets a paired phone securely use the
same AITerm desktop instance over a LAN or user-provided VPN. The phone is a
full interactive client: it can inspect and manage sessions and agents, open
and close terminal tabs, and send and receive terminal bytes.

## Scope

The first release is direct-device only. AITerm desktop is the authoritative
host and embeds the gateway; Android never reads desktop transcript files or
starts a local agent process. There is no cloud account, relay, port-forwarding
guide, or remote Internet exposure. Reachability outside a home LAN is the
user's VPN responsibility (for example Tailscale or WireGuard).

The Android application id is `com.adroited.aiterm`. It is a Kotlin /
Jetpack Compose application, with a minimum SDK of 26 and current stable
AndroidX dependencies at implementation time.

## Architecture

```text
Android AITerm (Kotlin / Compose)
        |  HTTPS + WebSocket, certificate pinning
        v
AITerm desktop gateway (Rust, loopback disabled, LAN/VPN listener)
        |  one internal command/service API
        +------------------------------+
        | existing session/agent/PTTY  |
        | implementation               |
        +------------------------------+
```

The desktop gateway is an adapter over service functions extracted from the
current Tauri commands. Tauri commands and remote RPC handlers must call the
same service functions; neither client may reach the other's transport layer.
PTYs remain owned by the desktop process. A terminal subscription has an
independent server-generated stream id and does not expose the internal PTY id
to Android.

## Pairing and trust

1. The desktop user explicitly enables Remote Access and selects **Pair phone**.
2. Desktop starts (or reuses) a TLS listener bound to selected LAN/VPN
   interfaces, creates a self-signed ECDSA P-256 certificate, and records its
   SHA-256 SPKI fingerprint.
3. Desktop creates a cryptographically random, single-use enrollment secret
   valid for five minutes. The QR contains a versioned `aiterm://pair` payload
   with hostname/IP candidates, port, server fingerprint, enrollment secret,
   and a display name for the desktop.
4. Android scans the QR, confirms the desktop identity, opens TLS only if the
   presented public-key fingerprint matches the QR, then sends its generated
   P-256 public key and requested device name with the enrollment secret.
5. Desktop displays the request and requires explicit approval. On approval it
   assigns a random device id, persists the Android public key and metadata,
   and returns a signed device certificate / refresh credential restricted to
   that device. The enrollment secret is consumed whether approval succeeds or
   fails.
6. Android stores the device credential and private key in Android Keystore;
   keys require `BIOMETRIC_STRONG | DEVICE_CREDENTIAL` user authentication.
   The app locks after five minutes in the background and requires biometric or
   device-PIN authentication before reconnecting or displaying terminal data.

The desktop stores its listener key, trusted devices, and pending enrollments
in a private AITerm state directory with owner-only permissions. Device rows
include name, public-key fingerprint, created time, and last-seen time.
Revocation terminates active connections, rejects future handshakes, and
deletes the device record. Pairing has no
implicit approval and no fallback to HTTP, an unpinned certificate, a bearer
token alone, or mDNS-discovered hosts.

## Transport and protocol

All traffic uses TLS 1.3 with the desktop certificate pinned from pairing.
After TLS, Android proves possession of its private key by signing a fresh
server nonce; the server verifies the trusted, non-revoked device key and
creates a short-lived connection session. A reconnect repeats this proof and
does not need another QR scan.

The gateway exposes one versioned WebSocket endpoint, `/v1/ws`. Frames are
binary CBOR envelopes:

```text
{ version: 1, request_id: u64, kind: string, payload: bytes }
```

Client requests include `session.list`, `session.preview`, `session.open`,
`session.close`, `session.delete`, `session.fork`, `session.stop`,
`agent.list`, `agent.action`, `terminal.attach`, `terminal.input`,
`terminal.resize`, and `terminal.detach`. The exact first-release command set
is constrained to actions already exposed by AITerm desktop; sensitive
desktop-only actions (settings-file writes, font installation, arbitrary file
system writes, and diagnostic toggles) stay local until their permissions and
UX are separately designed.

Server events include `snapshot`, `session.changed`, `agent.changed`,
`terminal.output`, `terminal.exited`, `terminal.title`, and `error`. Terminal
output contains raw bytes. The server maintains a bounded per-stream replay
buffer (1 MiB); an attach response includes a full terminal snapshot, then the
stream sequence number. A reconnect asks for output after its last acknowledged
sequence; if the buffer has rolled over, the server sends a fresh snapshot.

Only one client may hold input focus for a terminal at once. Attaching a second
client remains read-capable but its input requests are rejected with
`terminal.input_not_owned` until it explicitly takes focus. Taking focus emits
an event to every attached client. This avoids accidental concurrent typing
from desktop and phone while preserving visibility everywhere.

All protocol parsers enforce maximum frame sizes, strict version checks,
request rate limits, input and terminal-size bounds, and request-id replay
protection. Validation failures return structured errors and never panic or
drop the process.

## Android application

The app has four primary states:

- **No desktops:** scan QR or open the camera pairing view.
- **Locked:** show paired desktops but require biometric/PIN before connecting
  or displaying cached metadata.
- **Disconnected:** show the selected desktop, trusted-device status, and a
  reconnect action; never silently weaken certificate pinning.
- **Connected:** a phone-optimized session drawer, active terminal, agent
  summary, and overflow actions.

The terminal uses a native Kotlin terminal emulator component that consumes
the raw byte stream, supports UTF-8 and standard VT/xterm control sequences,
copy/paste, scrollback, selection, links, resize, soft keyboard input, and an
extra-key row (Escape, Control, Alt, Tab, arrows, Page Up/Down, and common
shell symbols). Its renderer must not use a WebView.

The session drawer shows the same session metadata the desktop exposes. The
active terminal presents connection and focus ownership states prominently.
Phone controls map to the server command set; a control absent from the remote
protocol is not simulated locally. The app uses foreground notifications only
while a user-enabled persistent session is active, and stops the service on
disconnect, lock, revoke, or user action.

## Desktop UI

Remote Access appears in Settings with enable/disable, bind addresses, port,
current certificate fingerprint, **Pair phone**, pending approval requests,
and a trusted-device list. Disabling Remote Access closes the listener and all
connections but does not revoke devices; revocation is explicit. The QR dialog
shows the expiry countdown and never logs or copies the enrollment secret.

## Reliability and observability

The listener is off by default and fails closed. Its lifecycle is independent
of a single terminal tab: desktop app restarts preserve trusted devices and
regenerate only expired enrollment tokens. It does not attempt to start at OS
login until that behavior is separately approved.

Structured, redacted diagnostics record listener lifecycle, device id prefix,
connection state, protocol version, and denial reason. They never record QR
payloads, enrollment secrets, credentials, terminal input, or terminal output.

## Non-goals for the first release

- Cloud relay, hosted account, or NAT traversal.
- iOS support.
- Sharing a desktop with other people or role-based permissions.
- Direct remote filesystem browsing/editing.
- Background agent execution from Android after the desktop app exits.
- Importing, exporting, or copying device credentials between phones.

## Acceptance criteria

1. A user can pair one Android device by scanning a five-minute QR and
   approving it on the desktop; it reconnects after an app restart without QR.
2. A wrong/changed TLS public key, expired/consumed QR secret, unapproved
   device, or revoked device cannot connect.
3. A paired and unlocked device can manage the agreed desktop sessions and
   interact with terminal bytes without corruption.
4. Desktop and Android receive coherent session/agent changes and terminal
   output after reconnects.
5. A second client cannot silently interleave terminal input with the current
   input owner.
6. The Android app needs biometric/PIN after the configured lock timeout and
   stores private material only through Android Keystore.
