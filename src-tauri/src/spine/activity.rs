//! Session activity inference used by the spine.

use std::path::PathBuf;
use std::time::Duration;

/// The tail of `path`, at most `keep` bytes. `None` for a missing file or a
/// tail that is not valid UTF-8 from the seek point — the same shrug the
/// transcript read below gives.
fn tail_of(path: &std::path::Path, keep: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(keep))).ok()?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// The verdict from grok's explicit state events, read off the tail of the
/// session dir's `events.jsonl`. [observed: grok 1.0.13]
///
/// Grok now writes codex-style turn brackets plus something neither other
/// engine records — an explicit waiting-on-a-person event:
///
/// ```text
/// {"ts":"…","type":"turn_started","session_id":"…","turn_number":0,"model_id":"grok-4.6",…}
/// {"ts":"…","type":"permission_requested","tool_name":"write"}
/// {"ts":"…","type":"permission_resolved","tool_name":"write","decision":"allow","wait_ms":0}
/// {"ts":"…","type":"turn_ended","outcome":"completed"}
/// ```
///
/// These are transcript facts and outrank the open-tool_call + cadence
/// inference (HARNESS-CONTRACT.md, "The state machine"): an open bracket is
/// working, an unresolved `permission_requested` is attention with no
/// 45-second wait, and a closed bracket is idle even when `chat_history.jsonl`
/// ends on a bare user/tool_result line from a killed run — the case the
/// inference reads as stuck-working forever. A cancelled turn is still
/// `turn_ended` (`outcome:"cancelled"`), so the bracket closes either way.
///
/// Nested option: `Some(state)` is a verdict (`Some(None)` = idle); the
/// outer `None` means the tail carries no bracket at all — an events file
/// from before the first turn, or a tail cut inside one turn's phase spam —
/// and the caller falls back to the chat_history inference, which is also
/// all that older grok sessions (no events.jsonl) have.
fn grok_events_state(text: &str) -> Option<Option<&'static str>> {
    let (mut open_turn, mut open_permission, mut saw_bracket) = (false, false, false);
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("turn_started") => {
                saw_bracket = true;
                open_turn = true;
                open_permission = false;
            }
            Some("turn_ended") => {
                saw_bracket = true;
                open_turn = false;
                open_permission = false;
            }
            Some("permission_requested") => open_permission = true,
            Some("permission_resolved") => open_permission = false,
            _ => {}
        }
    }
    if open_permission {
        // A fact on its own: the prompt is up whether or not the tail still
        // holds the turn_started that preceded it.
        return Some(Some("attention"));
    }
    saw_bracket.then(|| open_turn.then_some("working"))
}

/// The verdict from an antigravity transcript tail
/// (`~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`).
/// [observed: agy 1.1.24]
///
/// agy writes one record per step, and the step's `type` says where the
/// turn is: a `USER_INPUT` is a prompt the model has not answered; a
/// `PLANNER_RESPONSE` carrying `tool_calls` is a call whose result has not
/// landed — attention when one of them is `ask_question`,
/// `ask_permission` or `ask_custom_permission`, the tools agy lists for
/// putting a question to the person; a `GENERIC` step is that result, which
/// the model now has to act on; a `PLANNER_RESPONSE` with `content` and no
/// calls is the answer, and the turn is over. `SYSTEM_MESSAGE` (the
/// "server restart" notice every resume adds) changes nothing. No process
/// check, exactly as grok's events arm: a killed run mid-turn reads working
/// until its next resume, which is the inference's known limit. And on an
/// account with `toolPermission: always-proceed` (this one) the ask_* tools
/// never fire, so attention never does either.
///
/// Nested option as [`grok_events_state`]: outer `None` = no record in the
/// tail; `Some(None)` = idle; `Some(Some(_))` = working or attention.
fn antigravity_transcript_state(text: &str) -> Option<Option<&'static str>> {
    let mut verdict: Option<Option<&'static str>> = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("USER_INPUT") | Some("GENERIC") => verdict = Some(Some("working")),
            Some("PLANNER_RESPONSE") => {
                let calls = v
                    .get("tool_calls")
                    .and_then(|c| c.as_array())
                    .filter(|c| !c.is_empty());
                verdict = Some(match calls {
                    Some(calls) => {
                        let asks = calls.iter().any(|c| {
                            matches!(
                                c.get("name").and_then(|n| n.as_str()),
                                Some("ask_question" | "ask_permission" | "ask_custom_permission")
                            )
                        });
                        Some(if asks { "attention" } else { "working" })
                    }
                    None => None,
                });
            }
            _ => {}
        }
    }
    verdict
}

