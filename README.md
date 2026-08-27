# aiterm

A desktop workbench for running [Claude Code](https://claude.com/claude-code) sessions — a real terminal in the middle, with the state Claude keeps on disk surfaced as first-class UI around it.

Built with Tauri 2, React, and xterm.js. Linux-first (developed and packaged on Fedora); the stack is cross-platform but other platforms are untested.

> **Work in progress.** aiterm is built around one person's daily use and
> released often, so interfaces move, rough edges are expected, and nothing here
> is promised to be stable. It is developed in the open but is not open source —
> see [LICENSE](LICENSE) before using it or anything derived from it.

## What it does

- **Real terminals.** Each tab is a native PTY running `claude` (or a plain shell). No re-implementation of the CLI — the terminal is the terminal.
- **New sessions.** ＋ in the sessions panel starts a fresh `claude` in any directory — filter the ones you already work in, or pick a folder. aiterm mints the session id, so the new tab gets its sidebar row immediately instead of waiting for the first prompt to write a transcript.
- **Session browser.** Every Claude Code session on the machine, indexed with tantivy and searchable, grouped by project or date. Resume any session into a tab; preview, rename-by-drag, trash with undo, and fork lineage are all handled, including the session-id rewrites Claude does on resume and background mode.
- **Dialogs over the TUI.** Claude's own `/model`, `/rewind`, permission prompts, and the "Switch model?" confirm are detected from the screen and presented as real dialogs — keyboard-first, closed-loop (every keystroke is verified against what the TUI actually drew), and honest about failure. The TUI underneath stays the source of truth.
- **Composer pills.** Model, effort, tasks, artifacts, running agents, permission mode, git, and usage — read live from the session transcript and Claude's config, so they stay right no matter who changed them.
- **Usage.** Plan limits (the same data as `/usage`) as compact bars, plus the session's context-window fill read from the transcript.
- **Panels.** File explorer, git (branches, log, per-file diffs with inotify-driven refresh), and an agents view for background sessions.
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
