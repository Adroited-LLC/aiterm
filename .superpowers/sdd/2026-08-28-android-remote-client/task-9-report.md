# Task 9 report: authenticated Android remote sessions and native terminal

## Status

`DONE — Fix Round 2 automated verification complete; live first-pair smoke is user-interactive`

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

## Fix Round 1 — external review dispositions

Every claim was checked against the then-current implementation before it was
changed. All nine automated findings were technically valid; none required a
pushback. The live-pair finding remains deliberately user-interactive. The
review fixes are the immutable storm checkpoints `ba90fc2` through `33133ab`,
plus the exact maximum-burst coverage checkpoint `0cfdb2d`.

### Critical findings

1. **CRITICAL 1 — accepted and fixed.** `AuthenticatedRemoteTransport` now has
   one bounded outbound actor. It assigns monotonically increasing request IDs
   only as it dequeues and serially sends requests, so independently scheduled
   callers cannot reorder IDs or terminal input. Responses remain correlated
   and may complete independently. Covering test:
   `android/app/src/test/java/com/adroited/aiterm/remote/AuthenticatedRemoteTransportTest.kt`
   (`outboundRequestsCannotOvertakeAnEarlierBlockedSend`). Command and output:
   `./gradlew :app:testDebugUnitTest :app:assembleDebug :app:lintDebug` —
   `106 tests, 0 failures, 0 errors, 0 skipped; BUILD SUCCESSFUL; lint passed`.
2. **CRITICAL 2 — accepted and fixed.** A descriptor-derived recovery hardlink
   captures the exact source inode before the quarantine race window. Strict
   file purge captures the exact archive to a kernel-backed tombstone before
   truncation; recursive purge does the same for children and the directory.
   A pathname replacement is restored or left untouched, and a verified
   zero-length tombstone is deliberately retained where Linux has no atomic
   pathname-conditional unlink. Covering tests in `src-tauri/src/sessions.rs`:
   `source_name_replacement_immediately_before_quarantine_is_never_retired`,
   `strict_file_purge_restores_replacement_swapped_after_name_check`,
   `recursive_purge_restores_directory_replacement_swapped_after_final_check`,
   and `permanent_trash_delete_never_unlinks_a_rollout_name_replacement`.
   Command and output: isolated `CARGO_TARGET_DIR`,
   `cargo test --lib -- --test-threads=1` —
   `429 passed, 0 failed, 7 ignored`.
3. **CRITICAL 3 — accepted and fixed.** Archive publication now creates and
   fsyncs a descriptor-derived recovery link before source retirement,
   revalidates the public archive binding immediately before each exact-source
   truncation, and retains the recovery link through post-retirement
   validation. Covering tests in `src-tauri/src/sessions.rs`:
   `destination_replacement_after_quarantine_prevents_source_truncation` and
   `leased_archive_detects_a_destination_name_swap_before_retirement`.
   Command and output: the same isolated library command —
   `429 passed, 0 failed, 7 ignored`.
4. **CRITICAL 4 — accepted and fixed.** File-set and sidecar restore helpers now
   return a `PreparedRestore` holding exact destination FDs. Archive removal
   retains and revalidates that transaction after exact archive capture and
   before archive truncation; mismatch preserves the archive recovery link.
   Covering tests in `src-tauri/src/sessions.rs`:
   `strict_restore_keeps_archive_when_destination_is_replaced_before_removal`
   and `strict_restore_rechecks_destination_after_archive_capture_before_truncate`.
   Command and output: the same isolated library command —
   `429 passed, 0 failed, 7 ignored`.

### Important findings

1. **IMPORTANT 1 — accepted and fixed.** After each eight-event registry
   budget, the server gives inbound work one nonblocking fair turn and resets
   the event budget even when no request is waiting. Covering tests:
   `src-tauri/tests/remote_server.rs`
   (`sustained_registry_events_cannot_starve_a_correlated_inbound_request`,
   `idle_authenticated_connection_continues_past_each_registry_fairness_budget`).
   Command and output: isolated `CARGO_TARGET_DIR`,
   `cargo test --test remote_auth --test remote_desktop --test remote_operations
   --test remote_protocol --test remote_server --test remote_terminal
   --test tab_registry --test terminal_screen -- --test-threads=1` —
   `210 passed, 0 failed` (including `remote_server: 39 passed`).
