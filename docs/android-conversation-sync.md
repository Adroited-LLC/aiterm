# Android conversation synchronization fix

Desktop 0.10.79 and Android 0.3.2 address the API view falling behind the terminal.

The desktop implemented `session.spine` but omitted it from `KNOWN_REQUESTS`.
The wire decoder rejected every request before dispatch; the gateway converted
that rejection to `remote.unsupported`, making Android silently use the older
`session.conversation` snapshots. Adding the request to the decoder's allowlist
makes the native event feed reachable over the authenticated gateway.

Android now refreshes a visible conversation whenever connected, independently
of tab discovery, and refreshes immediately after reconnecting. If the desktop
epoch changes, it discards the response filtered by the old cursor and requests
the new history from zero. Catch-up paging only continues immediately when a
page advances the cursor, preventing an empty inconsistent page from spinning.

The view follows batched updates relative to its previous list length rather
than the enlarged list, and text/tool upserts trigger the same scrolling logic.
Reading older rows or actively scrolling suppresses automatic following.

## Validation

- 230 Android JVM tests passed, including multi-page catch-up, desktop restart,
  nonadvancing pages, and following the previous tail.
- 95 spine unit tests and 17 gateway protocol tests passed. The protocol
  regression feeds a serialized `session.spine` request through `RemoteRequest::decode`.
- The optional real-rollout test replayed 102 local Codex transcripts successfully,
  including the conversation shown in the reported screenshots.
- Android debug APK and Linux x86_64 RPM built successfully.

The desktop must restart after installation to expose the fixed endpoint.
Installation preserves the running desktop process and phone pairing. Final
phone-to-desktop verification of the native feed requires that restart.

## Android 0.3.3: working indicator and wire compatibility

Testing after the desktop restart exposed a second interoperability issue:
Rust's `serde(flatten)` encodes each spine event as an indefinite-length CBOR
map. Android's strict validator rejected those maps, again falling back to
snapshots and losing native working status. The validator now accepts this map
encoding while retaining duplicate-key, nesting, item-count, and termination
checks. `cargo run --example spine_wire_fixture` generates the actual Rust
payload used in the Android regression test. That test failed against the old
validator and passes with the fix; all 233 Android tests pass.

Automatic scrolling also counts the crew strip at the beginning of the list,
so the target includes the working indicator instead of stopping one row short.

Verified on the connected Pixel after installing 0.3.3: the visible API view
shows “Codex is working…”, the header says “working”, and native tool rows show
running status. Desktop 0.10.79 remained running throughout this verification.

## Desktop 0.10.80 / Android 0.3.4: push and bounded source reads

`session.spine.subscribe` returns the same cursor page as `session.spine` and
subscribes the authenticated socket to that conversation. The gateway checks
the in-memory sequence every 100 ms and pushes a coalesced
`session.spine.changed {session_id, epoch, latest_seq}` notification with
request ID zero when it advances. No transcript is read for this check. There
is one subscription per connection, replaced when another conversation is
selected. Old phones receive no unsolicited new event unless they opt in.

Android immediately fetches after its last applied sequence. Notifications
arriving during a fetch queue another catch-up, and duplicates already applied
are ignored. A five-second refresh is a recovery check and compatibility path;
desktop restarts still refetch from zero. An unsupported subscription falls
back to ordinary spine polling. Malformed replies now surface a sync error
instead of silently switching to legacy snapshots.

The transcript watcher ignores read/access notifications and has a single
250 ms deadline for an entire burst. The former sliding timeout could wait
indefinitely when notifications continued arriving, preventing both new
content and completion status from being produced.

Validation includes continuous-notification starvation, coalesced/isolated
subscriptions, authenticated unsolicited notification delivery, notifications
racing in-flight fetches, duplicate suppression, and completed-turn delivery
without advancing the polling clock. All 236 Android tests and the Linux
635-test library plus 17 protocol tests passed with four Rust test threads.
Three unrelated session filesystem tests failed in an initial unrestricted
parallel run and passed on the limited-parallelism rerun. Windows/WSL includes
the same backend changes in preview 0.1.2.

## Android 0.3.5: composed prompt submission