/// Whether agy's transcript ends on a tool call whose result has not
/// landed: the last step is a `PLANNER_RESPONSE` carrying `tool_calls`,
/// with no `GENERIC` (the result) after it. That is the only shape a
/// confirmation dialog can be sitting behind.
fn antigravity_open_call(text: &str) -> bool {
    let mut open = false;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("PLANNER_RESPONSE") => {
                open = v
                    .get("tool_calls")
                    .and_then(|c| c.as_array())
                    .is_some_and(|c| !c.is_empty());
            }
            // The result landing, or a new prompt, closes it.
            Some("GENERIC") | Some("USER_INPUT") => open = false,
            _ => {}
        }
    }
    open
}

/// agy's own log. A symlink into `log/cli-<stamp>.log` re-pointed on each
/// run; `metadata` and `File::open` both follow it, so this always reads
/// the current run's file.
pub(crate) fn antigravity_log_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".gemini/antigravity-cli/cli.log"))
}

/// The time in a glog header, as ms since the epoch.
///
/// `I0902 21:38:28.616360` is September 2nd at 21:38:28.616 LOCAL time —
/// glog writes no year and no zone. The year is this one, minus one when
/// that would place the line in the future (a December log read in
/// January). The line may be prefixed by agy's
/// `ERROR: logging before google.Init: `, so the header is found rather
/// than assumed to be first.
fn glog_time_ms(line: &str) -> Option<u64> {
    let mut fields = line.split_whitespace();
    let stamp = loop {
        let field = fields.next()?;
        // `I` + MMDD: the severity letter and the date, glued.
        let (Some(sev), true) = (field.chars().next(), field.len() == 5) else {
            continue;
        };
        if !matches!(sev, 'I' | 'W' | 'E' | 'F') || !field[1..].bytes().all(|b| b.is_ascii_digit())
        {
            continue;
        }
        break field;
    };
    let month: i32 = stamp[1..3].parse().ok()?;
    let day: i32 = stamp[3..5].parse().ok()?;
    let clock = fields.next()?;
    let mut parts = clock.split(':');
    let hour: i32 = parts.next()?.parse().ok()?;
    let minute: i32 = parts.next()?.parse().ok()?;
    // `unwrap_or` evaluates its argument, so the field is taken once and
    // then split — asking `parts` for it twice consumed the iterator.
    let seconds = parts.next()?;
    let (sec, frac) = seconds.split_once('.').unwrap_or((seconds, "0"));
    let second: i32 = sec.parse().ok()?;
    // glog writes microseconds; take whatever precision is actually there.
    let millis: u64 = format!("{frac:0<3}")[..3].parse().ok()?;

    // `mktime` is what turns a local civil time into an instant: it knows
    // this machine's zone and its DST rule, which no amount of arithmetic
    // here would. `tm_isdst = -1` asks it to work out which side of a
    // transition the time falls on.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let year = unsafe {
        let t = now as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        tm.tm_year
    };
    let at = |year: i32| -> Option<u64> {
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            tm.tm_year = year;
            tm.tm_mon = month - 1;
            tm.tm_mday = day;
            tm.tm_hour = hour;
            tm.tm_min = minute;
            tm.tm_sec = second;
            tm.tm_isdst = -1;
            let t = libc::mktime(&mut tm);
            (t != -1).then(|| t as u64 * 1000 + millis)
        }
    };
    let this_year = at(year)?;
    // More than a day ahead means the log rolled over a new year under us.
    if this_year > (now + 86_400) * 1000 {
        return at(year - 1);
    }
    Some(this_year)
}

