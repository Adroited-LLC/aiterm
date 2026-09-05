# Windows / WSL app

The Windows build mounts the same `src/App.tsx` as Linux. Its new-session menu,
session sidebar, terminal tabs, file editor, repository panels, home dashboard,
and full settings panel are shared components. Settings adds a Windows page
showing the connected distribution and its home folder.

Sessions, agent discovery/configuration, provider models, usage, tasks,
Librarian, transcript indexing, file watching, and session lifecycle commands
use the actual Linux backend modules running inside WSL. There is no separate
Windows session implementation to maintain. Windows owns its window, fonts,
tray menu, waiting indicator, notifications, and native font-file chooser.
The project folder chooser browses Linux.

## Running

The preview requires Windows 11, WebView2, and a configured WSL 2 distribution
with a normal Linux user and working login shell. Ubuntu 24.04 x86-64 is the
build/test baseline; the companion uses Ubuntu's glibc. Other distributions and
architectures have not been validated.

The installer embeds the companion. Users do not need Rust, Node.js, SSH,
an open network port, or a separately installed companion to run aiterm.
Windows copies the executable into
`~/.local/share/aiterm/backends/<sha256>` and starts it through `wsl.exe`.
The app uses the default distribution for its lifetime; restart aiterm after
changing that default. Agents and their credentials live in Linux.

Closing the window closes the service and its terminal sessions. It does not
stop WSL or other applications in the distribution. A disconnected service is
reported in the window; reopen aiterm to reconnect.

## Remaining platform differences

- Guided installation of WSL/distributions is still planned. The current
  startup screen requires an existing, working distribution.
- Fonts come from Windows. Downloaded font files install for the current
  Windows account; Fedora font packages do not apply.
- Renderer selection and its live sample work, but Linux WebKit CPU/GPU process
  counters are unavailable for WebView2. The measurement reports unavailable.
- The taskbar uses a waiting indicator; the shared tray menu contains the
  waiting sessions.
- Remote access runs inside WSL. LAN reachability still depends on Windows/WSL
  networking and firewall configuration.
- Feature availability inside the shared panels follows the installed Linux
  agents, credentials, and tools. Sharing an implementation is not a claim
  that every provider/network workflow has been exercised on Windows.

## Building

Preview 0.1.1 includes the Linux desktop's live conversation gateway fix:
`session.spine` is accepted by the wire decoder instead of being rejected
before dispatch. The shared decoder regression runs in the WSL companion's
test suite. Use Android 0.3.3 or later for the matching CBOR decoding and
working-indicator fixes. Updating the Windows app embeds the new companion;
its normal startup installs the updated backend into WSL.

Use a Windows-local checkout with Node.js, Rust MSVC, Visual Studio C++ Build
Tools, and WebView2. Inside Ubuntu install Rust, `build-essential`, `curl`,
`ca-certificates`, and `python3`.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-windows-wsl.ps1 -Bundle
```

The script copies Linux sources into a Linux filesystem build cache, tests and
builds the companion, builds the shared frontend and native executable, creates
an NSIS installer, and runs the native transport smoke test. Override the build
distribution with `-Distribution <name>`; runtime uses the user's WSL default.

Outputs are `windows/target/release/aiterm-windows.exe`,
`windows/target/release/bundle/nsis/`, and `windows/target/smoke-test.txt`.
Linux retains its existing `npm run tauri build` entry point.

## Implementation

- `src/windows/main.tsx`: workspace startup and Linux folder dialog around the
  shared application.
- `src/platform.ts`: command routing, file URLs, and terminal acknowledgments.
- `windows/src/workspace.rs`: persistent process supervision, command replies,
  native Tauri event/channel delivery.
- `windows/src/native.rs`: Windows desktop operations.
- `wsl-backend/src/core_modules.rs`: imports the production Linux modules.
- `wsl-backend/build.rs`: derives the dispatcher from their command signatures.
- `wsl-backend/src/runtime.rs`: state, events, channels, and async execution
  without a Linux GUI runtime.
- `wsl-backend/src/service.rs`: application state/startup and framed requests.

The `aiterm_headless` compiler configuration omits Tauri command wrappers only
in the companion build. It does not change the Linux desktop build. Hook-report,
chat, and MCP subprocess modes are retained for agents that launch aiterm itself.

The persistent service allows up to 64 concurrent commands and bounds each JSON
frame to 16 MiB. Terminal output is chunked and limited to 256 KiB
unacknowledged per attachment; xterm acknowledges bytes after consuming them.
Terminal traffic, events, and command replies share stdio without a TCP server.
The original single-PTY protocol remains for its native smoke test.

## Validation

```sh
cargo test --locked --manifest-path wsl-protocol/Cargo.toml
cargo test --locked --manifest-path wsl-backend/Cargo.toml -- --test-threads=4
cargo build --locked --manifest-path wsl-backend/Cargo.toml
python3 scripts/test-wsl-backend.py wsl-backend/target/debug/aiterm-wsl-backend
python3 scripts/test-wsl-service.py wsl-backend/target/debug/aiterm-wsl-backend
npm run test:ui
npm run build:windows-ui
cargo check --locked --manifest-path src-tauri/Cargo.toml
```

The shared tests cover session archive integrity and lifecycle, agents,
providers, terminal ownership, configuration, and remote services. Transport
tests exercise live PTYs, independent tabs, byte fidelity, resize, cleanup,
large file replies, structured save errors, registry events, and slow consumers.
Use four test threads because archive stress tests hold hundreds of file
descriptors each. Native builds and interactive Windows GUI checks remain
necessary in addition to these Linux tests.
