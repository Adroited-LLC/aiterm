# Private app updates

Installers and `updates.json` live in **Adroited-LLC/aiterm-releases**, a private repository. Source remains in the existing source repository. Each published distribution release contains a complete manifest plus RPM, Windows installer (including the WSL backend), and Android APK. Publishing source commits alone does not release an update.

## Access

Grant a person read access to the private distribution repository. They create a fine-grained GitHub personal access token selecting only that repository, with **Contents: read-only**. Organization approval may be required. Enter it in **App updates** (desktop Settings; Android Desktops toolbar or main drawer). Do not paste tokens into issues, source, release manifests, or chat. Tokens are never included in a package. GitHub credentials used to publish releases are separate from client credentials.

Tokens are stored in the user's application data: Linux uses a mode-0600 file, Windows uses per-user DPAPI encryption, Android uses Android Keystore AES-GCM encryption. Disconnect removes the local saved credential. Revoke the token in GitHub to revoke access everywhere it was used. When a token expires, connect a replacement in App updates.

Connected apps check at startup and every six hours while open. Manual checks are always available. Downloads use authenticated GitHub asset API requests; authorization is not forwarded to GitHub's separate download host. Each downloaded file must match its manifest's SHA-256 and byte count. Android additionally checks package ID, version, and installed signing identity. Installation uses the operating system installer, with user confirmation; AITerm does not force a restart.

## Publish

Build all three packages, preserving the Android signing identity and increasing its version code. In particular, a release-signed APK cannot update existing debug-signed installations. The bootstrap packages use the existing signing identity; keep that signing key private and backed up. Build Windows with `scripts/build-windows-wsl.ps1 -Bundle` to include a fresh Linux companion.

Run `scripts/publish-private-release.py --help`. It requires explicit paths and versions, checks repository privacy, creates a draft, uploads all packages plus the manifest, and verifies GitHub's SHA-256 digest for every uploaded asset. Add `--publish` to make the verified release current **inside the private repository**. Without that flag it remains a draft and clients do not see it. Publish complete release sets so a Windows-only update does not hide the latest Android or Linux package.

The first updater-enabled builds must be installed once using the existing installer/ADB flow. Older apps cannot discover an updater they do not contain.