/// Is agy sitting on a tool confirmation right now?
///
/// A permission dialog is INVISIBLE to the transcript: agy writes the
/// `PLANNER_RESPONSE` carrying the call and then nothing at all — no
/// `ask_*` tool, no further step — while its TUI waits for a person. The
/// only record anywhere is one line in agy's own log. Observed live: a
/// `run_command` sat on its dialog for minutes while the spine read
/// "working", with `tool_confirmation_manager.go:197] Surfacing tool
/// confirmation: "RunCommand" at step 2` the sole evidence.
/// [observed: agy 1.1.24, 2026-09-02]
///
/// `since_ms` is the transcript's mtime: a confirmation line NEWER than
/// the last thing the transcript learned is one still unanswered, because
/// answering it writes the result step and moves the transcript past it.
///
/// The log carries no conversation id, so this cannot say WHICH session
/// was asked. One agy TUI at a time is the normal case and the signal is
/// right for it; with two open, both sessions with an open call would read
/// `attention` off one prompt. Accepted: a false "come and look" on a
/// second session costs a glance, and the alternative is missing every
/// real one.
fn antigravity_confirmation_after(since_ms: u64) -> bool {
    let Some(path) = antigravity_log_path() else {
        return false;
    };
    // 64 KB is many minutes of agy's chatter; the line we want is at the
    // very end of the file when it matters at all.
    let Some(text) = tail_of(&path, 64 * 1024) else {
        return false;
    };
    text.lines()
        .filter(|l| l.contains("Surfacing tool confirmation"))
        .filter_map(glog_time_ms)
        .any(|at| at > since_ms)
}

/// When the transcript's verdict replaces what the terminal reported.
/// Cadence may promote to working, but it must not HOLD working against a
/// transcript that says a person is being waited on: codex's TUI keeps
/// animating (a ticking elapsed counter) while its approval dialog is up,
/// so cadence never goes quiet and, left alone, a session mid-approval
/// reads "working" forever — a brought-in codex sat exactly there
/// [observed: codex-cli 0.150.1]. Idle from cadence yields to any
/// transcript verdict (the old rule); attention beats working (this one).
/// A cadence "working" is never demoted to idle from here — output is
/// output.
fn transcript_outranks(terminal: &str, transcript: &str) -> bool {
    terminal == "idle" || (transcript == "attention" && terminal == "working")
}

/// The single place one session's activity is decided: the tab's output
/// cadence, corrected by what the session's own files say and by what a
/// Claude Code hook said as it happened. The sessions list and the spine's
/// phase tick both come through here, so neither can
/// hold a verdict the other would not.
///
/// Returns the verdict and a short human detail — "" when the source has
/// nothing to add beyond the state itself.
pub(crate) fn activity_verdict(
    terminal: Option<&str>,
    transcript: Option<(&'static str, &'static str)>,
    turn_open: Option<bool>,
    hook_attention: bool,
) -> (&'static str, &'static str) {
    // `session_activities` spells cadence "output"; the phone's session
    // state, the spine's phases and `transcript_outranks` all speak
    // working/attention/idle. Normalising here is what lets the rule below
    // fire at all: against the raw "output" spelling `transcript_outranks`
    // matched neither arm, so a codex parked on an approval kept reading as
    // busy — the exact case that rule was written for.
    let mut cadence: &'static str = match terminal {
        Some("output" | "working") => "working",
        Some("attention") => "attention",
        _ => "idle",
    };
    // Cadence is bytes on a pty, and a TUI goes on repainting after the
    // answer is finished — a spinner clearing, a footer redrawn, the prompt
    // coming back. Held on its own it kept the phone's header on "working"
    // for the ten seconds `session_activities` counts as recent, well after
    // the turn had visibly ended [observed: Claude Code, 2026-09-02]. So
    // when the spine's adapter has told us the turn is closed, cadence may
    // no longer promote to working. It may still say attention, and a new
    // `turn_started` re-opens the gate within a poll of the user's line
    // being written. `None` — the legacy adapter reports no turns at all —
    // leaves the old rule exactly as it was.
    if cadence == "working" && turn_open == Some(false) {
        cadence = "idle";
    }
    let verdict = match transcript {
        Some((state, detail)) if transcript_outranks(cadence, state) => (state, detail),
        _ => (cadence, ""),
    };
    // A Claude Code hook said a permission dialog is up. That is the harness
    // announcing its own state as it happens — not a file read after the
    // fact, not bytes on a pty — so it is the one input here that is not an
    // inference, and nothing below it may demote it. Cadence in particular
    // would: claude's TUI redraws its own dialog, so the pty is busy for as
    // long as the person takes to answer. It stands until a later hook (the
    // tool running, the turn ending) or the transcript retires it. A
    // transcript that already says attention keeps its own reason, which is
    // more specific than this one; the caller replaces even that with the
    // hook's detail when it has one ("permission: Edit").
    if hook_attention && verdict.0 != "attention" {
        return ("attention", "permission");
    }
    verdict
}

