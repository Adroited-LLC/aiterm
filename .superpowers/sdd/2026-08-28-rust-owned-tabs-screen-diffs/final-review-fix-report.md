# Final Review Fix Report: Rust-Owned Tabs and Screen Diffs

Date: 2026-08-29

Branch: `feature/android-remote`

Fix base: `a68b0c6`

Source commit: `28f7b48` (`fix(tabs): reconcile global registry and screen bounds`)

Binding plan: `docs/superpowers/plans/2026-08-28-rust-owned-tabs-screen-diffs.md`

Binding spec: `docs/superpowers/specs/2026-08-28-android-remote-client.md`

## Status

All three Important final-review findings and the deferred Minor finding are fixed in one scoped TDD wave. The safe automated Rust, remote transport, renderer, build, formatting, and diff checks are green.

The HOME-sensitive backend integration suite was intentionally not run because its harness uses the real HOME. The KDE/manual smoke was not run and is not claimed.

## Finding 1: bound resident combining-character storage

### Review verification

The finding was valid. `ScreenModel` previously sent PTY bytes directly to Alacritty Terminal 0.26's parser. Alacritty stores zero-width characters in the cursor cell's resident `CellExtra.zerowidth: Vec<char>`, with no upstream length bound. The existing 32-scalar limit existed only while converting an emulator cell into a wire `ScreenCell`; it therefore did not bound emulator memory.

### Fix

`src-tauri/src/terminal/screen.rs` now owns an exact-version `BoundedProcessor` adapter around Alacritty's parser:

- ASCII runs still pass through unchanged.
- Non-ASCII bytes are advanced one byte at a time through the upstream parser, preserving UTF-8 and escape-sequence parser state.
- After each byte, the adapter locates the cell that exact Alacritty Terminal 0.26 zero-width input can extend, using the upstream cursor/previous-cell rule.
- If resident zero-width storage exceeds 32 scalars, the cell is rebuilt with the first 32 while preserving the base character, foreground/background colors, flags, underline color, and hyperlink.
- The wire projection keeps its existing defensive cap.

This is intentionally isolated as an exact-version compatibility adapter. It does not rewrite PTY bytes, infer Unicode before the parser, or silently consume escape-state bytes.

### TDD evidence

RED:

```text
CARGO_TARGET_DIR=/tmp/aiterm-android-rust-target cargo test --lib terminal::screen::tests::processor_bounds_resident_combining_storage_per_cell -- --exact --nocapture
```

The new test inspected `screen.term.grid()[Line(0)][Column(0)].zerowidth().len()` directly and failed with resident length `4096`, expected `32`.

GREEN:

```text
CARGO_TARGET_DIR=/tmp/aiterm-android-rust-target cargo test --lib terminal::screen::tests::processor_bounds_resident_combining_storage_per_cell -- --exact --nocapture
```

Result: `1 passed; 0 failed`.

The final `terminal_screen` integration suite also passed `30/30`.

## Finding 2: process-wide bounded recoverable tab-change stream

### Review verification

The finding was valid. The prior exit-only desktop stream and attachment-local remote events could not keep independent desktop and authenticated remote roster projections coherent. There was no process-wide ordered change source or overflow recovery point.

### Registry architecture

`src-tauri/src/tabs.rs` now provides a process-wide registry stream with monotonically increasing revisions and these events:

- `Snapshot { revision, tabs }`
- `Opened { revision, tab }`
- `Changed { revision, tab }`
- `Removed { revision, tab_id, requested }`

Each subscriber has a bounded `RegistryMailbox`. If a subscriber falls behind, its stale queued events are atomically replaced by a current authoritative snapshot. Initial subscription and its snapshot are created under the registry maps lock, so no mutation can be lost between snapshot capture and subscription. Dropping a receiver removes its weak subscriber entry immediately.

The registry's authoritative descriptor roster and revision are updated and published for open, metadata update, desktop focus attachment, resize, focus transfer, detach, title change, unexpected process exit, and requested removal.

### Desktop bridge and reconciliation

`src-tauri/src/lib.rs` installs a registry bridge and exposes the snapshot command. The bridge emits `tab://registry` to the renderer. Unexpected exits additionally preserve the legacy `tab://exit` notice; requested removals do not masquerade as an unexpected exit.

`src/tabModel.ts` applies revisions deterministically. A revision gap leaves partial local state untouched and requests an authoritative snapshot. `src/App.tsx` installs the listener before requesting its initial snapshot, reconciles Rust descriptors with local presentation data, removes remotely closed tabs from the local roster and ended/file state, and selects an active fallback. A requested remote close therefore does not create an ended desktop tab.