The terminal composer queued paste and Enter together, while the API composer
waited only 75 ms after desktop acceptance. Codex's unbracketed paste detector
keeps Enter in newline mode for 120 ms. Both composers now use the acknowledged
submission path, with a 250 ms settling interval. A submission stays bound to its
original transport and attachment and stops if focus is lost. The terminal
composer retains its draft until acceptance and prevents another submission
while the first is pending. Raw terminal keys remain immediate.

Validation: 237 Android unit tests passed, debug APK built, and Android UI test
sources compiled. A standalone harness against upstream Codex paste_burst.rs
reproduced newline classification at 75 ms and submission eligibility at 250 ms.
This is a timing regression check, not an end-to-end guarantee under arbitrary
receiver stalls. Phone prompt submission still needs user confirmation.
Source: https://raw.githubusercontent.com/openai/codex/main/codex-rs/tui/src/bottom_pane/paste_burst.rs

## Android 0.3.6: first pairing to a private WSL VM

Adds support for version-4 WSL pairing codes containing public relay enrollment
commitments. The phone validates the digest binding the control origin, route,
connector-token hash, and desktop TLS identity before signing. Existing version
1–3 codes and direct-first pairing behavior remain supported. Only after direct
endpoints fail does the phone provision the route, then attempt the pinned relay
connection. Provisioning never sends the connector token or pairing secret to
the relay and never substitutes for desktop approval.

Validation: 244 Android unit tests passed and APK built. New tests cover QR
binding/tampering, HTTPS provisioning payloads, route-limit rejection, repeated
registration, direct-path preservation, required desktop approval, and rejection
of a substituted desktop TLS identity after relay provisioning. An
isolated WSL service completed a real relay enrollment and approval exchange;
its temporary relay route was deleted afterward (HTTP 204). The installed Linux
desktop and Linux Rust sources were not changed for this fix.

## Memory references in assistant replies

Android 0.3.27 renders a recognized Codex memory-citation footer as a collapsed Memory references section. Expanding it shows source file/line references and their notes, without the raw XML wrapper or rollout identifiers. Live spine rows, history fallback, and Copy/Share use the same parsing. User messages, code examples, unsupported markup, and incomplete footers remain unchanged; stored and transported conversation text is preserved.

Validation: all 322 Android unit tests, APK assembly, and lint passed. The five new regressions cover the reported footer, multiple source ranges, malformed/incomplete input, code examples, and assistant-only Copy/Share normalization. No device installation was performed.

### Android session status and scroll stability (0.3.29)

The dashboard's three-second roster refresh used terminal byte cadence as its
working verdict. Idle Codex terminals repaint, so opening a conversation briefly
corrected the row before the next roster response replaced it with `output`.
Android now reads the existing spine endpoint's atomic turn gate for running
sessions. A metadata-only cursor avoids downloading history; when its sequence
changes, a bounded recent tail supplies explicit permission phases. These reads
use `session.spine`, never replace the selected subscription, and keep their
status cursors separate from the conversation/outbox cursors. Requests are
deduplicated and time out after eight seconds. Sequence checks reject older
replies; reconnects reset status caches. Unknown status stays open rather than
claiming work from terminal redraws.

Both views use the same phase rule. A closed native turn is idle; an open native
turn stays working through quiet periods. Older desktop phase details `approval`
and `a tool call is waiting` are transcript timeout guesses, so they do not alone
raise Needs you. Explicit permission phases remain visible. This cannot identify
an approval that the desktop reports only through that timeout heuristic; its
terminal remains available for inspecting and answering the prompt.

The API conversation keeps its status/approval controls in a fixed-height area
above the composer, outside the message list. Phase-only updates neither add or
remove message rows nor trigger follow-to-newest scrolling. Regression coverage
includes repeated roster refreshes, starting/completing turns, stale responses,
permission retention, desktop epochs, partial history, and message/composer
geometry at the newest message and while reading older context.

Validation: 341 Android unit tests, lint, and APK assembly passed. All four
SessionNavigationTest device tests passed on the Pixel 10 Pro XL, including
repeated Working/Needs you/Idle geometry checks at the newest message and
in older history. Pixel upgraded in place to 0.3.29/code 32; pairing preferences
were byte-for-byte unchanged. No desktop update or restart was required.
