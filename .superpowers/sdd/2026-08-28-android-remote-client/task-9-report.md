# Task 9 report: authenticated Android remote sessions and native terminal

## Status

`PENDING — Fix Round 5 automated verification complete; live pairing/interoperability retry required`

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

## Fix Round 3

Both automated review claims were reproduced against the round-two code before
implementation and are fixed. The live pairing smoke remains explicitly
user-interactive.

1. **IMPORTANT full-queue/channel-completion client state — accepted and
   fixed.** Transport shutdown now closes the event channel with an out-of-band
   typed terminal cause (`Revoked` or `Recoverable`) that cannot be displaced by
   a full event queue. The generation-owned client collector handles normal
   unexpected completion and typed causes, clears active attachment state,
   screen, scrollback, assemblers, and transfers, then either enters bounded
   reconnect or remains revoked. Explicit close advances/cancels the generation
   and cannot resurrect reconnect. Covering tests:
   `android/app/src/test/java/com/adroited/aiterm/remote/RemoteClientTest.kt`,
   `fullQueueRevocationCompletionPurgesStateAndNeverReconnects` and
   `fullQueueProtocolFailureCompletionPurgesStateAndReconnects`; transport
   terminal-cause coverage remains in
   `AuthenticatedRemoteTransportTest.kt`,
   `protocolFailureClosesEvenWhenFailureNotificationCannotBeQueued` and
   `revocationWithAFullEventQueueStillTerminatesCollectors`. Focused command:
   `./gradlew :app:testDebugUnitTest --tests
   'com.adroited.aiterm.remote.RemoteClientTest' --tests
   'com.adroited.aiterm.remote.AuthenticatedRemoteTransportTest'`; output:
   `32 passed, 0 failed; BUILD SUCCESSFUL`.
2. **IMPORTANT retained-recovery purge exhaustion — accepted and fixed.** Exact
   purge now applies separate bounded physical-name and unique-object budgets.
   Retained aliases are opened and deduplicated by the held descriptor's
   `(device,inode)` identity before the unique-object limit is enforced. Purge
   preflight retains exact FDs and bounded hashes without installing hundreds
   of simultaneous Linux leases; each exact object acquires the established
   write lease immediately before its quarantine/truncation transaction.
   Descriptor parents are shared so the 512-object bound does not exhaust file
   descriptors. Path/name/depth/archive-byte bounds and final directory
   identity checks remain in force. Covering tests in
   `src-tauri/src/sessions.rs`:
   `exact_purge_deduplicates_more_than_257_retained_restore_alias_pairs`
   creates 258 distinct retained objects with 516 hardlink aliases and proves
   exact purge succeeds without touching a different outside object;
   `exact_purge_counts_distinct_retained_inodes_before_destructive_work`
   proves 513 distinct inodes are rejected before destructive work. Focused
   commands and outputs:
   `CARGO_TARGET_DIR=/tmp/aiterm-task9-round3-red.M1rGKQ cargo test --lib
   exact_purge_deduplicates_more_than_257_retained_restore_alias_pairs --
   --nocapture` — `1 passed, 0 failed`; the equivalent distinct-inode command —
   `1 passed, 0 failed`; and `cargo test --lib purge -- --nocapture` —
   `6 passed, 0 failed`. The existing deterministic pathname-replacement test
   also confirmed the displaced inode receives a durable recovery link before
   the mismatch is reported.
3. **IMPORTANT live smoke — USER-INTERACTIVE.** Live QR enrollment, desktop
   approval, app/device unlock, Unicode/input, resize, focus, reconnect, and
   revoke still require the user. No automation weakened or changed QR,
   biometric/device credential, approval, trust, or remote-access settings.

Previously addressed archive recovery lifetime, non-destructive strict restore
recovery, accept/close linearization, lock ordering, selection-owned scrollback,
exact padded geometry, serialized wire IDs, and attachment draining remain
covered by the aggregate suites. The retained-recovery ruling remains in force;
mouse is a future typed-contract addition and the eager mixed drawer remains the
deferred minor.

### Fresh Fix Round 3 verification