/// `Some(("working", …))`, `Some(("attention", …))` — codex mid-approval, or
/// a grok permission prompt — or `None`.
/// Public within the crate: the pty layer consults it before believing a
/// quiet terminal means an idle session, and the spine's phase tick turns
/// it into a `phase` event.
/// Codex writes nothing while its approval prompt is up, so "waiting on a
/// person" is read as: a turn in progress whose last act is a tool call
/// with no output, and a transcript that has gone quiet. Grok ≥1.0.13 writes
/// explicit events instead ([`grok_events_state`]), which short-circuit that
/// inference for grok sessions only.
///
/// The second half is a short human reason, "" when there is none. It is
/// never inferred: each return below names only what the record it read
/// actually says.
pub(crate) fn transcript_verdict(session_id: &str) -> Option<(&'static str, &'static str)> {
    // OpenCode sessions live in a SQLite store, not a transcript file —
    // `owner_in` resolves one to `opencode.db` itself, and the tail read
    // below then fails UTF-8 on binary SQLite into a silent `None`, every
    // call. Answer from the store instead: the newest assistant message row
    // with `time.completed` still NULL is a turn in flight; completed means
    // no busy claim. A killed run leaves the NULL forever, so "working" also
    // requires a live process holding the session (argv naming the id, or an
    // `opencode` in the session's directory for a fresh launch whose argv
    // names no session yet). No needs-you verdict exists to give: OpenCode's
    // permission config auto-answers, and its TUI emits no OSC 9;4 and no
    // bell — output cadence and this bracket are the only signals.
    // [observed: opencode 1.18.25]
    if crate::opencode::valid_id(session_id) {
        return match crate::opencode::open_turn(session_id) {
            Some((true, dir))
                if crate::sessions::opencode_process_alive(session_id, dir.as_deref()) =>
            {
                Some(("working", ""))
            }
            _ => None,
        };
    }
    let list = crate::agents::backends();
    let Some((_, path)) = crate::agents::owner_in(&list, session_id) else {
        return None;
    };
    // Grok ≥1.0.13: the transcript sits in a session DIRECTORY named by the
    // session id, and `events.jsonl` beside it carries explicit state events
    // that replace the inference below. Grok only by construction: claude and
    // codex transcripts never sit in a directory named after their session,
    // so they cannot take this branch. Older grok sessions have no
    // events.jsonl and fall through to the open-tool_call inference.
    // [observed: grok 1.0.13]
    if let Some(dir) = path
        .parent()
        .filter(|d| d.file_name().is_some_and(|n| n == session_id))
    {
        if let Some(text) = tail_of(&dir.join("events.jsonl"), 256 * 1024) {
            if let Some(verdict) = grok_events_state(&text) {
                // That function returns attention for exactly one reason —
                // an unresolved `permission_requested`; nothing else in
                // events.jsonl can produce it — so naming the reason here
                // reads the record rather than guessing at it.
                return verdict.map(|s| (s, if s == "attention" { "permission" } else { "" }));
            }
        }
    }
    // Antigravity: the transcript sits under `…/antigravity-cli/brain/<id>/`
    // and its step types say where the turn is — the generic parser below
    // knows none of them, so the verdict comes from the tail alone.
    // [observed: agy 1.1.24]
    if path.to_string_lossy().contains("/antigravity-cli/brain/") {
        let text = tail_of(&path, 256 * 1024)?;
        let verdict = antigravity_transcript_state(&text).flatten();
        // An open call plus a confirmation line newer than the transcript
        // is a dialog still on screen — the one state agy's own records
        // cannot express. See `antigravity_confirmation_after`.
        if verdict == Some("working") && antigravity_open_call(&text) {
            let written = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if antigravity_confirmation_after(written) {
                return Some(("attention", "permission"));
            }
        }
        // agy's other attention is an unanswered `ask_question` /
        // `ask_permission` / `ask_custom_permission` call.
        return verdict.map(|s| (s, if s == "attention" { "permission" } else { "" }));
    }
    let stale = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e > Duration::from_secs(45));
    let Ok(mut f) = std::fs::File::open(&path) else {
        return None;
    };
    use std::io::{Read, Seek, SeekFrom};
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(128 * 1024);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return None;
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return None;
    }
    let mut state: Option<bool> = None;
    let mut pending_call = false;
    // Codex-shaped records seen: gates the no-pending-call attention
    // fallback below to codex rollouts only.
    let mut saw_codex = false;
    for line in buf.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("event_msg") => {
                saw_codex = true;
                match v.pointer("/payload/type").and_then(|t| t.as_str()) {
                    Some("task_started") => {
                        state = Some(true);
                        pending_call = false
                    }
                    Some("task_complete") | Some("turn_aborted") => {
                        state = Some(false);
                        pending_call = false
                    }
                    _ => {}
                }
            }
            Some("turn_context") | Some("session_meta") => saw_codex = true,
            Some("response_item") => {
                saw_codex = true;
                match v.pointer("/payload/type").and_then(|t| t.as_str()) {
                    Some("custom_tool_call") | Some("function_call") => pending_call = true,
                    Some("custom_tool_call_output") | Some("function_call_output") => {
                        pending_call = false
                    }
                    _ => {}
                }
            }
            Some("user") => {
                // A tool result is Claude talking to itself, not a new ask.
                let is_result = v
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                    .is_some_and(|a| {
                        a.iter()
                            .all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                    });
                if !is_result {
                    state = Some(true);
                } else {
                    state = Some(true); // mid-turn: the model has a result to act on
                }
            }
            Some("assistant") => {
                // Text without a tool call ends the turn; a tool call means
                // more to come. Claude nests tool_use in /message/content;
                // grok puts tool_calls at the top of the line.
                let claude_tool = v
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                    .is_some_and(|a| {
                        a.iter()
                            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                    });
                let grok_tool = v
                    .get("tool_calls")
                    .and_then(|c| c.as_array())
                    .is_some_and(|a| !a.is_empty());
                state = Some(claude_tool || grok_tool);
            }
            // Grok writes tool results as their own lines: the model has a
            // result to act on, so the turn is still going.
            Some("tool_result") => state = Some(true),
            _ => {}
        }
    }
    match state {
        Some(true) if pending_call && stale => Some(("attention", "a tool call is waiting")),
        // Codex asks for command approval BEFORE writing the exec record, so
        // a dialog can be up with NO unanswered call on disk — a live stuck
        // approval showed exactly that: open turn, all steps completed, phone
        // said "working" [observed: codex-cli 0.150.1, 2026-08-31; the audit
        // found no approval record type in any rollout 0.144→0.150.1]. For
        // codex files only: an open turn that has written nothing for 45s is
        // a person being waited on — or a wedge, which wants the same glance.
        // Claude keeps the pending-call requirement: its long silent Bash
        // calls are routine, and its prompts ring the terminal bell instead.
        Some(true) if saw_codex && stale => Some(("attention", "approval")),
        Some(true) => Some(("working", "")),
        _ => None,
    }
}
