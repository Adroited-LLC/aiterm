# Windows / WSL terminal preview

This is the first milestone of the Windows port: a native Windows window that
automatically opens a real terminal in the user's default WSL distribution.
It has a separate, minimal interface. Full agent/session browsing, project
navigation, guided WSL installation, and update/uninstall lifecycle management
are later milestones. See [the product plan](windows-wsl-port.md).

## Runtime requirements

- Windows 11 with WebView2 and a working WSL 2 distribution.
- Ubuntu 24.04 x86-64 is the initial build/test baseline. Other distributions and
  architectures have not been validated; the companion currently uses the build
  distribution's glibc rather than a portable static runtime.
- A normal Linux user and working login shell in the default distribution.

No Rust, Node.js, compiler, network port, SSH service, or manually installed
companion is required to **run** the packaged preview. The desktop embeds the
Linux companion, installs it under `~/.local/share/aiterm/backends/<sha256>`, and
starts it with `wsl.exe`. Existing distribution defaults are not changed.

Each window owns one shell session. Closing the window stops the processes still
in that terminal's Linux session. After exiting a shell, Open a new terminal
starts a fresh session. Persistence and detached agent sessions are not included
yet. Other applications and the WSL distribution itself remain running.

## Build on Windows

Use a Windows-local checkout. Install Node.js LTS, Rust with the MSVC toolchain,
Visual Studio C++ Build Tools (Desktop development with C++), and WebView2.
Inside Ubuntu, install Rust and `build-essential`, `curl`, `ca-certificates`, and
`python3`. WSL builds stay in the Linux filesystem; Windows builds stay in NTFS.

From PowerShell at the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-windows-wsl.ps1 -Bundle
```

The script builds/tests the Linux companion, builds the Windows UI and native
executable, optionally creates an NSIS installer, and runs a real Windows-to-WSL
terminal smoke test. The default build distribution is Ubuntu; override with
`-Distribution <name>`. At runtime the application uses the user's WSL default.

Outputs:

- `windows/target/release/aiterm-windows.exe`
- `windows/target/release/bundle/nsis/` (with `-Bundle`)
- `windows/target/smoke-test.txt`

The existing Linux app continues to use `npm run tauri build`. Its Rust package
and entry point are unchanged by the preview.

## Boundary and validation

- `windows/`: native Tauri host, companion installation and process supervision.
- `src/windows/`, `windows-ui/`: minimal React/xterm.js frontend.
- `wsl-backend/`: Linux PTY companion, initially scoped to one terminal. The
  existing application's provider, session and process-discovery modules have
  not yet been extracted into it.
- `wsl-protocol/`: versioned, size-limited JSON frames over stdin/stdout. Terminal
  bytes use base64 so partial UTF-8 and control sequences arrive unchanged.

The renderer acknowledges output after xterm consumes it. The companion limits
unacknowledged output to 256 KiB and applies backpressure instead of accumulating
an unbounded queue. Protocol errors close the session. Startup has timeouts and
visible retry/error states.

Run the terminal checks on Linux:

```sh
cargo test --locked --manifest-path wsl-protocol/Cargo.toml
cargo build --locked --manifest-path wsl-backend/Cargo.toml
python3 scripts/test-wsl-backend.py wsl-backend/target/debug/aiterm-wsl-backend
npm run test:ui
npm run build:windows-ui
```

Tests exercise protocol framing/version mismatch, Unicode, actual terminal
dimensions, Ctrl+C, output-before-exit ordering, output backpressure/resumption,
and session cleanup including a foreground job that ignores hangup.

The Windows executable also accepts `--smoke-test`, with `AITERM_SMOKE_REPORT`
pointing to the result file. This exercises companion provisioning, WSL launch,
the handshake, terminal output, dimensions, and exit status through the same
bridge code as the UI. It does not substitute for visual/interactive GUI testing.
