# SDD ledger — plan: docs/superpowers/plans/2026-08-28-android-remote-client.md

## Resume state

- Tasks 1–4 are complete in commits `54d9b6e`, `0c29627`/`7ae5246`, `779cfa7`, and `2fc0b9c`.
- Task 5 shared session/agent services are implemented through `23a6ee4` and
  exhausted the five-round task-review loop. The public RPC/service boundary,
  shared managed graph, typed launches, bounded discovery, concurrent resume,
  OpenCode exact-object transaction, and leased archive foundation are
  implemented and verified (415 library tests/7 ignored; 204 safe integration
  tests). The breaker left four real load-bearing filesystem findings, ruled
  mandatory Task 9 prerequisites: exact source/destination protection through
  retirement, held-FD sidecar metadata, descriptor-exact strict restore/archive
  removal, and strict rollout/task/job purge correctness. Task 5 is complete
  with these four findings explicitly carried; they are not dismissed.
- Tasks 6–7 are complete in `fa9c4e2`/`5d1dda9` and `9a05126`.
- The Rust-owned-tabs/screen-diff subplan is implemented through `9270f90`. Resident combining storage, canonical desktop focus sizing, requested removal, registry revisioning/overflow, and failed-send detach are implemented and verified. Three roster prerequisites remain mandatory for Task 9: lossless desktop recovery interleaving, intrinsically sub-1 MiB remote roster recovery, and fair/bounded registry-event scheduling.
- Task 8 is complete and approved in `2eb5bf9`, `32cb65e`, and `a4c8672`. Android now has strict QR parsing, pinned TLS 1.3, challenge-first one-shot enrollment, remembered paired-desktop storage, Keystore-backed device identity, biometric/device-credential locking, and native Compose pairing UI. Verification: 60 JVM tests, assemble/lint/release-manifest checks, and 12/12 instrumentation tests on the pinned Pixel; install and cold launch succeeded. Physical API 26–29, camera scanning, interactive biometric/PIN, and live desktop approval remain manual compatibility checks rather than Task 8 blockers.
- Device ruling: the only permitted ADB target is `10.0.0.115:37713`, verified as Google Pixel 10 Pro XL, API 37, 1344×2992. Every ADB command must use `-s 10.0.0.115:37713`; Gradle instrumentation must set `ANDROID_SERIAL=10.0.0.115:37713`. Never use an unqualified device command because a watch may also advertise wireless debugging.
- Ruling: Task 9 uses the typed `TerminalScreenStore` snapshot/diff contract documented in the plan; references to a native byte emulator are superseded. Cost if wrong: Android rendering would be implemented against an obsolete raw-byte architecture forbidden by the spec.
- Safety ruling: never run the desktop backend target against real HOME and never repurpose HOME. The KDE interactive desktop smoke remains human-only.
- Task 9 implementation is complete through `c692f62` and its formal review
  loop is active: all seven mandatory Rust prerequisites are closed; Android has unlock-gated pinned authentication,
  bounded reconnect/correlation, descriptor and terminal transfer recovery,
  generation-owned cancellation, linearized lock/signing, bounded
  attach-event publication, exact tab/attachment selection and stale detach
  ownership, live revoke termination, typed session/agent/tab actions, and a
  native Compose terminal/drawer.
  Fresh evidence is 425 Rust library tests/7 ignored, 209 safe Rust integrations,
  75 desktop UI tests/build, 98 Android JVM tests, and 14/14 pinned-Pixel
  instrumentation tests; assemble, lint, install, and cold launch also pass.
  Mouse reporting remains a future typed Rust-mode/RPC addition; Android does
  not synthesize raw mouse bytes.
- Task 9: minor (deferred): the eager mixed drawer can push session actions
  offscreen near the 128-tab bound; use one lazy drawer list in a later polish
  pass unless another Task 9 fix naturally touches it.
- Task 9: fix round 1/5 (4 addressed, 8 open — archive recovery cleanup
  timing; restore check-to-truncate; full-queue terminal notification;
  scrollback selection cleanup; render-area sizing; user-interactive live
  smoke; outbound enqueue/close race; transport/client lock inversion;
  commits `ed8f0a0`..`49b2894`).
