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
