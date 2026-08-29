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
- Task 9 is complete through `c692f62`: all seven mandatory Rust
  prerequisites are closed; Android has unlock-gated pinned authentication,
  bounded reconnect/correlation, descriptor and terminal transfer recovery,
  generation-owned cancellation, linearized lock/signing, bounded
  attach-event publication, exact tab/attachment selection and stale detach
  ownership, live revoke termination, typed session/agent/tab actions, and a
  native Compose terminal/drawer.
  Fresh evidence is 425 Rust library tests/7 ignored, 209 safe Rust integrations,
  75 desktop UI tests/build, 98 Android JVM tests, and 14/14 pinned-Pixel
  instrumentation tests; assemble, lint, install, and cold launch also pass.
  Only the user-interactive live pairing smoke remains a handoff step. Mouse reporting remains a future
  typed Rust-mode/RPC addition; Android does not synthesize raw mouse bytes.

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
