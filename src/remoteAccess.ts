/**
 * The Remote Access panel's decisions, kept out of the component.
 *
 * Everything here answers a question the desktop has to get right whether or
 * not anyone is looking at the screen: may this QR be shown, is it still
 * valid, has the user really asked to lock a phone out. A component that
 * decides those things while rendering can only be checked by rendering it,
 * and this repo tests logic, not pixels.
 */

/** Matches `ENROLLMENT_LIFETIME` in `src-tauri/src/remote/auth.rs`. The
 *  gateway is the authority; this only decides when to stop drawing a code
 *  the gateway has already retired. */
export const INVITE_LIFETIME_MS = 5 * 60 * 1000;

export interface RemoteStatus {
  /** Whether the gateway is listening. False means no socket exists at all. */
  enabled: boolean;
  address: string | null;
  port: number | null;
  /** SHA-256 of the listener's SPKI, base64url. The phone pins this. */
  fingerprint: string | null;
}

export interface ListenerConfig {
  address: string;
  port: number;
}

/** Keep the authoritative live address visible even during a transient
 * interface scan that no longer reports it. */
export function listenerAddressOptions(
  selected: string,
  discovered: string[],
): string[] {
  return selected && !discovered.includes(selected)
    ? [selected, ...discovered]
    : discovered;
}

/** Resolve the control values without letting interface discovery overwrite
 * either the live socket or a still-valid saved choice. */
export function preferredListenerConfig(
  status: RemoteStatus,
  addresses: string[],
  saved: ListenerConfig | null,
): ListenerConfig {
  if (status.enabled && status.address && status.port !== null) {
    return { address: status.address, port: status.port };
  }
  const savedAddress = saved && addresses.includes(saved.address) ? saved.address : null;
  return {
    address: savedAddress ?? addresses[0] ?? "",
    port: saved?.port ?? 8443,
  };
}

/** Move a live listener while keeping the old socket recoverable if the new
 * interface disappeared between discovery and bind. */
export async function rebindListener(
  current: ListenerConfig,
  target: ListenerConfig,
  stop: () => Promise<unknown>,
  start: (config: ListenerConfig) => Promise<RemoteStatus>,
): Promise<RemoteStatus> {
  await stop();
  try {
    return await start(target);
  } catch (targetError) {
    try {
      await start(current);
    } catch (rollbackError) {
      throw new Error(
        `${String(targetError)}; restoring ${current.address}:${current.port} also failed: ${String(rollbackError)}`,
      );
    }
    throw targetError;
  }
}

export interface PairingInvite {
  /**
   * The QR as an SVG, rendered by the desktop backend.
   *
   * The `aiterm://pair` payload never reaches the renderer as a string. A
   * secret that exists in JavaScript can end up in a devtools scope, a
   * clipboard, a crash report, or a screenshot of a React tree; one that only
   * ever existed as drawn geometry cannot. This image is the single value in
   * the panel that carries the secret at all — draw it, never serialize it.
   */
  svg: string;
  /** Epoch milliseconds, from the same clock the gateway expires it on. */
  expires_at: number;
}

export interface TrustedDevice {
  id: string;
  name: string;
  fingerprint: string;
  /** Epoch seconds, matching the Rust store. */
  created_at: number;
  last_seen_at: number | null;
  last_ip: string | null;
}

export interface PendingPairing {
  id: string;
  name: string;
  fingerprint: string;
  requested_at: number;
}

/**
 * The invite the panel may draw, or `null`.
 *
 * Disabling Remote Access closes the listener but does not, on its own, erase
 * a QR already on screen. Deciding here means a stale code cannot survive a
 * re-render: a secret shown after the socket is gone is one a user may still
 * scan, and it would fail in a way that looks like a broken phone.
 */
export function inviteToShow(
  status: RemoteStatus,
  invite: PairingInvite | null,
  now: number,
): PairingInvite | null {
  if (!status.enabled || !invite) return null;
  return now < invite.expires_at ? invite : null;
}

/**
 * Whole seconds left on an invite.
 *
 * Rounded down, never up: a countdown that reads 299 with 298.5 seconds left
 * promises validity the gateway will not honour, and the user finds out by
 * scanning a code that fails.
 */
export function inviteCountdownSeconds(invite: PairingInvite, now: number): number {
  return Math.max(0, Math.floor((invite.expires_at - now) / 1000));
}

/** Which device, if any, the revoke button is armed for after a click. */
export type RevokeStep = string | "confirmed";

/**
 * Revocation is two deliberate steps.
 *
 * The first click on a device arms that device and nothing else; the second
 * on the *same* device commits. Clicking a different row moves the question
 * rather than answering it, so a mis-aimed second click cannot lock out a
 * phone the user never meant to touch.
 */
export function nextRevokeStep(armed: string | null, deviceId: string): RevokeStep {
  return armed === deviceId ? "confirmed" : deviceId;
}

/**
 * A fingerprint grouped into four-character blocks.
 *
 * Pinning only works if a human actually compares the desktop's fingerprint
 * with the one the phone shows, and nobody compares a 43-character run of
 * base64 correctly. Grouping is the whole reason the check gets done.
 */
export function fingerprintLabel(fingerprint: string): string {
  if (!fingerprint) return "unavailable";
  return (fingerprint.match(/.{1,4}/g) ?? []).join(" ");
}

/** Plain-language "last seen", from the epoch-seconds the Rust store keeps. */
export function lastSeenLabel(device: TrustedDevice, now: number): string {
  if (device.last_seen_at === null) return "never connected";
  const seconds = Math.max(0, Math.floor(now / 1000 - device.last_seen_at));
  if (seconds < 60) return "just now";
  const units: [number, string][] = [
    [86_400, "day"],
    [3_600, "hour"],
    [60, "minute"],
  ];
  for (const [size, name] of units) {
    if (seconds >= size) {
      const count = Math.floor(seconds / size);
      return `${count} ${name}${count === 1 ? "" : "s"} ago`;
    }
  }
  return "just now";
}

/** Where the gateway can be reached, for the panel's status line. */
export function listenerLabel(status: RemoteStatus): string {
  if (!status.enabled) return "off";
  if (!status.address || status.port === null) return "starting";
  return `${status.address}:${status.port}`;
}