Rust commands used the isolated target
`/tmp/aiterm-task9-round3-red.M1rGKQ`. The unsafe real-HOME backend target was
not run, HOME was not repurposed, preserved dumps were not inspected, and
`src/App.css` was not changed.

```text
CARGO_TARGET_DIR=/tmp/aiterm-task9-round3-red.M1rGKQ \
  cargo test --lib -- --test-threads=1
434 passed, 0 failed, 7 ignored (441 total)

CARGO_TARGET_DIR=/tmp/aiterm-task9-round3-red.M1rGKQ cargo test \
  --test remote_auth --test remote_desktop --test remote_operations \
  --test remote_protocol --test remote_server --test remote_terminal \
  --test tab_registry --test terminal_screen -- --test-threads=1
210 passed, 0 failed

CARGO_TARGET_DIR=/tmp/aiterm-task9-round3-red.M1rGKQ cargo check
passed

./gradlew :app:testDebugUnitTest --rerun-tasks
113 passed, 0 failed, 0 errors, 0 skipped; BUILD SUCCESSFUL

./gradlew :app:assembleDebug :app:lintDebug
assemble passed; lint passed; BUILD SUCCESSFUL

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:connectedDebugAndroidTest
17/17 passed on Pixel 10 Pro XL; BUILD SUCCESSFUL

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:installDebug
Installed on exactly one device; BUILD SUCCESSFUL

adb -s 10.0.0.115:37713 shell am force-stop com.adroited.aiterm
adb -s 10.0.0.115:37713 shell am start -W \
  -n com.adroited.aiterm/.MainActivity
Status: ok; LaunchState: COLD; TotalTime: 858 ms
```

## Fix Round 4

The remaining automated review finding was reproduced against the round-three
code and is fixed. The live interoperability smoke remains explicitly
user-interactive.

1. **IMPORTANT terminal-cause ordering — accepted and fixed.** The transport
   now commits its typed terminal cause to the event channel before completing
   accepted requests. Every request still completes, but terminal teardown
   completes it with the same `RemoteTransportTerminatedException` carrying
   `Revoked` or `Recoverable`, rather than an earlier unclassified disconnect.
   Client request waiters route that typed result through the exact originating
   generation and transport; ordinary send/timeout failures are likewise
   generation-and-transport scoped. Whichever continuation runs first therefore
   applies one authoritative policy: revocation purges and stops, recoverable
   termination purges and schedules one bounded reconnect, and a late outcome
   is stale only after the same policy has already advanced the generation.
   Explicit close/lock still advances and cancels ownership before closing the
   transport. Channel closure and deferred completion remain outside the
   transport state lock, while transport job cancellation/close remains outside
   the client lifecycle lock.

   The deterministic combined client/real-transport regression is
   `android/app/src/test/java/com/adroited/aiterm/remote/RemoteClientTest.kt`,
   `pendingRequestFailureCannotWinBeforeFullQueueRevocationOutcome`. It gates
   the client collector ahead of the real transport flow, fills all 64 event
   slots, proves a request was sent and remains unanswered, then delivers
   `auth.revoked`. RED checkpoint `d7d690e`; GREEN checkpoint `7afe008`; both
   were followed by `sync`.

   RED command and output:

   ```text
   ./gradlew :app:testDebugUnitTest --tests \
     'com.adroited.aiterm.remote.RemoteClientTest.pendingRequestFailureCannotWinBeforeFullQueueRevocationOutcome'
   1 test completed, 1 failed
   expected:<Revoked> but was:<Reconnecting>
   BUILD FAILED
   ```

   GREEN focused command and output:

   ```text
   ./gradlew :app:testDebugUnitTest --tests \
     'com.adroited.aiterm.remote.RemoteClientTest' --tests \
     'com.adroited.aiterm.remote.AuthenticatedRemoteTransportTest'
   RemoteClientTest: 17 passed, 0 failed, 0 errors
   AuthenticatedRemoteTransportTest: 16 passed, 0 failed, 0 errors
   BUILD SUCCESSFUL
   ```

   Existing focused coverage also reconfirmed recoverable full-queue reconnect
   (`fullQueueProtocolFailureCompletionPurgesStateAndReconnects`), explicit
   lock/late-connect suppression (`lockCancelsPendingRequestsTransfersAndConnection`,
   `lockDuringConnectCannotPublishTheLateConnection`), accepted-deferred close
   completion (`closeRacingAnAcceptedEnqueueCompletesTheDeferredExceptionally`),
   and both sides of the lock-order contract
   (`requestAssignmentCallbackNeverRunsUnderTransportStateLock`,
   `disconnectClosesTransportOutsideTheClientLifecycleLock`).