2. **IMPORTANT 2 — accepted and fixed.** Publication now uses one suspending,
   backpressured path. Reader teardown closes in `finally`, including when a
   failure notice cannot enter a full queue. Covering tests:
   `AuthenticatedRemoteTransportTest.kt`
   (`maximumValidEventBurstBackpressuresWithoutClosingTheTransport`,
   `maximumValidHeldAttachmentBurstDrainsWithoutClosingTheTransport`, and
   `protocolFailureClosesEvenWhenFailureNotificationCannotBeQueued`). The
   dedicated command
   `./gradlew :app:testDebugUnitTest --tests
   com.adroited.aiterm.remote.AuthenticatedRemoteTransportTest` produced
   `13 tests, 0 failures, 0 errors; BUILD SUCCESSFUL`; this explicitly exercises
   the protocol's legal 128-event and 512-held-frame maxima.
3. **IMPORTANT 3 — accepted and fixed.** Held attachments now retain
   pending/publish/discard and complete state; publication and reader arrival
   share one mutex, and a held correlation remains pinned until its final chunk
   drains. Covering tests in `AuthenticatedRemoteTransportTest.kt`:
   `attachmentDrainCannotBeOvertakenByANewChunk` and
   `attachmentCorrelationRemainsPinnedPastCompletedRequestEviction`. Command
   and output: the final Android aggregate above — `106/106 JVM tests passed`.
4. **IMPORTANT 4 — accepted and fixed.** One in-flight scrollback transaction
   owns the generation, transport, tab, attachment, offset, and assigned
   request ID. Duplicate offsets are rejected and an unexpected correlation
   cannot append rows. Covering tests:
   `android/app/src/test/java/com/adroited/aiterm/remote/RemoteClientTest.kt`
   (`rapidScrollbackPagingKeepsOnlyOneRequestForTheExpectedOffset`,
   `unexpectedScrollbackCorrelationCannotPublishOutOfOrderRows`). Command and
   output: the final Android aggregate above — `106/106 JVM tests passed`.
5. **IMPORTANT 5 — accepted and fixed.** A measured monospace `TerminalMetrics`
   is now the single source for viewport, row, cell, cursor, and text geometry.
   A `LazyColumn` virtualizes scrollback and visible rows. Covering tests:
   `android/app/src/androidTest/java/com/adroited/aiterm/ui/TerminalScreenTest.kt`
   (`measuredGridKeepsWideCombiningAndCursorOnTheSameFontScaledGeometry`,
   `largeScrollbackComposesOnlyTheBoundedVisibleRowWindow`). Command and output:
   `ANDROID_SERIAL=10.0.0.115:37713 ./gradlew
   :app:connectedDebugAndroidTest` — `16/16 passed on Pixel 10 Pro XL / API 37;
   BUILD SUCCESSFUL`.
6. **IMPORTANT 6 — USER-INTERACTIVE, not automated.** A live desktop/phone QR,
   desktop approval, device unlock, attach/input/reconnect/revoke smoke still
   requires the user. No test changed credentials, biometrics, trust, or remote
   access settings.

The eager mixed drawer is recorded as the review's deferred minor; the terminal
geometry work did not require risking an unrelated drawer rewrite. Mouse remains
a legitimate future typed-contract gap and is not a Task 9 defect.

### Prior-task guarantee checks

- Default-disabled LAN/VPN bind is still explicit: `RemoteState::default()` has
  no gateway, status is enabled only when `gateway.is_some()`, and only
  `remote_start(address, port)` validates a shareable non-loopback/non-link-local
  address and starts the listener.
- QR lifecycle remains single-use and approval-gated. The safe `remote_auth`
  suite passed `pairing_submission_consumes_the_qr_but_does_not_trust_before_desktop_approval`,
  `enrollment_can_be_approved_exactly_once_before_expiry`, and revoked-device
  persistence checks.
- Android Keystore generation still requires user authentication, strong
  biometric or device credential on API 30+, and an unlocked device on API 28+.
  The pinned instrumentation aggregate includes `AndroidDeviceKeyStoreTest`.