### Authenticated remote bridge and privacy

Every authenticated remote connection subscribes to the same registry stream:

- Authentication starts with a `state.snapshot` recovery point.
- `Opened`, `Changed`, and `Removed` become `tab.changed` frames.
- Subscriber overflow produces a fresh `state.snapshot` rather than a partial sequence.

The transport descriptor exposes only `desktop`, `remote`, or `unowned` focus plus canonical dimensions. The internal input-owner attachment ID remains skipped during serialization. No numeric PTY identifier or another connection's attachment identifier is included.

### Requested versus unexpected termination

- Explicit close removes the tab and publishes `Removed { requested: true }`.
- Unexpected child exit keeps the tab recoverable in the roster and publishes `Changed` with exited state and exit details.
- Snapshot recovery reconstructs unexpected-exit notification state, while a requested removal never produces an exit notification.

### Deterministic coverage

New registry integration tests prove:

- `registry_change_stream_recovers_overflow_with_a_current_snapshot`
- `remote_open_and_close_are_process_wide_requested_roster_changes`
- `desktop_descriptor_projects_safe_focus_owner_and_canonical_dimensions`
- `registry_stream_projects_focus_size_title_and_unexpected_exit_as_changes`

New real TLS/WebSocket gateway tests prove:

- `authenticated_connection_starts_with_a_recoverable_tab_state_snapshot`
- `desktop_open_and_update_emit_authenticated_remote_tab_changes`
- `remote_open_and_close_drive_the_desktop_registry_projection`

Renderer reducer tests prove remote open/requested close projection and revision-gap recovery. Existing gateway tests were made tolerant of valid interleaved global `tab.changed` frames while retaining their correlated-response and transfer-order assertions.

### TDD evidence

Initial renderer RED:

```text
npm run test:ui
```

Result after adding the roster/revision tests but before the reducer: `62 passed; 1 failed`; `applyTabRegistryEvent` was missing.

GREEN after the minimal reducer and bridge implementation was `70 passed; 0 failed`; the final amended renderer suite is `72 passed; 0 failed`.

Additional focused receiver-lifecycle RED:

```text
CARGO_TARGET_DIR=/tmp/aiterm-android-rust-target cargo test --lib tabs::tests::dropping_a_registry_change_receiver_removes_its_idle_subscriber_entry -- --exact --nocapture
```

The new assertion failed because the idle subscriber entry remained. After receiver-drop cleanup, the same command passed `1/1`.

The snapshot/unexpected-exit recovery test was also written before its helper existed, observed failing at compilation, then passed `1/1` after the minimal helper was added.

## Finding 3: enforce authoritative focus owner and dimensions in xterm

### Review verification

The finding was valid. Rust correctly rejected a desktop resize while a phone owned focus, but `FitAddon.fit()` could already have resized and reflowed the local xterm grid. That made xterm's state diverge from the canonical Rust grid even though the IPC resize failed.

### Fix

`src/terminalSizing.ts` defines a pure `projectTerminalGrid` policy:

- Desktop owner: use fitted dimensions and publish the resize to Rust.
- Remote or unowned: use Rust's canonical dimensions and do not resize the backend.

`TerminalView` now receives the authoritative focus owner and canonical size. Every prior fit path (mount, redraw, settings, renderer change, active-tab change, and `ResizeObserver`) goes through that policy. Non-owner xterm instances are resized to the broadcast canonical grid; FitAddon-driven local reflow and backend resize are suppressed.

Explicit desktop focus acquisition uses `FitAddon.proposeDimensions()` without applying a local reflow, asks Rust to transfer focus using those proposed dimensions, and only switches the local projection after Rust accepts. When focus becomes remote, xterm's hidden textarea is blurred so a later desktop click is an explicit reacquisition.

### TDD evidence

RED:

```text
node --experimental-strip-types --test src/terminalSizing.test.ts
```

Result: failed with `ERR_MODULE_NOT_FOUND` for `terminalSizing.ts`.

GREEN:

```text
node --experimental-strip-types --test src/terminalSizing.test.ts
```

Result: `2 passed; 0 failed` for remote canonical sizing and desktop-owned fitted sizing.

Rust coverage in `desktop_descriptor_projects_safe_focus_owner_and_canonical_dimensions` proves the projected owner and dimensions and asserts that internal owner/attachment identifiers are absent.

## Minor: drop a failed desktop forwarding receiver

### Review verification and fix

The finding was valid. The forwarding loop ignored `Channel::send` failure and could retain a receiver after the renderer channel was gone. The raw desktop forwarding helper now returns immediately on send failure. Dropping the receiver follows the existing cancellation path, closes the mailbox, and detaches the attachment.

