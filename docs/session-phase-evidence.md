# Session status evidence

The desktop is the source of session phases sent to Android. Linux 0.10.92
removes a reproducible conflict between the transcript phase tick and terminal
output notifications. The old transcript inference called a Codex turn an
approval wait after 45 seconds without a transcript write, even during model
execution or a long command. The terminal path evaluated the same turn without
that transcript evidence and pushed Working again. An actual adapter question
or transcript permission also lost to the next repaint because only Claude
hooks were retained.

The regression tests reproduce those defects without requiring a live freeze:
`silence_is_not_a_permission_request` and
`explicit_adapter_question_survives_repaints_until_answered` fail on the prior
implementation. This identifies a real source-level mechanism, not a capture
of the user's particular 100 ms episode.

All producers now update retained evidence through one registry operation:

- Explicit hook and adapter phases retain their details until resolved by that
  source or retired by a new turn, completion, cancellation, or reset.
- Transcript permission evidence is shared with the terminal-output path.
- An open native turn remains Working through quiet periods. A closed native
  turn remains Idle through terminal redraws. Silence and pending tool calls
  cannot establish Needs you.
- Phase calculation, deduplication and event insertion use one registry lock.
  A transcript read carries an evidence revision and is discarded/retried if a
  newer hook, adapter phase, or turn boundary arrived while it was reading.
- Older transcript boundaries do not undo a newer hook's turn state. Source
  stamps retain subsecond mtime precision so rapid same-length changes can
  invalidate cached evidence.

`aiterm::phase` debug trace entries identify the session prefix, producing
source, sequence, epoch, previous/next phase, turn gate and evidence states.
They omit message text, tool arguments, file contents and permission details.
Enable the existing Diagnostics verbose trace to capture these entries after
starting the updated desktop.

A limitation remains explicit: Codex versions whose rollout contains no actual
approval request cannot distinguish that prompt from a quiet model/tool solely
from the rollout. They now stay Working instead of inventing Needs you. The
terminal remains available for inspecting and answering such a prompt. Actual
reported permission/question phases continue to work.

Android 0.3.29 already filters the old quiet-period guesses and keeps status
controls outside the scrolling conversation. This change fixes the producer;
it does not require another Android build. It takes effect when the updated
desktop backend starts. Desktop installation/restart remains on hold until Matt
releases his active sessions; the Windows VM remains off.

Validation: the full Rust library suite passed with 676 tests and 18 existing
ignored tests using `--test-threads=1`. The default highly parallel run hit
process file-limit/lease contention in four archive tests; the serial run
passed those too. The Windows WSL backend passed `cargo check`.