- `android/app/build.gradle.kts` still declares
  `applicationId = "com.adroited.aiterm"` and `minSdk = 26`; the final JVM
  aggregate includes `AppIdentityTest`.

### Fresh Fix Round 1 verification

All Rust commands used a newly created isolated `CARGO_TARGET_DIR`. The unsafe
real-HOME backend target was not run; HOME was not repurposed and preserved
dumps were not inspected. No Fix Round 1 change touched `src/App.css` or desktop
frontend code, so the already-green 75-test UI/build evidence above remains the
applicable desktop evidence.

```text
cargo test --lib -- --test-threads=1
429 passed, 0 failed, 7 ignored (436 total)

safe integration aggregate listed above
210 passed, 0 failed

cargo check
passed

./gradlew :app:testDebugUnitTest :app:assembleDebug :app:lintDebug
106 passed, 0 failed; APK build passed; lint passed

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:connectedDebugAndroidTest
16/16 passed on the pinned Pixel

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:installDebug
installed on exactly one pinned device

adb -s 10.0.0.115:37713 shell am force-stop com.adroited.aiterm
adb -s 10.0.0.115:37713 shell am start -W \
  -n com.adroited.aiterm/.MainActivity
Status: ok; LaunchState: COLD; TotalTime: 746 ms
```

## Fix Round 2

All review claims were reproduced or confirmed against the current code before
implementation. The seven automated findings are fixed; the live pairing smoke
remains explicitly user-interactive.

1. **CRITICAL archive recovery cleanup — accepted and fixed.** The durable
   descriptor-derived recovery link now remains until the final public archive
   binding validates. A swap at that boundary fails closed and retains the
   recovery object. Covering test: `src-tauri/src/sessions.rs`,
   `archive_recovery_survives_public_unlink_at_final_release_boundary`.
   Focused command: `CARGO_TARGET_DIR=<isolated> cargo test --lib
   archive_recovery_survives_public_unlink_at_final_release_boundary --
   --exact --nocapture`; output: `1 passed, 0 failed`.
2. **CRITICAL restore validation-to-truncate — accepted and redesigned.** Linux
   offers no atomic multi-path validation-and-deletion transaction. Strict
   restore therefore retains a bounded-name, descriptor-bound full-byte hidden
   recovery object, known to the internal recovery contract and removable only
   by explicit exact-object purge. Destination bindings remain held and are
   revalidated through archive capture. Covering tests in `sessions.rs`:
   `strict_restore_retains_full_archive_when_destination_swaps_at_retirement_boundary`
   and `retained_restore_archive_is_full_bounded_and_only_exact_purge_can_zero_it`.
   Focused command: `CARGO_TARGET_DIR=<isolated> cargo test --lib
   strict_restore_retains_full_archive_when_destination_swaps_at_retirement_boundary
   -- --exact --nocapture`; output: `1 passed, 0 failed`. The disk cost is
   intentional: successful restore retains recovery bytes and directory
   metadata so a concurrent pathname mutation cannot cause unrecoverable loss.
3. **IMPORTANT full event queue close — accepted and fixed.** Transport close
   now closes `eventChannel`; termination does not depend on successfully
   enqueueing a failure or revocation event into an already-full queue. Queued
   events drain and collectors then complete. Covering tests in
   `AuthenticatedRemoteTransportTest.kt`:
   `protocolFailureClosesEvenWhenFailureNotificationCannotBeQueued` and
   `revocationWithAFullEventQueueStillTerminatesCollectors`. Command:
   `./gradlew :app:testDebugUnitTest --tests
   com.adroited.aiterm.remote.AuthenticatedRemoteTransportTest`; output:
   `BUILD SUCCESSFUL`, with both tests passing.
4. **IMPORTANT scrollback selection cleanup — accepted and fixed.** Tab
   selection clears ownership of the prior tab's in-flight scrollback request;
   late A chunks cannot affect B or block B paging. Covering test:
   `RemoteClientTest.kt`,
   `selectingBDiscardsAPagingTransactionAndAllowsBPaging`. Command:
   `./gradlew :app:testDebugUnitTest --tests
   com.adroited.aiterm.remote.RemoteClientTest`; output: `BUILD SUCCESSFUL`.
