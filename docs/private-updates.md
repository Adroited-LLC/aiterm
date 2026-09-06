# Private app updates

GitHub's private **Adroited-LLC/aiterm-releases** repository stores release packages and `updates.json`. Source remains in the existing source repository. The invite service keeps a verified copy of each published release, served over HTTPS at `https://control.34-23-107-73.sslip.io:8443/updates`. Users and the service need no GitHub credentials. Only the publisher uses GitHub authentication.

## Users

Enter your personal invite code in **App updates** (desktop Settings; Android Desktops toolbar or main drawer). You need no GitHub account. Codes are saved once per device: Linux uses a mode-0600 file, Windows uses per-user DPAPI encryption, Android uses Android Keystore AES-GCM. Disconnect removes the saved code.

Connected apps check at startup and every six hours while open. Manual checks remain available. Downloaded packages must match the release manifest's SHA-256 and byte count; Android also verifies the application ID, version, and signing identity.

**Automatically install updates** is off by default, independently on each device. When enabled, Windows downloads and waits for AITerm to close before silently installing. Linux invokes the system package manager and may require administrator confirmation; running sessions continue and use the new version on next launch. Android downloads and opens its installer automatically, but Android still requires installation confirmation. A regular sideloaded app cannot bypass that system requirement.

The first updater-enabled builds need one manual install. Older apps cannot discover an updater they do not contain.

## Invite administration

Run `python3 update-service/invites.py --help`. The store contains only hashes; the separate output file contains the codes and is mode 0600. Create one code per person, usable on their devices. To revoke a person, remove their entry with the tool, then replace `/etc/aiterm-updates/invites.json` on the server (root:aiterm-updates, mode 0640). Revocation applies to the next request without a service restart. Revoked users keep the software they already installed but cannot check for or download more updates.

Never put codes or the local plaintext output file into source control, public release notes, or logs. The service accepts authorization only in the HTTP header and never logs credentials. Only assets explicitly named in the current manifest can be served.

## Publishing

Build all three packages. Increase versions and Android's version code. Preserve the Android signing key: a differently signed APK cannot update existing installs. Current bootstrap builds retain the existing debug signing identity; keep that key private and backed up. Build Windows with `scripts/build-windows-wsl.ps1 -Bundle` to embed a fresh WSL companion.

Run `scripts/publish-private-release.py --help`. It checks repository privacy, stages a draft, uploads a complete three-platform release and manifest, and verifies GitHub's size and SHA-256 digest for each asset. `--publish` makes it current only inside the private repository. `--cache-out /private/path` saves the verified release set.

Then run `scripts/deploy-update-cache.py /private/path --host matt@34.23.107.73 --identity ~/.ssh/google_compute_engine`. It rechecks every file, uploads the set, and switches the service's active release atomically. No GitHub token is sent to the service. A failure before activation leaves the previous complete release active.

The service binds only to 127.0.0.1:8090 behind Caddy. Its systemd definition and tests are in `update-service/`. Caddy routes `/updates/*` to it while preserving the existing relay routes. Keep older release caches for rollback; clients will never automatically downgrade.
