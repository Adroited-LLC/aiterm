//! Session activity inference used by the spine.

use std::path::PathBuf;

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

/// Resolve facts about a turn. Silence and terminal redraws cannot establish
/// a request for human input. Legacy timeout verdicts are rejected here too,
/// so cached evidence cannot reintroduce that inference.
pub(crate) fn activity_verdict(
    terminal: Option<&str>,
    transcript: Option<(&'static str, &'static str)>,
    turn_open: Option<bool>,
    hook_attention: bool,
) -> (&'static str, &'static str) {
    if hook_attention {
        return ("attention", "permission");
    }
    if turn_open == Some(false) {
        return ("idle", "");
    }
    if let Some(("attention", detail)) = transcript {
        if !matches!(detail, "approval" | "a tool call is waiting") {
            return ("attention", detail);
        }
    }
    if turn_open == Some(true)
        || matches!(transcript, Some(("working", _)))
        || matches!(terminal, Some("output" | "working"))
    {
        return ("working", "");
    }
    ("idle", "")
}

/// Read active turns and explicit permission/question records. A quiet
/// transcript is insufficient to distinguish model/tool execution from an
/// approval prompt; Codex rollouts without a permission record stay Working.
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
    transcript_turn_verdict(&buf)
}

fn transcript_turn_verdict(buf: &str) -> Option<(&'static str, &'static str)> {
    let mut state: Option<bool> = None;
    for line in buf.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("event_msg") => match v.pointer("/payload/type").and_then(|t| t.as_str()) {
                Some("task_started") => {
                    state = Some(true);
                }
                Some("task_complete") | Some("turn_aborted") => {
                    state = Some(false);
                }
                _ => {}
            },
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
        Some(true) => Some(("working", "")),
        _ => None,
    }
}

#[cfg(test)]
mod phase_regression_tests {
    use super::*;

    #[test]
    fn silence_is_not_a_permission_request() {
        for detail in ["approval", "a tool call is waiting"] {
            for cadence in ["output", "idle"] {
                assert_eq!(
                    activity_verdict(
                        Some(cadence),
                        Some(("attention", detail)),
                        Some(true),
                        false
                    ),
                    ("working", "")
                );
            }
        }
    }

    #[test]
    fn transcript_tail_uses_turn_boundaries_without_inventing_approval() {
        let open = r#"{"type":"event_msg","payload":{"type":"task_started"}}"#;
        let call = r#"{"type":"response_item","payload":{"type":"function_call","call_id":"one"}}"#;
        let output =
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"one"}}"#;
        let end = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        assert_eq!(transcript_turn_verdict(open), Some(("working", "")));
        assert_eq!(
            transcript_turn_verdict(&format!("{open}\n{call}")),
            Some(("working", ""))
        );
        assert_eq!(
            transcript_turn_verdict(&format!("{open}\n{call}\n{output}")),
            Some(("working", ""))
        );
        assert_eq!(transcript_turn_verdict(&format!("{open}\n{end}")), None);
    }

    #[test]
    fn open_turn_stays_working_without_terminal_output() {
        assert_eq!(
            activity_verdict(Some("idle"), None, Some(true), false),
            ("working", "")
        );
    }
}
