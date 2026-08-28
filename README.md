# aiterm

A desktop workbench for running [Claude Code](https://claude.com/claude-code) sessions — a real terminal in the middle, with the state Claude keeps on disk surfaced as first-class UI around it.

Built with Tauri 2, React, and xterm.js. Linux-first (developed and packaged on Fedora); the stack is cross-platform but other platforms are untested.

> **Work in progress.** aiterm is built around one person's daily use and
> released often, so interfaces move and rough edges are expected. Free to
> build, run and modify for your own use, at home or at work — but it is not
> open source: it may not be sold, offered as a service, or redistributed.
> See [LICENSE](LICENSE).

## What it does

- **Real terminals.** Each tab is a native PTY running `claude` (or a plain shell). No re-implementation of the CLI — the terminal is the terminal.
- **New sessions.** ＋ in the sessions panel starts a fresh `claude` in any directory — filter the ones you already work in, or pick a folder. aiterm mints the session id, so the new tab gets its sidebar row immediately instead of waiting for the first prompt to write a transcript.
- **Session browser.** Every Claude Code session on the machine, indexed with tantivy and searchable, grouped by project or date. Resume any session into a tab; preview, rename-by-drag, trash with undo, and fork lineage are all handled, including the session-id rewrites Claude does on resume and background mode.
- **Dialogs over the TUI.** Claude's own `/model`, `/rewind`, permission prompts, and the "Switch model?" confirm are detected from the screen and presented as real dialogs — keyboard-first, closed-loop (every keystroke is verified against what the TUI actually drew), and honest about failure. The TUI underneath stays the source of truth.
- **Composer pills.** Model, effort, tasks, artifacts, running agents, permission mode, git, and usage — read live from the session transcript and Claude's config, so they stay right no matter who changed them.
- **Usage.** Plan limits (the same data as `/usage`) as compact bars, plus the session's context-window fill read from the transcript.
- **Panels.** File explorer, git (branches, log, per-file diffs with inotify-driven refresh), and an agents view for background sessions. The Agent panel's tasks and artifacts read Claude Code, Grok, and Codex sessions — each engine's own on-disk shape.
- **File tabs.** The center is a browser-style tab strip: the session's terminal in a locked tab, and files from the explorer (or the artifacts list) opening beside it in an in-app CodeMirror editor — syntax highlighting, search, Ctrl+S save with a conflict guard so an agent's concurrent write is never silently clobbered.
- **Comforts.** Themes, font picker with system font install, per-panel layout persistence, window-state restore, attention bell when a session needs input.

## Running it

```bash
npm install
npm run tauri dev
```

A release build (`npm run tauri build -- --bundles rpm`) produces an installable RPM under `src-tauri/target/release/bundle/rpm/`.

You'll want the `claude` CLI installed and signed in; aiterm reads its state from `~/.claude/` and drives the real thing.

## Design notes

The session/fork model — what owns a session's lifetime, how resume and background mode move ids, why tabs own processes — is written up in [SESSION-MODEL.md](SESSION-MODEL.md).

The recurring principle: read state from where Claude already keeps it (transcripts, config files, the drawn screen) rather than tracking a copy, so the UI can't drift from the truth. Where aiterm drives the TUI, it does so closed-loop and refuses to guess.

## Development workflow

Two branches in our own copy, and a pull request upstream whenever the second one moves.

- **`5lime`** is dev. All work happens here.
- **`main`** is prod-ready. When a version is ready, `5lime` is merged into `main`, and every update of `main` goes to the original repo as a pull request.

The repos, with the remote names in this checkout (`git remote -v`):

- **`Adroited-LLC/aiterm`** — remote `origin`. The canonical repo, Matt's. Read-only for us; it takes pull requests and squash-merges them.
- **`jallisonfl/aiterm`** — remote `fork`. Our own copy, private, holding `5lime` and `main`. It is *not* a GitHub fork of the canonical repo (an independent repo that shares history), and GitHub only accepts cross-repo pull requests from real forks — so no PR can be opened from here, which is why the next one exists.
- **`jallisonfl/aiterm-upstream`** — remote `upstream-fork`. A real fork of the canonical repo, public because a fork of a public repo cannot be private. It carries a mirror of `main` for the sole purpose of opening pull requests from it. Nothing is developed there.

The cycle:

1. **Sync from upstream before starting.** If `origin/main` has moved, merge it into `5lime`. Upstream squash-merges, so `5lime` keeps its own history and this merge is routine:

   ```bash
   git fetch origin
   git rev-list --count 5lime..origin/main   # non-zero → needs merging
   git merge origin/main                    # on 5lime
   ```

2. **Work on `5lime`** and push it to our copy when john says so:

   ```bash
   git push fork 5lime
   ```

3. **Publish a version** — merge dev into prod, push prod to our copy and to the PR fork:

   ```bash
   git checkout main && git merge 5lime
   git push fork main
   git push upstream-fork main
   ```

4. **Open the pull request** from `jallisonfl:main` (the fork `aiterm-upstream`) into `Adroited-LLC:main`. Through the API, that is `POST /repos/Adroited-LLC/aiterm/pulls` with `head: jallisonfl:main`, `head_repo: jallisonfl/aiterm-upstream`, `base: main` — and it needs `GITHUB_CLASSIC_TOKEN`; the fine-scoped `GITHUB_TOKEN` is refused for a repo we do not own. Both live in `~/AI-OS/.env`. Check the build first: `(cd src-tauri && cargo test --lib) && npx tsc --noEmit`.

After pulling a commit that adds npm packages, run `npm install` in the checkout — otherwise `npm run tauri dev` comes up with a Vite "Failed to resolve import" overlay instead of the app.