2. **IMPORTANT live smoke — USER-INTERACTIVE.** Live QR enrollment, desktop
   approval, app/device unlock, Unicode/input, resize, focus, reconnect, and
   revoke still require the user. No automation weakened or changed QR,
   biometric/device credential, approval, trust, or remote-access settings.

Previously addressed Rust purge/recovery and Android lifecycle work were not
modified. The retained-recovery ruling remains binding; mouse is still a future
typed-contract addition, and the eager mixed drawer remains the deferred minor.
`src/App.css` was not changed. The unsafe real-HOME backend target was not run,
HOME was not repurposed, and preserved dumps were not inspected.

### Fresh Fix Round 4 verification

```text
./gradlew :app:testDebugUnitTest :app:assembleDebug :app:lintDebug --rerun-tasks
114 passed, 0 failed, 0 errors, 0 skipped
assemble passed; lint passed; BUILD SUCCESSFUL

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:connectedDebugAndroidTest
17/17 passed on Pixel 10 Pro XL / API 37; BUILD SUCCESSFUL

ANDROID_SERIAL=10.0.0.115:37713 ./gradlew :app:installDebug
Installed on exactly one device; BUILD SUCCESSFUL

adb -s 10.0.0.115:37713 shell am force-stop com.adroited.aiterm
adb -s 10.0.0.115:37713 shell am start -W \
  -n com.adroited.aiterm/.MainActivity
Status: ok; LaunchState: COLD; TotalTime: 826 ms
```

## Fix Round 5

The live first-pair attempt exposed one desktop TLS identity interoperability
defect after the automated round-four gate. The secure desktop fix is committed
and verified; Task 9 remains pending deployment and a complete live retry.

### Root-cause evidence

- The desktop first bound `192.168.1.99` and persisted a self-signed gateway
  certificate with SANs `IP Address:192.168.1.99, DNS:localhost`.
- The listener was later rebound to the phone-reachable `10.0.0.151`. The Pixel
  at `10.0.0.115` could ping the desktop and establish TCP to port `8443`, but
  Android correctly reported `UNREACHABLE`. `openssl s_client` against the live
  listener proved its certificate still omitted `10.0.0.151`.
- `remote_start` supplied only the selected bind IP to
  `TlsIdentity::load_or_create`, which returned an existing valid identity
  without refreshing its certificate. `remote_begin_pairing` independently
  rescanned interfaces later and advertised the bound IP plus then-current
  local addresses. The QR could therefore name hosts absent from the live
  certificate. Android retained OkHttp's default hostname verifier, so its
  refusal was the intended secure outcome.

### Secure correction

- Listener start now creates one preferred-order host vector: the selected bind
  address first, followed by every current shareable LAN/VPN address, with
  exact-IP deduplication. More than 16 unique hosts fails validation instead of
  creating an unbounded certificate or QR. `PairingUri` also rejects more than
  16 host fields.
- That vector is passed unchanged to `TlsIdentity` and stored in `RemoteState`
  for the gateway lifetime. `remote_begin_pairing` builds its payload only from
  this stored vector and never calls `local_addresses`; `remote_stop` clears it.
  An interface change therefore takes effect only after an explicit listener
  restart, keeping certificate SANs and every invite identical.
- A complete existing certificate/key pair is first parsed and validated by
  the established rustls server-config path, including public-key matching.
  Malformed, incomplete, or mismatched identity state returns an error before
  any write. A valid certificate is reissued only when rustls hostname
  validation finds a required IP or `localhost` missing, using the exact
  existing PKCS#8 private key. The SHA-256 SPKI fingerprint and key bytes remain
  unchanged, so remembered phones keep the same pin.
