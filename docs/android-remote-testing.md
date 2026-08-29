# Remote access: topology, pairing, and how to test it

AITerm desktop can be driven from a paired Android phone. This describes what
that connection is, what it deliberately is not, and how to verify it by hand.

## What the connection is

The desktop is the host. It embeds a TLS gateway; the phone is a client. All
of it runs over your own network:

```text
Android AITerm  ──TLS 1.3, pinned──▶  AITerm desktop gateway
   (phone)                              (this machine)
                                             │
                                     sessions, agents, PTYs
```

The phone never reads a transcript file, never starts an agent process, and
never owns a PTY. It asks the desktop, and the desktop answers.

**Reachability is yours to arrange.** The gateway binds a LAN or VPN address
you pick. There is no relay, no hosted account, no NAT traversal, and no
port-forwarding advice here — forwarding this port to the internet is not a
supported configuration. To reach your desktop from outside the house, put
both devices on the same VPN (Tailscale and WireGuard both work) and bind the
VPN address. Loopback and link-local addresses are not offered as bind
candidates: a phone cannot reach either, so a listener on one starts cleanly
and then fails in a way that is tedious to diagnose.

The listener is **off by default** and starts nothing on disk until you turn
it on. A desktop that never pairs a phone never grows a trusted-device file.

## Pairing

1. **Settings → Remote access → Turn on.** Pick the address the phone can
   reach. The panel shows the listener's certificate fingerprint, grouped in
   fours so a person can actually compare it.
2. **Pair phone.** A QR appears with a countdown. It carries a single-use
   32-byte secret and stops working after five minutes, whichever comes first.
3. **Scan it in AITerm on the phone.** The phone pins the fingerprint from the
   QR *before* it sends anything, and refuses to continue if the certificate
   presented does not match.
4. **Approve on the desktop.** The phone appears under "Pair a phone" with the
   fingerprint of the key it generated. Nothing is trusted until you approve
   it here. The QR secret is consumed either way — approve or deny, that code
   is spent.
5. The phone stores its private key in Android Keystore, requiring biometric
   or device PIN. Reconnecting later needs no QR: the phone signs a fresh
   challenge with the key you approved.

Pairing the same phone again replaces its row rather than adding a second one,
so the list always shows one row per phone and revoking that row is complete.

## Revoking

**Settings → Remote access → Paired phones → Revoke**, then confirm. Revoking
forgets the key, refuses future handshakes, and drops the phone's live
connection.

Turning remote access **off** is a different statement: it closes the listener
and every connection but keeps your phones trusted, so turning it back on does
not mean scanning a QR again.

## When the fingerprint changes

The phone refuses to connect and says the desktop's identity does not match.
That is the pinning working. It means one of:

- The desktop's TLS identity was regenerated — its key file was deleted, or
  the remote state directory was moved or restored from a backup.
- You are connecting to a different machine that answers on that address.
- Something is intercepting the connection.

There is no "continue anyway". The fix is to revoke the phone on the desktop
and pair again, which is a deliberate act at the desktop keyboard — and if you
did not expect the change, find out why before you do it.

## What a phone cannot do

Not oversights; each is a decision:

- Enable the listener, approve another device, or revoke one. Trust is granted
  at the desktop keyboard only.
- Write settings, install fonts, browse or edit the filesystem, or toggle
  diagnostics. These return `remote.unsupported`.
- Type into a terminal another client is holding input focus on. Two clients
  may watch one terminal; only one may type, and taking focus is explicit and
  visible to everyone attached.
- Keep an agent running after the desktop app exits.

## Manual test

Requires a desktop and a phone on the same LAN or VPN.

**Pairing**
1. Turn remote access on, note the fingerprint, tap **Pair phone**.
2. Scan on the phone. Confirm the name and fingerprint it shows match the
   desktop's.
3. Approve on the desktop. The phone reaches the session list.

**Terminal fidelity**
4. Open a session on the phone and run `printf '✓\n'`. A multi-byte character
   must arrive intact — this catches chunk-boundary corruption.
5. Run something that draws a full screen (`top`, or an agent TUI). Rotate the
   phone and confirm the resize reflows rather than corrupting.

**Reconnection**
6. Put the phone in airplane mode for ten seconds, then restore it. The
   session must resume where it was, not blank and not duplicated.
7. Leave it disconnected long enough to produce more than 1 MiB of output
   (`yes | head -c 2000000`), then reconnect. The phone must redraw from a
   snapshot rather than appending a partial stream.

**Focus**
8. With the same terminal open on desktop and phone, type on the desktop.
9. Type on the phone: it must be refused, with a visible way to take focus.
10. Take focus on the phone. The desktop must show that it lost it.

**Lock**
11. Background the phone app for five minutes. Returning must require
    biometric or PIN before any terminal content is shown.

**Revocation**
12. With the phone connected, revoke it on the desktop. Its connection must
    drop immediately, and reconnecting must fail without a new pairing.

**Off is not revoked**
13. Turn remote access off and on again. A trusted phone must reconnect with
    no QR.

## Diagnostics

Remote access logs listener lifecycle, a device id prefix, connection state,
protocol version, and the reason a connection was denied. It never logs a QR
payload, an enrollment secret, a credential, or a single byte of terminal
input or output.
