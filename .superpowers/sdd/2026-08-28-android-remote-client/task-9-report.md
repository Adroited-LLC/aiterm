# Task 9 report: authenticated Android remote sessions and native terminal

## Status

`DONE — automated verification complete; live first-pair smoke is user-interactive`

Task 9 now has the seven binding Rust prerequisites, a remembered-device
authenticated Android WebSocket client, descriptor-bound roster and terminal
recovery, and a native Compose terminal/session surface. The phone consumes
Rust screen snapshots/diffs; it never replays PTY bytes and contains no WebView
or terminal emulator.

## Rust prerequisites closed before Android actions were exposed

1. Desktop recovery captures registry events before and during snapshot R,
   drops events through R, replays contiguous events after R, and refetches on
   gaps. The exact R/R+1 promise interleaving is deterministic (`27fc981` RED,
   `211c890` GREEN).
2. Remote rosters use their own bounded, ordered, row-independent descriptor
   transfer. Every envelope is intrinsically below 1 MiB; tab count and total
   resident transfer work are bounded (`40669dc`/`74f7abf` RED, `4d08b74`
   GREEN).
3. Registry egress runs in bounded bursts so sustained title/event traffic
   cannot starve correlated inbound work. A deterministic flood test proves a
   request completes within the bound (`4d08b74`).
4. Session archive/source retirement holds exact source/archive FDs and
   directory identities through final publication and retirement checks. It
   revalidates public bindings, source names, directory entry sets, hashes, and
   lease state immediately before exact-source truncation. Replacement,
   destination rename, lease break, and late directory entry tests fail with
   recoverable paths without truncating unarchived data (`d9cb1bf` RED,
   `0924d7b` GREEN).
5. Sidecar mode/timestamp/content metadata is derived after opening the held
   FD, never from a prior pathname stat (`0924d7b`).
6. Strict restore holds archive/destination directory identities, proves every
   created inode, and quarantines/removes only the exact archive object.
   Replacement paths remain untouched and the held archive receives a durable
   recovery name (`39d1d52` RED, `0daf00c`, `37cc84b` GREEN).
7. Permanent purge preopens and leases strict transcript, task, job, rollout,
   origin, and main artifacts; errors propagate and the main artifact is
   removed last (`21b2e21` RED, `615ce58` GREEN).

The aggregate-only blocking cancellation regression was made sticky with
`watch::send_replace` in `a2bdd81`; this removed the full-suite hang without
weakening the production bound.

## Android protocol and lifecycle

- `AuthenticatedRemoteTransport` requires the opening 32-byte challenge,
  checks the app unlock policy immediately before calling the existing
  Android-Keystore `SHA256withECDSA` signer, sends its DER signature, and
  requires `auth.ok` before decoding any application envelope. App lock and
  Keystore signing share one linear gate, so a lock cannot race a new proof
  onto the wire; the post-sign policy check also rejects a re-entrant lock.
- Every connection and reconnect is TLS 1.3-only, uses the paired desktop SPKI
  trust manager, and retains OkHttp's default hostname verifier. There is no
  trust-all, pin fallback, hostname bypass, or cleartext path.
- Binary frames, pending requests, completed correlations, event queues,
  transfers, roster entries, terminal dimensions, rows, cells, graphemes, and
  scrollback are bounded. Lock/disconnect/revoke cancels transport work and
  clears transfer/screen state. A connection generation owns dial, handshake,
  event, request, recovery, and selection work; late callbacks cannot publish
  after lock or replace a newer transport. Reconnect uses a bounded 1/2/4/8/16-second
  sequence and creates the same pinned transport policy each time.
- Desktop device revocation publishes an ordered `auth.revoked` event and
  closes the authenticated socket. A future proof from the removed key gets
  `auth.denied`; Android purges active state and never enters a reconnect loop.
- CBOR is definite length and rejects duplicate keys, trailing values,
  indefinite forms, unknown fields, overlarge frames, invalid versions,
  correlation mismatch, malformed cell structure, multiple base scalars per
  cell, and non-contiguous transfers.
- The dedicated roster assembler atomically publishes the complete Rust
  camelCase `RemoteTabDescriptor` transfer. The terminal assembler validates
  ordered row-boundary snapshot/diff/scrollback chunks and advances the screen
  only on complete semantic transfers. Diff mismatch requests correlated
  `terminal.resume` recovery instead of byte replay.
- Typed RPCs cover session list/preview/open/close/delete/fork/stop, agent list/start,
  tab list/open/close, terminal attach/input/resize/focus/scrollback/resume, and
  their public Task 5 error responses. Input includes exact tab and attachment
  ownership; an unowned phone stays read-only until explicit focus takeover.
