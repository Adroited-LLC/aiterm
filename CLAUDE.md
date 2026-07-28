# Working in this repository

## Before you branch, push, or open a PR

Read [BRANCHING.md](BRANCHING.md). It is the repository's branching policy, not
a suggestion. The short version:

- **Pull requests target `dev-pr`, never `main`.**
- Cut each branch from `dev-pr`, one change per branch.
- Keep a contributed change to **one commit** — amend, do not stack.
- Never commit directly to `dev-pr`, `alpha`, `beta`, `staging` or `main`.
- Promotion runs `dev-pr → alpha → beta → staging → main`, by fast-forward
  merge, never by force-push.

## Before you claim something works

The prevailing standard here is that an unverified claim is a bug. Two files
exist because of it: [SESSION-MODEL.md](SESSION-MODEL.md) records what has been
proven about Claude Code's session model and what is still open, and it opens by
noting that a full day was once lost building on plausible-but-unchecked
assumptions.

So: before relying on a CLI flag, an endpoint, a config key or a file format,
prove it. Run the command. Curl the endpoint. Read the file. Then record the
evidence in the comment or commit message — dated, with what you ran and what it
returned. Several comments in this codebase do exactly that, and they are the
reason later changes were safe.

If you cannot verify something, **do not ship a guess.** Leave it out and say so.
A feature that silently reports nothing is worse than an absent one, because
nothing distinguishes it from a correct empty answer.

## House style

- **Comments explain why**, including what was tried and rejected. Read
  `src-tauri/src/agents.rs`, `src-tauri/src/pty.rs` or
  `src/components/SessionsPanel.tsx` before writing your first one.
- **No new HTTP or TLS crates.** Network calls shell out to `curl` — see
  `src-tauri/src/usage.rs`, which also documents why any such call must be
  `#[tauri::command(async)]`: a synchronous one froze the whole window.
- **Rust changes need tests.** Split pure parsing from I/O so it is testable
  without a network or a fixture machine — see `parse_models` in
  `src-tauri/src/providers.rs`.
- **Credentials never cross to the frontend**, and never go on a command line —
  a command runs through `$SHELL -ic`, so anything in it is visible in `ps`. Put
  them in the process environment instead (`LaunchPlan` in `agents.rs`).
- **Never hide a row on a heuristic.** The session list shows what is on disk;
  only an explicit delete removes something.

## Building

```bash
npm install
npm run build                      # tsc + vite
cd src-tauri && cargo test --lib
npm run tauri dev                  # do not run while another instance is up
npm run tauri build -- --bundles rpm
```

Note that `tauri dev` watches the Rust sources and rebuilds on save, so an
in-progress edit that does not compile will take the running window down with
it. Stop it before a multi-step refactor.
