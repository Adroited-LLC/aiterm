# Remote desktop session manager

Open the computer icon and choose a paired desktop. The sidebar combines that
computer's saved sessions and running terminals. All, Live, History, and Starred
filters and a title/project/agent/branch search help find a session.

- **Open** means a session is live and waiting for input.
- **Working** means the agent has an open turn.
- **Needs you** reflects explicit attention, excluding timeout-based tool approval guesses.
- **History** means no live session was reported.

Click a live session to open its terminal. Click a saved session to read its
conversation, then **Resume session** to continue it on the remote computer.
Reading history and opening a terminal do not take input control. Existing
terminal typing/control handoff behavior is unchanged.

Each session's ellipsis button (or right-click) offers applicable history,
resume, star, rename, fork, stop/close, and delete actions. Agent capabilities
control availability. Stopping and deleting require confirmation; live sessions
cannot be deleted. A session running outside a controllable AiTerm tab may
report the server's ownership error when resuming.

The Rust desktop client uses the same authenticated `session.roster`,
`session.conversation`, `session.spine`, `agent.list`, and session mutation
operations as Android. It never copies sessions into the local catalog or
constructs remote shell commands. Resume uses the server's shared launch
resolver and reuses an existing live tab. Actions are bound to the connection
generation and are not replayed after a disconnect. Older hosts without spine
status support retain conservative roster status labels.

History is a bounded, read-only conversation preview (200,000 characters), not
Android's full interactive API view. Resume continues in the remote terminal.

Validation:

- `npm run test:ui`: roster merging, statuses, ordering and search/filter tests.
- `cargo test --manifest-path src-tauri/Cargo.toml desktop_remote --lib`: local TLS
  gateway tests cover authentication, saved-session preview, resume/reuse,
  action errors, live-delete refusal and existing input/control/reconnect behavior.
- `tests/browser/remote-sessions.html`: production component with an in-memory
  transport for inspecting themes, filtering, menus, confirmation and resume.
  This fixture does not access real sessions or pairing keys.