- Tab selection is serialized through exact `terminal.detach` then
  `terminal.attach`. Stale attach replies are detached, and screen events are
  accepted only when both tab and attachment match the active selection.
  Attach-correlated terminal frames are held in a bounded transport queue
  until the client atomically commits that attachment, preventing both a lost
  initial snapshot and publication from a superseded tab. Superseded
  selections retain responsibility for detaching the old attachment.
- AppLock immediately closes and clears process client state. An unlocked
  process may reuse its remembered Keystore identity; a QR is not required on
  ordinary reconnect.

## Native UI

- The paired desktop list opens a typed terminal route with a lifecycle-owned
  client.
- A phone-sized drawer shows live Rust tabs, desktop sessions and their
  actions, available agents/models and capability summary, and new-shell
  controls. Transcript deletion requires explicit confirmation.
- The terminal is a native Compose grid with monospace cell placement, the
  full 256-color indexed palette plus RGB/default colors,
  bold/faint/italic/underline/inverse/hidden/strike attributes, wide-cell
  continuation handling, native block/beam/underline cursor rendering,
  Unicode combining content, selection, explicit copy, bounded paged
  scrollback, and host-bound `http`/`https` links that reject userinfo and
  malformed/non-network schemes.
- Soft-keyboard commit/composition input, hardware Backspace/Enter/arrows,
  canonical resize, bracketed paste, application cursor arrows, Escape,
  Control, Alt, Tab, arrows, Page Up/Down, and common shell
  symbols are exposed. Connection and focus rails never hide terminal output.
- Mouse reporting is intentionally absent: the current Rust `TerminalModes`
  wire model exposes only application cursor, bracketed paste, line wrap, and
  alternate screen, and the RPC set has no mouse-event request. Android does
  not fabricate an unsupported byte protocol. Adding mouse modes plus a typed
  broker request is the required future contract change.

Exact upstream desktop CSS was not changed.

## TDD and storm checkpoints

Android RED/GREEN boundaries were preserved rather than rewritten:

- state/store RED `1da8ecd`, GREEN `713bb2d`;
- strict wire/roster GREEN `309b16d`;
- terminal transfer RED `ebca6a5`, GREEN `bf514dd`;
- pinned authentication/correlation `cb766a5`;
- bounded reconnect `455456a`;
- native session UI `9065326`;
- scrollback/selection/copy/link/agent/layout completion `8150e03`;
- strict operation/cell validation `a14c67d`.
- lifecycle/tab race RED `fa44967`, GREEN `b62aed6`;
- bounded transfer/recovery flood `d27aee4`;
- live revocation/denial `e706700`;
- complete RPC/render/input controls `567d373`;
- lock/sign, attachment-publication, nullable-preview, and cell-geometry
  review fixes `154dc4b`;
- superseded-selection detach race `c692f62`.

Each meaningful boundary was committed and followed by `sync` because severe
weather made power loss plausible.

## Verification evidence

The ruled-unsafe backend target that can consult real HOME was not run. HOME
was never repurposed and preserved dumps were not inspected.

Fresh final Rust verification:

```text
cargo test --lib -- --test-threads=1
425 passed, 0 failed, 7 ignored

safe integration aggregate (remote_auth, remote_desktop, remote_operations,
remote_protocol, remote_server, remote_terminal, tab_registry, terminal_screen)
209 passed, 0 failed

cargo check
passed
```

Fresh final desktop/Android evidence:

```text
npm run test:ui
75 passed, 0 failed

npm run build
TypeScript and Vite production build passed

./gradlew :app:testDebugUnitTest :app:assembleDebug :app:lintDebug
98 JVM tests passed; build passed; lint 0 errors

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:connectedDebugAndroidTest
14/14 instrumentation tests passed on Pixel 10 Pro XL / API 37

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:installDebug
installed on exactly one pinned device

adb -s 10.0.0.115:37713 shell am start -W \
  -n com.adroited.aiterm/.MainActivity
cold launch status ok (1792 ms)
```

The terminal-specific suite includes two pinned-Pixel tests covering
read-only focus takeover/native rendering/extra keys and deterministic
portrait-to-landscape viewport recalculation without losing typed output.

## Manual interoperability boundary

The installed app is ready for live use. A true desktop/phone smoke still
needs the user to unlock the app and, if this debug install has no retained
pairing, scan/approve one QR. The manual check should attach a live tab, run
`printf '✓\n'`, type through the phone, take/release focus, resize/rotate, page
scrollback, disconnect/reconnect, and revoke the phone. No automated step
changes biometric state, device credentials, remote-access settings, or trust
records.