5. **IMPORTANT render-area sizing — accepted and fixed.** The exact padded
   `BoxWithConstraints` content bounds now drive both advertised viewport
   dimensions and drawing. Covering instrumentation test:
   `TerminalScreenTest.kt`,
   `advertisedViewportUsesTheFontScaledPaddedRenderBoundsAcrossRotation`.
   Focused command: `ANDROID_SERIAL=10.0.0.115:37713 ./gradlew
   :app:connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.class=com.adroited.aiterm.ui.TerminalScreenTest#advertisedViewportUsesTheFontScaledPaddedRenderBoundsAcrossRotation`;
   output: `1/1 passed on Pixel 10 Pro XL; BUILD SUCCESSFUL`.
6. **IMPORTANT live smoke — USER-INTERACTIVE.** Live QR enrollment, desktop
   approval, device unlock, Unicode/input, resize, focus, reconnect, and revoke
   still require the user. No automation weakened credentials, biometric/device
   credential policy, approval, trust, or remote-access settings.
7. **IMPORTANT outbound enqueue/close race — accepted and fixed.** Request
   acceptance and outbound enqueue are one `stateLock`-linearized transition;
   close closes the outbound channel and exceptionally completes every accepted
   deferred. Covering test: `AuthenticatedRemoteTransportTest.kt`,
   `closeRacingAnAcceptedEnqueueCompletesTheDeferredExceptionally`. Command:
   the focused transport-test command in item 3; output: `BUILD SUCCESSFUL`.
8. **IMPORTANT transport/client lock inversion — accepted and fixed.** The
   writer invokes assignment callbacks only after releasing transport state;
   client teardown detaches under `lifecycleLock` but cancels jobs and closes
   transport only after releasing it. Covering tests:
   `AuthenticatedRemoteTransportTest.kt`,
   `requestAssignmentCallbackNeverRunsUnderTransportStateLock`, and
   `RemoteClientTest.kt`,
   `disconnectClosesTransportOutsideTheClientLifecycleLock`. Commands: the
   focused transport and client commands above; output: `BUILD SUCCESSFUL`.

Previously addressed serialized request IDs, exact source/tombstone purge,
fair registry budgeting, and held attachment drain/correlation remain covered
by the aggregate suites. Mouse is still a future typed-contract addition; the
eager mixed drawer remains the accepted deferred minor.

### Fresh Fix Round 2 verification

Rust commands used the fresh isolated target
`/tmp/aiterm-task9-round2.VMrDFy`. The unsafe real-HOME backend target was not
run, HOME was not repurposed, preserved dumps were not inspected, and
`src/App.css` was not changed.

```text
CARGO_TARGET_DIR=/tmp/aiterm-task9-round2.VMrDFy \
  cargo test --lib -- --test-threads=1
432 passed, 0 failed, 7 ignored (439 total)

CARGO_TARGET_DIR=/tmp/aiterm-task9-round2.VMrDFy cargo test \
  --test remote_auth --test remote_desktop --test remote_operations \
  --test remote_protocol --test remote_server --test remote_terminal \
  --test tab_registry --test terminal_screen -- --test-threads=1
210 passed, 0 failed

CARGO_TARGET_DIR=/tmp/aiterm-task9-round2.VMrDFy cargo check
passed

./gradlew :app:testDebugUnitTest --rerun-tasks
111 passed, 0 failed, 0 errors, 0 skipped; BUILD SUCCESSFUL

./gradlew :app:assembleDebug :app:lintDebug
assemble passed; lint passed; BUILD SUCCESSFUL

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:connectedDebugAndroidTest
17/17 passed on Pixel 10 Pro XL; BUILD SUCCESSFUL

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:installDebug
Installed on exactly one device; BUILD SUCCESSFUL

adb -s 10.0.0.115:37713 shell am force-stop com.adroited.aiterm
adb -s 10.0.0.115:37713 shell am start -W \
  -n com.adroited.aiterm/.MainActivity
Status: ok; Activity: com.adroited.aiterm/.MainActivity
```