- Binding strict-restore ruling: Linux cannot atomically validate multiple
  destination path bindings and destructively retire an archive. Successful
  strict restore therefore retains a bounded-name, descriptor-bound, full-byte
  hidden recovery object discoverable by the internal restore contract. It is
  removable only by explicit exact-object purge. Cost: retained recovery data
  and directory metadata consume disk after restore; benefit: concurrent path
  mutation cannot turn a reported restore into unrecoverable transcript loss.
- Task 9: fix round 2/5 (6 addressed, 3 open — full-queue transport
  completion must clear stale client state; retained restore aliases exhaust
  the exact-purge enumeration bound; user-interactive live
  QR/Unicode/resize/reconnect/focus/revoke smoke; commits
  `49b2894`..`b751f45`). Fresh evidence: 432 Rust library tests/7
  ignored, 210 safe Rust integrations, 111 Android JVM tests, and 17/17
  pinned-Pixel instrumentation tests; cargo check, assemble, lint, install,
  and cold launch also pass.
- Task 9: fix round 3/5 (1 addressed, 2 open — terminal-cause ordering lets
  pending-request failure reconnect before a queued Revoked outcome; the
  user-interactive live QR/approval/unlock/Unicode/resize/focus/reconnect/
  revoke smoke; commits `b751f45`..`72b89d5`). Fresh evidence: 434 Rust
  library tests/7 ignored, 210 safe Rust integrations, 113 Android JVM tests,
  and 17/17 pinned-Pixel instrumentation tests; isolated cargo check, assemble,
  lint, install, and cold launch also pass.
- Task 9: fix round 4/5 (1 automated finding addressed, 1 deliberately open —
  the user-interactive live QR/approval/unlock/Unicode/resize/focus/reconnect/
  revoke smoke; commits `d7d690e`..`7afe008`). Fresh evidence: 33/33 focused
  transport/client tests, 114/114 Android JVM tests, and 17/17 pinned-Pixel
  instrumentation tests; assemble, lint, one-device install, and cold launch
  also pass. The retained-recovery ruling remains binding; eager drawer is the
  deferred minor and mouse remains the future typed-contract gap.
- Task 9 automated review gate: clean through `4ced20b`; all automated
  Critical/Important findings are addressed with no new breakage. Task 9
  subsequently exposed a live TLS hostname-coverage defect during the
  security-sensitive, user-interactive pairing smoke.
- Task 9: fix round 5/5 (live stale-certificate SAN defect addressed;
  user-interactive QR/approval/Unicode/resize/focus/reconnect/revoke retry still
  open; RED `00b1ddb`, GREEN `54ea0c1`). Listener start now freezes one bounded
  host list for both certificate and invites, and certificate refresh retains
  the existing private key/SPKI pin with fail-closed validation and durable
  atomic persistence. Fresh evidence: frozen-state unit 1/1, remote desktop and
  server 50/50, 435 Rust library tests/7 ignored, 214 safe Rust integrations,
  and `cargo check` passed. Task 9 is not complete until the rebuilt desktop and
  pinned Pixel pass the full live interoperability smoke.

## Task 5 review breaker rulings

- Task 5: fix round 5/5 (OpenCode exact-object binding, archive creation,
  streaming bounds, and sidecar failure ordering addressed; four filesystem
  findings open; commits `d5f97da`..`23a6ee4`).
- Ruling: archive/source mutation protection through retirement is real and
  load-bearing because Android will expose session deletion. Carry it into
  Task 9 before delete/restore UI is enabled.
- Ruling: sidecar metadata must come from held FDs, strict restore/removal must
  remain descriptor-bound, and permanent purge must include strict rollout
  archives and propagate task/job errors. Carry all three into Task 9.
- Task 5: complete through `23a6ee4` with 4 parked-but-mandatory Task 9
  prerequisites after the five-round breaker; no finding is silently waived.
