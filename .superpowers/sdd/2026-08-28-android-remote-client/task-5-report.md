# Task 5 report: shared desktop and remote session/agent services

## Status

`DONE`

Session discovery and mutation plus agent discovery/launch resolution now live behind transport-independent Rust services. Existing Tauri command names remain thin adapters, while the authenticated gateway exposes only the explicitly allowed typed operations. The gateway continues to use the Rust tab registry and terminal broker, so opening or closing from Android changes the same process-wide tab roster used by the desktop.

No Task 9 Android roster residual was changed in this task.

## Delivered behavior

- One long-lived `ApplicationServices` graph owns `SessionService` and `AgentService`. The production Tauri adapters and `RemoteServices` receive clones of the same Arc-backed graph; fixture roots/providers are injected without changing the common validation, existence gates, discovery budget, error mapping, or operation ordering.
- `SessionService` provides list, find, preview, delete, fork, and stop operations. Typed provider adapters retain store-specific I/O (including OpenCode's database transaction/dump behavior); `SessionRoots` provides explicit fixture-owned roots for parity and destructive-operation tests without consulting process `HOME`.
- `AgentService` owns installed-agent discovery, capabilities, choices, and launch resolution behind an injectable `AgentOperations` boundary.
- Existing `list_sessions`, `session_preview`, `session_delete`, `session_fork`, `stop_session`, `detect_agents`, `agent_caps`, `agent_choices`, and `resolve_launch` Tauri commands retain their names and return behavior as service adapters.
- `session.open` validates the id, then selects one unambiguous matching `Running` Rust tab. When none exists, it resolves the existing resume launch plan through `AgentService` and opens exactly one resumed tab through the shared registry. If an exited tab retains the canonical internal slot, the new tab receives a collision-free internal slot while retaining the requested `session_id`/`resumed_id`; the exited tab is preserved. It never closes or restarts a matching tab.
- Concurrent authenticated `session.open` calls for the same session coordinate through a deterministic bounded stripe lock and re-check the live registry while holding it. A two-socket race produces one new tab and one selection of that same tab, never duplicate resumes.
- `session.close` considers only matching `Running` registry tabs and never deletes a transcript. An exited match is left intact, an ambiguous live session requires `tab_id`, and `tab.close` remains the precise primitive.
- `agent.action` is a strict tagged union with only `start`. The client supplies an agent/model/effort selection, exposed project directory, title, and viewport, but never a shell command. The selection is first validated against the same offered-and-available choices returned by `agent.list`, including per-model effort values; resolution and command construction stay on the trusted desktop side.
- `tab.open` is now a strict server-owned shell intent. It accepts no command, environment, slot, or arbitrary cwd. An omitted/null `project_path` opens the default shell in the server default directory; a present path must exactly equal a `project_path` exposed by `session.list`.
- Agent start accepts a working directory only when it exactly matches a `project_path` exposed by the session service. Settings, arbitrary filesystem operations, font installation, diagnostics, unknown requests, and arbitrary command fields are rejected.
- Session identifiers are validated and production discovery uses one global per-request budget of 4,096 candidates across providers, with recursion depth 16, visited directory identities, symlink rejection, and SQL limits applied before allocation. Preview/message sizes retain the desktop bounds, terminal dimensions retain the protocol bounds, request fields are bounded, and every complete serialized response envelope remains under the existing terminal-frame limit. Oversized replies produce `protocol.response_too_large`.
- Production generic preview/fork/delete I/O verifies the approved provider root and walks parents with no-follow directory FDs. Reads, sibling creation, transcript/sidecar moves, atomic fork-map writes, trash purge, and OpenCode dump creation remain relative to pinned directory FDs. Root replacement and file/directory symlinks fail closed or continue only against the already-verified inode; unsupported platforms reject destructive operations.
- Rooted deletion resolves the requested transcript before creating or moving anything. Deleting an unknown session has no disk side effect.

## Android RPC contract

The outer binary CBOR envelope remains protocol version 1:

```text
request  = { version: 1, request_id: u64, kind: text, payload: bytes(CBOR) }
response = { version: 1, request_id: u64, kind: text, payload: bytes(CBOR) }
error    = response(kind = "error", payload = { code: text, message: text })
```

All operation payload structs reject unknown fields. Empty operations require a zero-length payload rather than an encoded empty map. Field names below are exact snake_case wire names.

| Request kind | Request payload | Success kind and payload |
|---|---|---|
| `session.list` | empty bytes | `session.list`: `{ sessions: Session[] }` |
| `session.preview` | `{ session_id: text }` | `session.preview`: `{ messages: PreviewMsg[] }` |
| `session.open` | `{ session_id: text, size: { cols: 1..512, rows: 1..512 } }` | `session.open`: `{ tab_id: text, selected_existing: bool }` |
| `session.close` | `{ session_id: text, tab_id?: text }` | `session.close`: `{ tab_id: text, ok: true }` |
| `session.delete` | `{ session_id: text }` | `session.delete`: `{ ok: true }` |
| `session.fork` | `{ session_id: text }` | `session.fork`: `{ session_id: text }` (the new id) |
| `session.stop` | `{ session_id: text }` | `session.stop`: `{ ok: true }` |
| `agent.list` | empty bytes | `agent.list`: `{ agents: AgentChoice[], caps: map<agent_id, Caps> }` |
| `agent.action` | `{ action: "start", agent_id: text, model?: text, effort?: text, cwd: text, title: text, size: { cols, rows } }` | `agent.action`: `{ tab_id: text, session_id: text/null }` |
| `tab.open` | `{ kind: "shell", project_path?: text/null, title?: text/null, size: { cols, rows } }` | `tab.open`: `{ tab_id: text }` |

`Session` retains the desktop serialized fields `id`, `agent`, `title`, `project_path`, `group_path`, `branch`, `forked`, `background`, `fork_parent`, and `last_active`. `PreviewMsg` contains `role` and bounded `text`. `AgentChoice` contains `id`, `display_name`, `models`, and `mints_session_id`; model choices retain their id, display name, efforts, and default effort. `Caps` is keyed by stable agent id and retains `fork`, `clear`, `resume`, `tui_drive`, `panels`, `tasks`, `delete`, `config`, and `roster_liveness`.

Stable operation errors are:

- `session.invalid_id`, `session.not_found`, `session.tab_ambiguous`, `session.tab_not_found`, `session.tab_mismatch`
- `session.delete_failed`, `session.fork_failed`, `session.stop_failed`
- `agent.unavailable`, `agent.invalid_selection`, `remote.path_not_allowed`, `remote.unsupported`
- existing protocol/terminal errors, including `protocol.invalid_payload`, `protocol.response_too_large`, and tab/terminal broker errors

The strict decoder still reports `protocol.unknown_request` when tested directly. Once a request reaches an authenticated gateway socket, that condition is intentionally translated to `remote.unsupported`, matching the Task 5 public remote contract without weakening the versioned decoder.

## TDD evidence

The first isolated `remote_operations` build failed because `services::sessions`, `services::agents`, and injectable application services did not exist. Subsequent behavior-first RED tests failed for the intended missing semantics before each minimum implementation:

- live-tab `session.open` initially looked for a transcript first;
- live-tab `session.close` initially required a transcript;
- the injectable `AgentOperations` boundary did not exist;
- authenticated unknown/desktop-only requests returned the decoder's internal unknown-request code;
- `agent.action` initially accepted an arbitrary unexposed working directory.

Additional GREEN coverage proves strict unknown-field/arbitrary-command/raw-path rejection, response-size failure, list/preview/fork/delete/stop fixture behavior, unknown-delete disk invariance, existing-tab selection, one-tab resume launch, close ambiguity, agent-list parity, and shared resolver use.

The completion review added five further RED regressions. Before their fixes, the suite was `16 passed, 5 failed`: close treated exited tabs as live matches; an exited canonical slot blocked a new resume; an invalid id could select a live tab; file/directory symlinks escaped an explicit root; and a nested payload below the cap could overflow its outer event envelope and close the socket. All five are now covered by GREEN behavior tests. A separate agent-list RED proved capability flags were absent; the reply now carries both choices and the shared capability map.

The independent hardening review added RED coverage for the second wave: typed `tab.open` was unsupported while legacy command/cwd input was accepted; unavailable model/effort selections reached the resolver; two simultaneous session opens could launch twice; discovery materialized provider results without one global budget; and root replacement separated pathname validation from the later destructive operation. The final tests exercise two real authenticated sockets, the production-shaped common service path, depth/budget limits, file and directory symlinks, source and trash root replacement, pinned trash purge, and consolidated permission stamping.

## Verification

All Rust commands used the isolated build directory `/tmp/aiterm-task5-target.rXGkuZ`; the ruled-unsafe real-`HOME` backend integration target was not run.

```text
cargo test --test remote_operations
25 passed, 0 failed

cargo test --test remote_server
35 passed, 0 failed

cargo test --lib
397 passed, 0 failed, 7 ignored

cargo test --test remote_auth --test remote_desktop --test remote_operations \
  --test remote_protocol --test remote_server --test remote_terminal \
  --test tab_registry --test terminal_screen
203 passed, 0 failed

cargo check
Finished dev profile successfully (isolated target)

npm run build
TypeScript and Vite production build succeeded

git diff --check
passed
```

A plain `cargo check` first encountered a pre-existing stale Tauri build-cache path pointing at `/home/matt/Projects/aiterm-android-remote`. Re-running against the isolated target compiled successfully. The temporary isolated target is removed before commit. The unsafe backend target was deliberately skipped because its current harness can consult real user state.

The first post-review aggregate run exposed an order-sensitive assertion in `remote_server`: a valid `terminal.snapshot` could precede the correlated `terminal.focus` acknowledgement. The test passed in isolation and the transport was behaving correctly; its existing kind-wait helper now drains valid intervening asynchronous events. The final `35/35` server run and full `203/203` safe aggregate passed with that deterministic assertion.

## Self-review and concerns

The final tracked diff was reviewed after reversing broad legacy-module rustfmt changes; only semantic Task 5 hunks remain. Fixture tests use unique explicit temporary roots and do not override `HOME`. Review findings for running-only close, exited-slot-safe open, early id validation, rooted symlink containment, complete-envelope bounds, strict server-owned launch input, common bounded services, production FD-relative I/O, concurrent resume coordination, offered-agent validation, and permission parity were reproduced and fixed. Remote handlers run through the gateway's existing bounded blocking-operation path, retain authenticated request correlation and attachment authorization, and do not change screen/focus/tab primitives.

There is one intentional compatibility split to carry into Task 9: the direct protocol decoder preserves `protocol.unknown_request`, while authenticated clients see the public `remote.unsupported` code. Android should implement the public gateway contract above.
