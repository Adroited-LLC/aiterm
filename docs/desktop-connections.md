# Connect to another desktop

AiTerm can view and control another computer’s live terminal sessions through the same authenticated gateway used by Android. Sessions continue running on the host computer. This does not copy session files, credentials, projects, or settings between computers.

1. On the host, open **Settings → Remote Access**, enable remote access, and select the **AITerm network** stack.
2. Under **Pair a device**, choose **Save pairing file**. Transfer that file to the other computer and open it within five minutes.
3. On the client, choose the monitor button (**Connected desktops**) in the top toolbar, then **Pair another desktop**. Enter the client’s device name and open the pairing file.
4. Approve the request on the host. The client saves its own key and the host’s pinned identity.
5. Select a desktop and live session. Choose **Take control** to type. **History** displays earlier terminal output. **This desktop** disconnects and returns to local sessions.

Pairing files are single-use invitations, expire after five minutes, and are written with private file permissions. They are not session archives. The host retains approval and revocation controls under Remote Access. Forgetting a pairing only removes the client’s saved identity; revoke the trusted device on the host to remove its authorization there.

## Implementation

`src-tauri/src/desktop_remote` owns saved identities, pinned TLS, P-256 challenge authentication, pairing approval, requests, reconnects, and the canonical terminal model. The same module is compiled into the Windows WSL backend. The renderer receives a projection through long polling and uses the existing xterm renderer for display, keyboard input, and IME composition.

The client uses the existing `/v1/ws` gateway, `tab.list`, `session.roster`, and `terminal.*` operations. LAN/VPN addresses and the saved TLS relay route are tried with the same SPKI pin. Iroh and direct QUIC optimization are not implemented in the desktop client; use the AITerm network stack for desktop pairing.

Reconnects authenticate again and recover the selected terminal with a new attachment and snapshot. They do not automatically take input control. Queued input is bound to the connection generation, reconnect attempt, and selected tab; stale or canceled requests are rejected, and writes are never replayed. A lost write acknowledgement is reported as a connection change rather than retried.

Tests exercise the real TLS gateway for pairing approval, authentication, terminal attachment, focus, input, history, and reattachment after a disconnect. Additional tests cover certificate pinning, route validation, private identity storage, stale input, and safe screen rendering.
