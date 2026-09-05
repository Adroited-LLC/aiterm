# Windows application powered by WSL

Status: Windows mounts the shared Linux application and full settings panel,
with its backend running as a persistent WSL service. See
[implementation and current limits](windows-wsl-preview.md). Guided first-run
WSL installation remains planned.

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

The visual direction is the same GUI style as Linux aiterm: preserve its
sidebar, toolbar, tabs, colors and panel layout. Windows-specific behavior and
fewer settings do not imply a separate minimalist terminal design.

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

The Windows UI should preserve the Linux interface and settings as closely as
possible. Use the default Linux distribution automatically. Put platform-specific
configuration under **Settings → Windows**. Candidate controls, not requirements
to add now:

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

## Relay pairing from a private VM (Windows 0.1.3 / Android 0.3.6)

The first phone could not pair to a NAT-only VM: the old flow registered the
relay only after a direct pairing succeeded. A fresh WSL pairing code now uses
version 4 and includes the relay control origin and connector-token hash, bound
to the same phone-signed enrollment digest as the route and desktop TLS pin.
The connector token remains on the desktop.

The WSL app persists one pending route and starts its outbound connector when
preparing the code. Retries and restarts reuse that route. Android still tries
direct endpoints first; if those are unreachable, it registers the signed route
through the existing relay provisioning API before trying the pinned desktop
connection through the relay. Only normal desktop approval grants device access.
Repeated registration conflicts never replace a route; successful pairing still
requires the exact desktop TLS identity. Approval preserves an already connected
relay so it does not interrupt its own pairing response.

No relay-server deployment or VM network change is required. Android continues
to support pairing versions 1–3 used by the installed Linux desktop. The Linux
installation is unchanged. Pending routes can be removed with the existing relay
removal control; a scan may authorize transport before desktop approval.

Validation: 601 backend tests passed; the Windows frontend built. An isolated
service generated a decodable QR, provisioned a temporary route through the real
relay, verified the exact desktop TLS pin, required approval, delivered approval
on the original relay socket, and persisted the approved configuration. The
relay removed the temporary test route with HTTP 204. This check caught and fixed
pending-to-approved configuration replacement: relay saves now use a private
synced temporary file followed by atomic rename.

## Installer networking guidance (Windows 0.1.4)

The NSIS installer checks the active mode of the user's default WSL distribution
with `wslinfo --networking-mode`, matching the distribution selected by the app.
Mirrored mode proceeds without a warning. Other recognized modes prompt the user
to choose Mirrored under WSL Settings > Networking; a failed or unavailable probe
reports that the mode could not be confirmed. The probe has a 15-second output
inactivity timeout. Silent installs use the message box's default response.

The notice recommends mirrored networking for direct LAN access, identifies the
Windows 11 22H2 requirement, and explains that reachability and firewall rules
still matter. It does not edit `.wslconfig`, change firewall rules, or shut down
WSL. Users save their work and restart WSL or Windows when ready. If WSL Settings
is unavailable, update WSL or follow Microsoft's `.wslconfig` instructions:
https://learn.microsoft.com/windows/wsl/networking#mirrored-mode-networking

Validation: the Windows installer built successfully. A temporary NSIS harness
verified real mirrored-mode detection plus simulated NAT and unavailable-probe
responses, including noninteractive completion in silent mode. Existing Linux
and Android application code is unchanged by this installer-only update.

## Guided WSL setup (Windows 0.1.5)

The installer checks whether the default distribution can run as a non-root
user. If it cannot, installation finishes with an offer to open WSL setup.
Silent installs skip the offer and never launch prerequisite installation.
The first-launch recovery screen also provides **Set up WSL** and **Try again**.

Both entry points use the bundled `windows/setup/setup-wsl.ps1` helper in a
visible PowerShell console. Setup installs Windows WSL components with explicit
UAC approval, using `--no-distribution` so an alternate administrator account
cannot accidentally own the user's distribution. It asks the user to save and
restart Windows, then reopen AITerm to continue. It does not restart the machine
or persist an automatic elevated startup task.

After WSL is available, setup offers Ubuntu when no usable distribution exists,
or lets the user select an existing distribution (excluding Docker internals).
Ubuntu installation and interactive account creation run as the original Windows
user. The distribution handles the Linux password directly. Setup verifies a
non-root default account and asks before changing WSL's default distribution.
It recommends mirrored networking after account setup. It does not unregister
distributions, reset accounts, edit firewall rules, or change networking modes.
A per-session mutex prevents duplicate setup windows from making concurrent changes.

Validation: Windows release and NSIS packaging build; frontend typecheck/build;
PowerShell workflow tests in `windows/tests/setup-wsl.tests.ps1`. Installer tests
in `windows/tests/installer-setup.tests.ps1` use the existing non-root default
distribution for the `actual` case and simulate missing/root-only workspaces.
The missing-WSL installation, UAC, reboot, and first-account creation paths are
mocked in the workflow tests; a full fresh-Windows acceptance run is still needed.
Existing developer WSL distributions are preserved during validation.
