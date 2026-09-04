# Windows application powered by WSL

Status: first terminal milestone built, installed, and exercised in the Windows
11 / Ubuntu 24.04 WSL 2 VM. Full agent/session functionality and first-run WSL
installation are not yet implemented.

## Product contract

aiterm on Windows requires WSL. It provides a native Windows desktop experience
with terminals, agents, projects, Git, and session management running inside a
Linux distribution. Native Windows shells and native Windows agent execution
are outside the scope of this port.

Priority order: a working, reliable system; ease of use; visual polish. Setup
should feel like installing one application. Users should not need prior WSL
knowledge, terminal commands, or an understanding of the backend architecture.
Explain Linux and administrator actions where they help the user make a decision.
Do not conceal required system changes.

All implementation work belongs on `feature/windows-wsl`, based initially on
`728bda5`. Keep changes in focused commits suitable for review and incremental
pull requests. Preserve the Linux desktop application throughout the port.

## Desktop and backend boundary

- Windows owns the Tauri window, existing React interface, desktop integration,
  onboarding, and backend supervision.
- A headless Linux companion inside WSL owns PTYs, agent execution and discovery,
  session history and resume, project files, Git, file watching, and Linux process
  tracking. Reuse existing Rust logic after separating it from Tauri dependencies.
- Windows launches the companion through `wsl.exe` in a selected distribution.
  Prototype a framed, versioned protocol over standard input/output. Keep logs on
  standard error; define framing, request IDs, cancellation, event delivery,
  backpressure, and terminal binary data before expanding feature coverage.
- Negotiate protocol compatibility before allowing operations. Package a matching
  companion and update it with the desktop app, with atomic installation and a
  recoverable failure path.
- Address each workspace by distribution identity and Linux path. Do not treat a
  Windows path or Windows home directory as an agent's Linux configuration root.
- Keep credentials in the environment where the agent runs. Do not put secrets
  into process arguments or diagnostic output.

This is a proposed boundary, not an assumption that the current backend already
runs headlessly. Audit Tauri coupling and existing transport abstractions before
choosing which modules to extract or reuse.

## First-run experience

1. Detect WSL availability, distributions, and whether the chosen distribution
   can actually start. Distinguish missing components from a broken installation.
2. For a new user, present one recommended setup action with a short explanation
   of the Linux workspace and required download/system changes. Request Windows
   elevation only at the step that needs it.
3. If a restart is required, persist setup progress and resume when aiterm is
   reopened. Recheck actual machine state before continuing.
4. Provision the default distribution and user, then install and verify the
   companion and required runtime dependencies. Separate optional agent installs
   and account sign-in from the minimum needed to open a working terminal.
5. Open the application when a real backend health check and terminal launch
   succeed. Use Linux-local project storage by default.

For an existing WSL installation, reuse a supported distribution. Choose the
obvious valid option automatically when there is only one, and offer a compact
selection when necessary. Do not silently change the system's default
distribution, overwrite shell configuration, or replace an existing Linux user.

Onboarding must handle unsupported Windows versions, disabled virtualization,
missing nested virtualization in a VM, interrupted downloads, insufficient disk
space, failed elevation, and distribution initialization. Present the cause and
one useful next action. Put technical details behind an expandable disclosure.

## Everyday experience and settings

Launch and connect to WSL automatically. Show a quiet startup state rather than
flashing console windows. Preserve session ownership across reconnects; report
when a session has ended rather than pretending it survived a backend restart.
Define desktop-close versus session-stop semantics before implementing shutdown.
Never terminate the user's entire WSL distribution to close aiterm.

Provide actions such as Open in Explorer and import/clone project so users do not
have to translate paths. File dialogs, drag-and-drop, uploads, previews, and open
links must consistently cross the Windows/Linux boundary.

The Windows UI may differ from Linux and should have fewer settings. Start with
no settings panel; use the default Linux distribution automatically. When a real
configuration need emerges, place Windows-specific controls under
**Settings → Windows**. Candidate controls, not requirements to add now:

- Linux workspace/distribution and connection status.
- Repair or retry setup, preserving projects and user configuration.
- Diagnostics with copyable, redacted details.

Add further controls only for a demonstrated user need. Prefer automatic behavior
over exposing transport, ports, helper paths, or backend process options. Follow
aiterm's established visual language with restrained progress states, concise
copy, keyboard accessibility, and clear error recovery.

## Delivery milestones

### 1. Prove the complete terminal path

Establish SSH access to the Windows test VM and verify a WSL 2 distribution can
start. Build a native Windows app and Linux companion. Connect the existing UI
to one real WSL PTY through the proposed boundary.

Acceptance: terminal input/output, Unicode, resize, Ctrl+C, exit status, and
cleanup work in the Windows VM. Test a slow reader and backend disconnection.
No claim of full Windows support until this path runs on Windows.

### 2. Bring across the existing application behavior

Move agent discovery, session list/resume, process tracking, Git, project files,
watching, and previews behind the environment boundary. Keep Linux functionality
working while doing so. Exercise spaces and non-ASCII paths and multiple
distributions so identities do not collide.

Acceptance: create a project, launch an agent, discover and resume its session,
edit files, inspect Git changes, and reopen the app without losing project or
session identity. File navigation and previews work across the OS boundary.

### 3. Guided setup and Windows settings

Implement the resumable setup state machine and the minimal Windows settings
panel. Validate both a clean Windows VM and a VM with an existing distribution.
Use VM snapshots for destructive prerequisite/reset test cases.

Acceptance: a user unfamiliar with WSL can reach a working terminal without
typing setup commands. Restart, cancellation, offline failure, retry, and repair
have tested recovery paths that preserve existing user data.

### 4. Package and validate upgrades

Produce the Windows installer with the companion and required desktop runtime
handling. Add Windows build checks and Linux regression checks. Verify first
install, upgrade, protocol mismatch, repair, and uninstall behavior. Uninstall
must preserve user projects and existing WSL distributions by default.

## Development environment

Use a Windows-local checkout for native Windows compilation and a Linux-local
checkout/build directory for the companion. Do not share `node_modules` or Rust
build artifacts across operating systems. Use Git to synchronize source.

The initial test VM was shown at `10.0.0.196`; confirm its current address before
connecting. Use Windows OpenSSH for command execution and the graphical console
for UI testing. The host serial PTY does not establish a Windows shell.

The VM now uses the libvirt default network at `192.168.122.20`. Windows OpenSSH
is running with key authentication from the Linux host. Ubuntu 24.04 starts
successfully as WSL 2 under the Windows user's account. Windows MSVC, Node.js,
Rust, WebView2, and the Linux companion toolchain are available. The native
executable passed its WSL smoke test; the installed GUI displayed a shell and
ran commands through keyboard input. The NSIS installer completed successfully.
