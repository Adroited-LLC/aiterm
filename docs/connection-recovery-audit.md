# Remote connection and conversation recovery

Audited 2026-09-06 for Android 0.3.23 and desktop 0.10.86.

The review covered Android network callbacks, route selection and direct QUIC
upgrade, authentication, request cancellation and correlation, terminal
attachment recovery, conversation subscriptions and pagination, desktop
WebSocket dispatch, and the desktop relay connector.

## Additional failures fixed

- Late responses to abandoned requests could outlive Android's 64-entry
  correlation cache and terminate a healthy connection. The socket's monotonic
  issued-request range now recognizes old replies without an expiring cache.
- A response with the wrong operation name removed its waiter before validation,
  leaving the waiter unreachable during transport teardown. Validation now
  happens before removal.
- Switching A → B → A allowed the first A refresh to overwrite the newest
  refresh's loading/error state. A refresh generation now protects callbacks,
  including the legacy conversation fallback.
- Cancelled resource reads retained transport slots until timeout. Cancellation
  now abandons them; replies from replaced connections cannot update resources.
- The phone log showed repeated `UserNotAuthenticatedException` after the
  Keystore authentication window expired. That condition now locks AITerm and
  stops reconnect retries until authentication succeeds. Key policy and the
  five-minute window are unchanged.
- The desktop accepted fewer queued requests than Android could send. The
  bounded queue now accommodates Android's 64-request capacity, and WebSocket
  heartbeats are handled independently of application dispatch.
- A one-second desktop write deadline disconnected briefly stalled links. It
  is now 15 seconds, with a regression test covering backpressure beyond one
  second.
- The desktop relay connector could wait on its own full outgoing queue, or
  remain connected indefinitely to a silent relay/stalled writer. Control
  replies now go directly to the writer; writes and heartbeat silence have
  deadlines that feed the existing reconnect loop.

## Validation

- Android: 296 unit tests passed; debug APK, instrumentation APK, and lint passed.
- Desktop: 657 library tests passed; 17 existing opt-in tests remained ignored.
- New socket tests exercise a full request queue with a live WebSocket ping,
  queue overflow, delayed writes, relay heartbeat replies, and relay silence.
- This does not establish a long-duration Wi-Fi/cellular handover result on
  every device. Desktop changes activate on the next desktop process launch.
