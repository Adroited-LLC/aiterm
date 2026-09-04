# Windows / WSL workbench preview

The Windows preview now uses Linux aiterm's stylesheet, icons, toolbar and tab
layout, shared file explorer/editor, and shared Git panel. The Windows workbench
has a sessions/projects sidebar, independent WSL terminal tabs, a home launcher,
and a small Settings → Windows panel. It replaces the single-terminal demo.
See [the product plan](windows-wsl-port.md).

It discovers installed Claude Code, Codex, OpenCode and Gemini CLIs in the Linux
login environment. Recent Claude Code and Codex transcripts can be resumed;
agent credentials and defaults remain inside Linux. The folder picker browses
Linux directly, and project discovery covers both ~/Projects and ~/projects.
Ctrl+Shift+T opens another terminal; Ctrl+S saves in the shared editor.

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

Each terminal tab owns a Linux session. Closing a tab releases its processes;
other tabs stay running. Closing the window releases all its terminal sessions.
After a process exits, Restart terminal creates a fresh connection. Dirty file
tabs prompt before discarding edits, and saves retain the Linux editor's mtime
conflict check. Other applications and WSL itself remain running.

## Preview limits

This is the Linux **style** and a working core workbench, not complete feature
parity. Task/usage panels, provider setup, full session lifecycle operations,
detached terminals, automatic tab restoration, and guided WSL installation
remain future work. Agent installation and sign-in still happen in the terminal.
The distribution is pinned for the life of the app; restart after changing the
WSL default. Changes are refreshed every 15 seconds or with the refresh button.

History is a bounded recent-transcript scan (up to 300 files per supported
engine); it does not yet share the full Linux session indexer. OpenCode/Gemini
can launch, but their history is not included yet. HTML opens as source; Markdown
has the shared rendered preview. Media/HTML asset routing is not ported.
Requests and replies are bounded to 1 MiB serialized JSON; larger files or diffs
currently report an error. Build/test baseline remains Ubuntu x86-64.

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
and entry point remain the Linux build. Shared file/Git command attributes are
conditionally omitted when those modules compile into the headless companion.

## Boundary and validation

- `windows/`: native Tauri host, companion installation and process supervision.
- `src/windows/`, `windows-ui/`: Windows workbench composed with shared Linux UI.
- `src/platform.ts`: sends shared panel operations through the WSL bridge in
  Windows builds; Linux retains its original Tauri commands.
- `wsl-backend/`: Linux PTYs and short-lived RPC mode. Compiles the existing
  `src-tauri/src/git.rs` and `fsx.rs` directly, with no GUI dependencies.
- `wsl-protocol/`: versioned, size-limited JSON frames over stdin/stdout. Terminal
  bytes use base64 so partial UTF-8 and control sequences arrive unchanged.

The renderer acknowledges output after xterm consumes it. The companion limits
unacknowledged output to 256 KiB and applies backpressure instead of accumulating
an unbounded queue. Protocol errors close the session. Startup has timeouts and
visible retry/error states.

Run the terminal checks on Linux:

```sh
cargo test --locked --manifest-path wsl-protocol/Cargo.toml
cargo test --locked --manifest-path wsl-backend/Cargo.toml
cargo build --locked --manifest-path wsl-backend/Cargo.toml
python3 scripts/test-wsl-backend.py wsl-backend/target/debug/aiterm-wsl-backend
npm run test:ui
npm run build:windows-ui
```

Tests exercise protocol framing/version mismatch, Unicode, actual terminal
dimensions, Ctrl+C, output-before-exit ordering, output backpressure/resumption,
and session cleanup including a foreground job that ignores hangup. They also
cover separate tab directories/lifetimes, Unicode/quoted filenames, conflicting
saves, Git status/log/branches/diffs, safe Markdown, and transcript discovery.

The Windows executable also accepts `--smoke-test`, with `AITERM_SMOKE_REPORT`
pointing to the result file. This exercises companion provisioning, WSL launch,
workspace metadata, file listing, the handshake, terminal output, dimensions, and exit status through the same
bridge code as the UI. It does not substitute for visual/interactive GUI testing.
