import test from "node:test";
import assert from "node:assert/strict";
import {
  INVITE_LIFETIME_MS,
  fingerprintLabel,
  inviteCountdownSeconds,
  inviteToShow,
  listenerAddressOptions,
  relayLabel,
  preferredListenerConfig,
  rebindListener,
  lastSeenLabel,
  nextRevokeStep,
  type PairingInvite,
  type RemoteStatus,
  type TrustedDevice,
} from "./remoteAccess.ts";

const enabled: RemoteStatus = {
  enabled: true,
  address: "192.168.1.20",
  port: 8443,
  fingerprint: "n1oZ8kQ2xr7Yv0bDq3sTfLmE5wUcJhAaP9gRkNzXeIo",
};
const disabled: RemoteStatus = { ...enabled, enabled: false };

const invite = (issuedAt: number): PairingInvite => ({
  svg: "<svg viewBox='0 0 21 21'><rect width='21' height='21'/></svg>",
  expires_at: issuedAt + INVITE_LIFETIME_MS,
});

test("a pairing QR stays hidden until remote access is enabled", () => {
  const now = 1_000_000;
  assert.equal(
    inviteToShow(disabled, invite(now), now),
    null,
    "an invite issued while the listener is off must never be rendered",
  );
  assert.notEqual(inviteToShow(enabled, invite(now), now), null);
});

test("a pairing QR disappears the moment it expires", () => {
  const now = 1_000_000;
  const issued = invite(now);
  assert.notEqual(
    inviteToShow(enabled, issued, now + INVITE_LIFETIME_MS - 1),
    null,
  );
  assert.equal(
    inviteToShow(enabled, issued, now + INVITE_LIFETIME_MS),
    null,
    "the desktop must stop offering a secret the gateway has already retired",
  );
});

test("there is no QR to show before one is issued", () => {
  assert.equal(inviteToShow(enabled, null, 1_000_000), null);
});

test("the countdown never runs negative", () => {
  const now = 1_000_000;
  const issued = invite(now);
  assert.equal(inviteCountdownSeconds(issued, now), 300);
  assert.equal(inviteCountdownSeconds(issued, now + 1_500), 298);
  assert.equal(inviteCountdownSeconds(issued, now + INVITE_LIFETIME_MS * 2), 0);
});

test("revoking a device takes two deliberate steps", () => {
  // One tap must not be able to lock a phone out, and asking about one device
  // must not arm the button on another.
  assert.equal(nextRevokeStep(null, "device-a"), "device-a");
  assert.equal(nextRevokeStep("device-a", "device-a"), "confirmed");
  assert.equal(nextRevokeStep("device-a", "device-b"), "device-b");
});

test("a fingerprint is grouped so a person can compare it out loud", () => {
  const label = fingerprintLabel("n1oZ8kQ2xr7Yv0bDq3sTfLmE5wUcJhAaP9gRkNzXeIo");
  assert.equal(label, "n1oZ 8kQ2 xr7Y v0bD q3sT fLmE 5wUc JhAa P9gR kNzX eIo");
  assert.equal(fingerprintLabel(""), "unavailable");
});

test("a phone that has never connected does not read as recently seen", () => {
  const now = 1_700_000_000_000;
  const never: TrustedDevice = {
    id: "d1",
    name: "Pixel",
    fingerprint: "abc",
    created_at: now / 1000 - 10,
    last_seen_at: null,
    last_ip: null,
  };
  assert.equal(lastSeenLabel(never, now), "never connected");
  assert.equal(
    lastSeenLabel({ ...never, last_seen_at: now / 1000 - 30 }, now),
    "just now",
  );
  assert.equal(
    lastSeenLabel({ ...never, last_seen_at: now / 1000 - 3600 }, now),
    "1 hour ago",
  );
  assert.equal(
    lastSeenLabel({ ...never, last_seen_at: now / 1000 - 86_400 * 3 }, now),
    "3 days ago",
  );
});

test("the invite exposes drawn geometry, never the pairing payload", () => {
  // The backend renders the QR, so the enrollment secret has no string form
  // in the renderer at all. Everything else the panel shows — labels, device
  // rows, countdowns — must stay safe to put in an accessibility tree, a
  // screenshot, or a log.
  const now = 1_000_000;
  const shown = inviteToShow(enabled, invite(now), now);
  assert.ok(shown);
  assert.equal(
    Object.keys(shown).some((key) => /payload|secret|token/i.test(key)),
    false,
    "an invite must not grow a field that holds the secret as text",
  );
  assert.match(shown.svg, /^<svg/, "the invite is an image, not a URL");
});

test("the listener selector restores the live or saved address instead of the first interface", () => {
  const interfaces = ["192.168.1.10", "10.8.0.4"];
  const saved = { address: "10.8.0.4", port: 9443 };

  assert.deepEqual(
    preferredListenerConfig(
      { enabled: false, address: null, port: null, fingerprint: null },
      interfaces,
      saved,
    ),
    saved,
  );
  assert.deepEqual(
    preferredListenerConfig(
      { enabled: true, address: "10.8.0.4", port: 8555, fingerprint: "pin" },
      interfaces,
      { address: "192.168.1.10", port: 8443 },
    ),
    { address: "10.8.0.4", port: 8555 },
    "the socket that is actually listening is authoritative",
  );
});

test("the active listener remains selectable if interface discovery no longer returns it", () => {
  assert.deepEqual(
    listenerAddressOptions("10.8.0.4", ["192.168.1.10"]),
    ["10.8.0.4", "192.168.1.10"],
  );
  assert.deepEqual(
    listenerAddressOptions("192.168.1.10", ["192.168.1.10"]),
    ["192.168.1.10"],
  );
});

test("relay status explains route and connector state without implying a second backend", () => {
  assert.equal(relayLabel(enabled), "not configured");
  const withRelay: RemoteStatus = {
    ...enabled,
    relay: {
      configured: true,
      connector_url: "wss://control.relay.example.com/v1/connect",
      public_host: "desk-1234.relay.example.com",
      public_port: 443,
      route_id: "desk-1234",
      state: "connected",
    },
  };
  assert.equal(relayLabel(withRelay), "desk-1234.relay.example.com:443 — connected");
  assert.equal(relayLabel({ ...withRelay, enabled: false }), "desk-1234.relay.example.com:443 — off");
});

test("changing a running listener restores the old address when the new bind fails", async () => {
  let active = "192.168.1.10:8443";
  const current = { address: "192.168.1.10", port: 8443 };
  const target = { address: "10.8.0.4", port: 9443 };

  await assert.rejects(
    rebindListener(
      current,
      target,
      async () => { active = ""; },
      async (config) => {
        if (config.address === target.address) throw new Error("address unavailable");
        active = `${config.address}:${config.port}`;
        return { enabled: true, ...config, fingerprint: "pin" };
      },
    ),
    /address unavailable/,
  );

  assert.equal(active, "192.168.1.10:8443");
});