### TDD evidence

RED:

```text
CARGO_TARGET_DIR=/tmp/aiterm-android-rust-target cargo test --lib tabs::tests::failed_desktop_channel_send_closes_the_attachment_receiver -- --exact --nocapture
```

The test initially failed to compile because the forwarding helper did not exist.

GREEN: the same exact command passed `1/1`; subsequent mailbox pushes observe the closed receiver.

## Final verification

All final verification below ran after the complete implementation.

### Rust library and relevant integration suites

```text
CARGO_TARGET_DIR=/tmp/aiterm-android-rust-target cargo test --lib --test remote_protocol --test remote_auth --test remote_server --test remote_terminal --test remote_desktop --test tab_registry --test terminal_screen
```

Results:

| Suite | Passed | Ignored | Failed |
| --- | ---: | ---: | ---: |
| Rust library | 394 | 7 | 0 |
| `remote_auth` | 11 | 0 | 0 |
| `remote_desktop` | 7 | 0 | 0 |
| `remote_protocol` | 14 | 0 | 0 |
| `remote_server` | 35 | 0 | 0 |
| `remote_terminal` | 31 | 0 | 0 |
| `tab_registry` | 50 | 0 | 0 |
| `terminal_screen` | 30 | 0 | 0 |
| **Total safe automated Rust** | **572** | **7** | **0** |

The full library summary was independently rerun with:

```text
CARGO_TARGET_DIR=/tmp/aiterm-android-rust-target cargo test --lib --quiet
```

Result: `394 passed; 0 failed; 7 ignored`.

### Renderer tests

```text
npm run test:ui
```

Result: `72 passed; 0 failed`.

### Rust check

```text
CARGO_TARGET_DIR=/tmp/aiterm-android-rust-target cargo check
```

Result: passed.

### Production renderer build

```text
npm run build
```

Result: TypeScript and Vite production build passed; 2,573 modules transformed. Vite emitted only its existing advisory that a generated chunk exceeds 500 kB.

### Scoped formatting and diff checks

```text
rustfmt --edition 2021 --config skip_children=true --check \
  src/lib.rs src/remote/server.rs src/tabs.rs src/terminal/screen.rs \
  tests/remote_server.rs tests/tab_registry.rs
```

Run from `src-tauri`; result: passed with no output.

```text
git diff --check
git diff --exit-code -- src/App.css
```

Both passed. Upstream CSS is unchanged.

## Files changed

- `src-tauri/src/lib.rs`
- `src-tauri/src/remote/server.rs`
- `src-tauri/src/tabs.rs`
- `src-tauri/src/terminal/screen.rs`
- `src-tauri/tests/remote_server.rs`
- `src-tauri/tests/tab_registry.rs`
- `src/App.tsx`
- `src/components/TerminalView.tsx`
- `src/ipc.ts`
- `src/tabModel.ts`
- `src/tabModel.test.ts`
- `src/terminalSizing.ts`
- `src/terminalSizing.test.ts`
- This report

## Self-review

- Re-read all final-review findings against the completed diff.
- Confirmed the combining test observes resident emulator storage, not serialized output.
- Confirmed the parser adapter advances every original byte through Alacritty and does not interpret escape syntax.
- Confirmed all registry producers publish after authoritative state changes and subscriptions begin with an atomic recovery snapshot.
- Added immediate dead-subscriber cleanup and snapshot restoration of unexpected-exit notification during self-review.
- Confirmed requested removal and unexpected exit take different event/reconciliation paths.
- Confirmed remote descriptors omit internal input owner and attachment identity, and no PTY IDs were added.
- Confirmed every TerminalView fit/reflow entry point is gated by authoritative focus.
- Confirmed the failed desktop channel path drops the receiver and uses existing detach cancellation.
- Used scoped Rust formatting only; no broad `rustfmt` was run.
- Confirmed the source commit contains exactly the 13 intended source/test paths.

## Concerns and intentional omissions

- No known functional concern remains in this fix scope.
- `BoundedProcessor` deliberately tracks exact Alacritty Terminal 0.26 cell behavior; an Alacritty upgrade must revalidate that small compatibility adapter and its resident-storage regression.
- The Vite chunk-size advisory remains; it is unrelated to this fix wave and does not fail the production build.
- `cargo test --test backend` was intentionally not run because the backend integration harness uses real HOME. HOME was never reassigned or repurposed.
- The KDE/manual smoke was not run and is not claimed.
- The four protected untracked Android Task 8 paths were not read, edited, staged, or deleted. Android tests were outside this Rust/desktop fix wave.
- Preserved OpenCode dumps were not touched.
