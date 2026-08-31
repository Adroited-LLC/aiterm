# The harness adapter contract

What aiterm must know about an agent CLI to make it a first-class citizen —
in the sidebar, in a tab, and on the phone. Every engine answers every
question differently; an adapter is the set of answers for one engine.

**How to use this file**: hand it to the agent whose CLI you're integrating
("implement/audit the <engine> adapter against HARNESS-CONTRACT.md"), along
with two pointers: the trait in `src-tauri/src/agents.rs`, and `grok.rs` as
the reference adapter — it documents observed behavior with versions, and
nothing in it is guessed. Require the same standard: every answer verified
against real files from a real session, stamped with the CLI version it was
read off. Training-data memory of a CLI's formats is usually stale.

## 1. Launch & identity
- Command to start a fresh session; flags for model, effort, permission mode.
- Can it be TOLD a session id at launch (claude `--session-id`, grok
  `--session-id`)? If not (codex), adoption: how does the transcript that
  appears after launch identify itself (id + cwd), so the placeholder tab
  can be re-keyed to it?
- Resume by id from any directory.

## 2. Session discovery  (`sessions.rs` providers)
- Where sessions live on disk and their unit (file vs directory).
- How to read: id, title, cwd, branch, last_active, forked/parent.
- What must never be shown as a title (harness boilerplate).

## 3. Conversation parsing  (`detail.rs`)
- Turn encoding: roles, text blocks, tool calls, tool results, reasoning.
- What the phone hides: tool outputs, harness preambles (codex sends
  AGENTS.md as its own first "user" message), env blocks.
- Tool-input summaries: the person-readable one-liner per tool call
  (codex `exec` JS → the shell command inside; image_gen → the prompt).

## 4. Busy / needs-you  (`remote.rs` transcript_state, `pty.rs` activity)
- Turn-in-flight signal: explicit events (codex task_started/complete),
  last-role (claude), open tool_calls (grok).
- Waiting-on-a-person signal: bell/OSC 9 (claude), or inference — an
  unanswered tool call plus a transcript quiet for ~45s (codex approval
  prompts write NOTHING while up).
- Terminal: OSC 9;4 progress? bell? If neither, output cadence is the
  only working signal.

## 5. Artifacts  (`changes.rs`)
- Harness-owned output dirs outside the workspace, and how the path names
  the session (codex `~/.codex/generated_images/<sid>/…`, grok
  `~/.grok/sessions/<enc-cwd>/<sid>/images/…`). Add the shape to
  `harness_session_of`, the noise filter, backfill, `harness_output_dirs`,
  and the remote file allowlist.
- Which session-dir contents are bookkeeping (grok: everything but
  `images/`) and must never be recorded as artifacts.
- Transcript-declared writes (claude Write/Edit tool_use), for
  `session_artifacts`.

## 6. Tasks / plans  (`sessions.rs` session_tasks)
- The todo format: claude task records, codex `update_plan`,
  grok `todo_write`.

## 7. Usage / limits  (`usage.rs`)
- Endpoint/CLI for plan bars and balances; auth source; observed rate
  limits (Anthropic throttles the usage poll — cache and retry).

## 8. Models / efforts  (`agents.rs` models())
- Static list, or shelled out per launch? Efforts are per-MODEL, not
  per-engine (codex publishes different sets).

## 9. Lifecycle
- Stop: daemon roster (claude) vs pty-tree kill (everyone else).
- Delete/trash: safe for files; a session that is a DIRECTORY (grok)
  needs directory trash or no button — half-working is worse.
- Fork / clear / compaction: claude-only unless proven otherwise.

## 10. Version stamp
Every answer above rots. Stamp the adapter's doc comments with the CLI
version the behavior was read off, and re-verify on upgrades.

## The standard of proof
An adapter claim is either (a) read off real files produced by a real
session during the work, with the version noted, or (b) not made. The
reference for tone and rigor is `grok.rs`.