- The validated replacement certificate is persisted as a same-directory
  owner-only temporary file: create-new mode `0600`, write, file `fsync`, atomic
  rename over the certificate, then parent-directory `fsync` on Unix. Failure
  cleanup removes and syncs the temp namespace where possible. The private key
  is never opened for writing during refresh, and a write/rename failure leaves
  the prior certificate/key pair intact.
- The rustls listener remains explicitly TLS 1.3-only. No Android trust,
  hostname-verification, pinning, frame-limit, or authentication code changed.

### TDD checkpoints

RED checkpoint `00b1ddb` was followed by `sync`. The direct `x509-parser 0.18`
dev dependency supports real certificate SAN assertions.

```text
CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-red.SzEJum \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 --test remote_server \
  tls_identity_refreshes_certificate_sans_without_rotating_the_spki_pin \
  -- --exact --nocapture
FAILED: reissuing for a rebound listener must add the phone-reachable IP SAN

CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-red.SzEJum \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 --lib \
  pairing_invite_hosts_are_the_exact_frozen_start_host_list \
  -- --exact --nocapture
compile failed as intended: advertised_hosts function, Inner.advertised_hosts,
and Inner::pairing_payload were absent

CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-red.SzEJum \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 \
  --test remote_server tls_identity_rejects_more_than_sixteen_unique_advertised_hosts \
  -- --exact --nocapture
FAILED: an unbounded certificate identity must be rejected

CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-red.SzEJum \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 \
  --test remote_desktop a_pairing_uri_refuses_more_than_sixteen_advertised_hosts \
  -- --exact --nocapture
FAILED: the trust payload parser must not accept an unbounded host list
```

The fail-closed characterization
`mismatched_existing_certificate_and_key_fail_closed_before_san_refresh`
already passed at RED and remained green:

```text
CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-red.SzEJum \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 --test remote_server \
  mismatched_existing_certificate_and_key_fail_closed_before_san_refresh \
  -- --exact --nocapture
1 passed, 0 failed
```

GREEN checkpoint `54ea0c1` was followed by `sync`.

```text
CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-red.SzEJum \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 \
  --lib pairing_invite_hosts_are_the_exact_frozen_start_host_list -- --nocapture
1 passed, 0 failed

CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-red.SzEJum \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 \
  --test remote_desktop --test remote_server -- --test-threads=1
remote_desktop: 8 passed, 0 failed
remote_server: 42 passed, 0 failed
```

### Fresh Fix Round 5 verification

All final Rust commands used the newly created isolated target
`/var/tmp/aiterm-task9-round5-green.FfeNvE`, with incremental artifacts and
debug information disabled. The unsafe real-HOME backend target was not run,
HOME was not repurposed, preserved dumps were not inspected, and no desktop
listener, trust store, Android source, or `src/App.css` was touched.

```text
CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-green.FfeNvE \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 --lib \
  pairing_invite_hosts_are_the_exact_frozen_start_host_list -- --nocapture
1 passed, 0 failed

CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-green.FfeNvE \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 \
  --test remote_desktop --test remote_server -- --test-threads=1
remote_desktop: 8 passed, 0 failed
remote_server: 42 passed, 0 failed

CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-green.FfeNvE \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 --lib -- --test-threads=1
435 passed, 0 failed, 7 ignored (442 total)

CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-green.FfeNvE \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -j 2 \
  --test remote_auth --test remote_desktop --test remote_operations \
  --test remote_protocol --test remote_server --test remote_terminal \
  --test tab_registry --test terminal_screen -- --test-threads=1
214 passed, 0 failed

CARGO_TARGET_DIR=/var/tmp/aiterm-task9-round5-green.FfeNvE \
  CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo check -j 2
passed
```

An initial disposable target under `/tmp` hit that filesystem's quota during
dependency compilation before a test ran. It was deleted, and every required
RED/GREEN/final result above was rerun successfully under `/var/tmp`.

The desktop must now be rebuilt/restarted so its existing private key can issue
a certificate covering the frozen current hosts. Live QR scan and approval,
Unicode/input, resize/rotation, focus takeover, disconnect/reconnect, and device
revocation must all be retried before Task 9 can be marked complete.
