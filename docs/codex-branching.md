# Codex session branches

Codex sessions offer the same branch icon as Claude sessions. Clicking it saves
a separate branch at the current recorded history. The original session keeps
its tab and keeps running; the new branch appears as a stopped session, ready to
resume. The shared backend also exposes this capability to remote clients.

AiTerm invokes the installed Codex CLI's `app-server` over a private stdio pipe:
`initialize`, `initialized`, then `thread/fork` with the selected session id,
`ephemeral: false`, and `excludeTurns: true`. Codex owns the copying, identity,
and history semantics. AiTerm never sends `turn/start` and never sends `/fork`
into the source terminal, where it would switch the current conversation.

The helper is bounded to 30 seconds, limits response sizes, declines interactive
requests, and terminates its own process group after finishing. Errors are shown
without automatically retrying; a timed-out operation can already have persisted
a branch, so inspect the list before trying again. An older CLI that lacks the
fork API needs a Codex update.

The scanner reads Codex's `forked_from_id` metadata to show the branch marker and
parent relationship. This also identifies branches created with Codex's own CLI.
Claude retains its existing verified transcript-copy implementation.

Validation includes bounded-helper and protocol tests plus an opt-in test with
an installed Codex CLI and synthetic history in an isolated `CODEX_HOME`:

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib agents::codex_fork::tests -- --include-ignored
```

The native test checks that parent bytes remain unchanged, context is copied,
AiTerm discovers the new id and lineage, and a fresh Codex process can load and
fork that saved branch again. It sends no model turn and uses no credentials.
