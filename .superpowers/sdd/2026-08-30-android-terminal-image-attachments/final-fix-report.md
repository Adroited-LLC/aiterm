# Final Important-fix report — Android terminal image attachments

Date: 2026-08-31  
Branch: `fix/android-ime-submit`

## Outcome

Both Important final-review findings are fixed.

1. Android retains every begun upload id until the whole image submission succeeds. Rust retains a bounded, connection-local member record after `finish`, so any begun member—including a finished member—can cancel the incomplete submission. Cancellation is idempotent after the group closes, and the per-connection upload mutex serializes finish/cancel races. The member records are bounded by the existing `MAX_CLOSED_SUBMISSIONS` connection limit and four-member submission limit.
2. Compose reads the latest `ScreenSnapshot` after image upload completion, verifies that it still belongs to the submitting tab, and only then formats and locally submits the terminal input batch. The final bracketed-paste framing therefore follows the latest same-tab terminal mode while the prompt/path element remains ordered before `\r`.

The final-fix rereview exposed one remaining ordering race: detach could close attachment authorization before Android's retained-member cancel arrived. A successful sequenced detach now synchronously aborts the matching tab/attachment submission before the connection processes another request. A simultaneous disconnect may subsequently call whole-connection cleanup; both paths are idempotent. The already-published first member remains manifest-owned and is not removed.

Successfully published members are not deleted by group cancellation; the existing manifest/24-hour attachment lifecycle owns those files.

## TDD red evidence

Production code was unchanged when these regressions first ran.

- Android client:
  - Command: `cd android && ./gradlew testDebugUnitTest --tests 'com.adroited.aiterm.remote.RemoteClientTest.finishedFirstImageIsCancelledAfterFocusLossAndRetrySucceedsOnTheSameConnection'`
  - Result: failed as intended. Expected `begin, chunk, finish, cancel`; actual requests stopped at `begin, chunk, finish`, proving the completed id had been discarded before failure cleanup.
- Rust gateway:
  - Command: `cd src-tauri && cargo test --test remote_server terminal_upload_cancel_remains_authorized_after_focus_loss -- --exact --nocapture`
  - Result: failed as intended. The finished-member cancel returned an error payload (`unknown field code, expected ok`), proving the connection group was no longer cancellable through that member.
- Rust detach lifecycle rereview:
  - Command: `cd src-tauri && cargo test --test remote_server terminal_upload_detach_releases_incomplete_submission_before_late_cancel -- --exact --nocapture`
  - Result: failed as intended after detach and the late best-effort cancel. The fresh retry on the same WebSocket returned `error` instead of `terminal.upload.begin`, proving detach had left the old submission active.
- Delayed Compose submission:
  - Command: build/install the debug and test APKs with `adb install -r`, then run `TerminalScreenTest#delayedImageSubmissionUsesTheLatestSameTabBracketedPasteMode`.
  - Result: failed as intended. Expected an unframed prompt after mode changed to off; actual input still contained literal `ESC[200~` / `ESC[201~`, proving the pre-upload mode was captured.

## Green evidence

- Exact Rust gateway regression: 1 passed.
- Exact detach-before-cancel gateway regression: 1 passed; it also verified that the published first JPEG retained its original bytes before and after retry.
- Exact Android client regression: `BUILD SUCCESSFUL`.
- Exact delayed Compose regression on paired Pixel: `OK (1 test)`.
- `cd src-tauri && cargo test --test remote_uploads`: 62 passed, 0 failed, 2 subprocess helpers ignored.
- `cd src-tauri && cargo test --test remote_server terminal_upload -- --nocapture`: 5 passed, 0 failed.
- `cd src-tauri && cargo check --lib`: passed.
- `cd android && ./gradlew testDebugUnitTest --tests 'com.adroited.aiterm.remote.RemoteClientTest'`: passed.
- Three focused Compose submission cases on the paired Pixel (delayed mode change, ordered successful submission, failed-upload draft retention): `OK (3 tests)`.
- `cd android && ./gradlew lintDebug assembleDebug assembleDebugAndroidTest`: `BUILD SUCCESSFUL`.
- Debug and test APKs were installed in place with `adb install -r` / `adb install -r -t`; the paired main package was not uninstalled.
- `git diff --check`: passed before commits.

## Files changed

- `src-tauri/src/remote/uploads.rs`
- `src-tauri/src/remote/server.rs`
- `src-tauri/tests/remote_server.rs`
- `android/app/src/main/java/com/adroited/aiterm/remote/RemoteClient.kt`
- `android/app/src/test/java/com/adroited/aiterm/remote/RemoteClientTest.kt`
- `android/app/src/main/java/com/adroited/aiterm/ui/TerminalScreen.kt`
- `android/app/src/androidTest/java/com/adroited/aiterm/ui/TerminalScreenTest.kt`

## Commits

- `85be428 fix(remote): release interrupted image submissions`
- `d7a9e4a fix(android): resolve paste mode after image uploads`
- `ef78d25 fix(remote): release uploads when attachments detach`

## Residuals and exclusions

- The full Rust suite was intentionally not rerun. The final review already records an unsafe real-home `backend.rs` fixture and a known hanging live-PTY `remote_server` fixture; focused upload suites and a library check were used instead.
- Full device dogfood of camera/gallery capture and a restarted live desktop gateway remains outside this fix pass; the paired Pixel ran the focused Compose instrumentation against in-place APK installs.
- Repository-wide `cargo fmt --check` is red on extensive pre-existing formatting drift in unrelated Rust files. No broad formatter rewrite was performed; the changed diff passes `git diff --check` and the affected Rust targets compile and test.
- Existing residuals from the final review remain: API 26–27 normalization lacks runtime coverage, and non-`O_TMPFILE`/non-Linux publication plus non-baseline JPEG inputs fail closed by design.
