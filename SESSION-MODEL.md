# Claude Code's session model, as far as we have proven it

Working notes for aiterm. Everything here is split into **what was verified and
how**, and **what is still open**. The split is the point: on 2026-07-25 a full
day was lost building fixes on top of plausible-but-unchecked assumptions about
this model. If a claim moves from "open" to "verified", record the evidence.

Last updated 2026-07-25.

---

## Where sessions live

- A session is a JSONL transcript at
  `~/.claude/projects/<flattened-cwd>/<session-uuid>.jsonl`.
  The directory name is the session's launch cwd with `/` and `.` flattened to
  `-`. *Verified: listing the store.*
- Job state for daemon-run sessions is `~/.claude/jobs/<short-id>/state.json`,
  where `<short-id>` is the first UUID segment. *Verified: read directly.*
- Retired transcripts are renamed `<id>.orphaned-<epoch>-<hash>.jsonl` rather
  than deleted — but not always; a `/clear`ed original can be removed outright.
  *Verified: both shapes present in the store.*

## Resuming

- `claude --resume <id>` refuses a session that is **currently running**, with
  "…add --fork-session to branch off a copy". Nearly every awkward behaviour in
  a GUI wrapper descends from this one constraint.
  *Not re-verified 2026-07-25 — inherited from earlier work and from Claude
  Code's own error text. Worth reproducing once.*
- `--fork-session` copies the full history into a **new** session id and leaves
  the parent byte-identical and independently resumable.
  *Verified: parent `32cb631d` unchanged at 64358 bytes with its original
  mtime; child carried 36 records back to the parent's first message.*
- `--session-id <uuid>` **composes** with `--fork-session`, so the caller can
  mint the child's id instead of discovering it later.
  *Verified: ran it; the child transcript appeared at the chosen uuid.*
- Prompting an already-running session does **not** mint a new session.
  **Resuming does.** *Verified: sent a bare test message, no new transcript and
  no new roster entry; only the live transcript grew.*

## Background agents

- Background agents are held by the Claude Code daemon and **survive the client
  exiting**, so they are still running the next time the GUI starts.
- `claude agents --json` lists live sessions — **both** `background` and
  `interactive` — with `sessionId`, `kind`, `pid`, `cwd`, and `state`
  (`working` / `blocked` / `done`). *Verified: used throughout.*
  - It includes `state: "done"` entries, so "appears in the roster" is **not**
    the same as "is running". Filter `done`.
- **There is no attach-by-id.** No flag on `claude agents`, and no other
  subcommand (`auth`, `auto-mode`, `doctor`, `gateway`, `install`, `mcp`,
  `plugin`, `project`, `setup-token`, `ultrareview`, `update`), opens a session
  directly. `claude agents` always lands on a list.
  *Verified: read the full help for every subcommand.*
- `claude agents --cwd <path>` **filters** to agents started under `<path>` — it
  does not merely sort. A fork started in a different tree is therefore absent
  from a view filtered by the row's project path. *Verified: help text plus the
  observed empty view.*
- The roster reports a real `pid`, and SIGTERM to it stops the agent.
  *Verified: stopped three agents this way; all three left the roster and
  stayed gone.*

## Deleting

- **Deleting a running session's transcript does not stick.** The live process
  recreates the file at the same path within seconds, rebuilt from the deletion
  point — so the row returns *and* the history before the delete is lost.
  *Verified: `b79ba823` trashed at 11,660 bytes, reappeared at 660 bytes one
  minute later.*
- Therefore: stop the process first, then delete. Never offer delete on a
  session known to be running.

## Telling forks apart

Two different things are both called "fork", and they leave completely
different traces.

| | `/fork` command | `--fork-session` (what a GUI runs) |
|---|---|---|
| transcript | ~192-byte stub: `ai-title` + `agent-name` only, no cwd, no message chain | full copy of the parent's history |
| `sessionKind` | absent | absent |
| jobs state | `forkSessionId` + `forkParentSessionId` **present** | fork fields all `null` |
| `parentUuid` | no message records at all | resolves **in-file** (history was copied) |
| `bridgeSessionId` | — | child gets its **own**, not the parent's |

- So `/fork` lineage is discoverable from job state, and **`--fork-session`
  lineage is discoverable from nothing on disk.** *Verified three independent
  ways for the `--fork-session` case.*
- Consequence: a GUI that forks must **mint the child id itself**
  (`--session-id`) and record the pair, or it can never link them afterward.
- `sessionKind: "bg"` in a transcript is a **permanent scar** — it stays true
  long after the agent exits, so it answers "was this ever a background
  session", never "is it running now". *Verified: a dead session still
  reporting it.*

## Conversation identity across session ids

- `bridgeSessionId` is the stable id for a *conversation* while its session ids
  churn.
- It appears in transcripts as a `bridge-session` record, but **not always** —
  one live session had none.
- It **also** appears in job state as `bridgeSessionId`, and that copy linked
  two session ids whose transcripts could not be linked.
  *Verified: `8e6ad72e`'s job state carries the same bridge as `2a7f02c6`'s
  transcript; they are the same conversation.*
- This is the most promising key for "group these rows as one conversation".

## Configuration that changes all of the above

- `remoteControlAtStartup` exists in **two** files:
  - `~/.claude/settings.json`
  - `~/.claude.json` ← **authoritative**; setting only the first has no effect.
  *Verified the hard way.*
- Job state for a conversation that has moved to the background records
  `template: "bg"`, `backend: "daemon"`, `interactiveLineage: true`.

---

## Open questions

1. **Does `remoteControlAtStartup: false` actually stop sessions becoming
   background agents?** Both files now read `false`; nothing beyond that is
   established. One fresh (not resumed) session settles it.
2. Does a conversation whose job state carries `template: "bg"` **always**
   re-spawn as a background job when resumed?
3. What exactly triggers "Your conversation moved to the background"?
4. Reproduce the `--resume`-refuses-a-running-session error directly.

---

## What this implies for aiterm

- **A tab owns its session's lifetime.** `TerminalView` kills the pty on
  unmount, so closing a tab ends the session and a plain `--resume` works
  afterwards. *Verified: read the unmount path.* The whole fork-on-resume
  apparatus existed to work around sessions that outlived their tabs.
- **"Has a tab" and "is running" are different questions** and must not share a
  flag. They name the same row right up until a conversation moves to the
  background, and then every consequence — the live dot, which actions are
  offered, whether delete appears — lands on the wrong row.
- **Never hide a row on a heuristic.** The list should show what is on disk;
  only an explicit delete removes something.
- Duplicate near-identical rows are **real files**, not a display bug: one
  full-history snapshot per resume, each frozen at that moment. The fix is to
  group and label them honestly, not to hide them.
