//! The exact link between a claude process and its session id, and the
//! harness's own account of what that session is doing.
//!
//! Every heuristic aiterm has for "this tab's conversation changed its session
//! id" — mtime ordering, transcript markers, uuid overlap — exists because
//! nothing on disk ties a *process* to a *session*. This module is that tie,
//! built from the one thing Claude Code will actually tell us: a `SessionStart`
//! hook receives the new session id on stdin, names its cause (`startup`,
//! `resume`, `clear`, `compact`), and runs as a child of the claude process
//! itself. aiterm spawned that process's pty, so pid → pty → tab is a lookup,
//! not an inference. *All probed 2026-07-29: payload shape, `source` values,
//! and `PPID == claude` verified against Claude Code 2.1.220.*
//!
//! The same settings file now carries the hooks that say what a session is
//! DOING — `PreToolUse`, `PostToolUse`, `Notification`, `PermissionRequest`,
//! `UserPromptSubmit`, `Stop` — which reach the spine as `phase` events. A
//! hook is the harness talking about itself: where terminal cadence cannot
//! tell a permission dialog from a TUI repaint, and the transcript only says
//! what has already been written, `Notification/permission_prompt` names the
//! prompt the moment it goes up. *Payloads probed live against Claude Code
//! 2.1.259, 2026-09-02: see `hook_verdict`.*
//!
//! The pieces:
//!
//! - [`install`] writes a settings file containing the hooks. Claude launches
//!   get `--settings <that file>` (see `ClaudeBackend::launch`), which loads
//!   *additional* settings — Matt's own `~/.claude/settings.json` is never
//!   touched, and sessions aiterm did not launch never fire the hooks.
//! - [`hook_report`] is what every one of them runs: `aiterm --hook-report`,
//!   an argv mode dispatched in `main.rs` before Tauri exists. It reads the
//!   payload, takes its parent pid, and drops one file in a spool. It must
//!   never fail loudly — a hook that errors or dawdles would slow every tool
//!   call — so it swallows everything and always exits 0.
//! - [`drain_session_events`] is the app side of the `SessionStart` spool:
//!   resolve each reported pid to the pty whose child it descends from, hand
//!   the events to the frontend, and delete what was delivered.
//! - [`start_hook_drain`] is the app side of the phase spool: an inotify
//!   watch on the spool directory, so a hook's word reaches the phone in
//!   milliseconds rather than on somebody's poll.
//!
//! The heuristics stay, demoted to fallback: a claude typed into a shell tab,
//! or a session already running when aiterm restarts, has no hook wired in.

use std::io::Read;
use std::path::PathBuf;

/// The argv flag that identifies aiterm's own hook.
///
/// Public and used everywhere the marker matters, so a rename here cannot
/// silently drift from what `main.rs` dispatches on or what `claudecfg`
/// matches against to recognise aiterm's own hook — the config panel must
/// never offer that hook for editing, since it lives in aiterm's own
/// `--settings` file and an edit through the panel would either fight this
/// module's own writer or fail.
pub const HOOK_REPORT_FLAG: &str = "--hook-report";

/// Every event aiterm asks Claude Code for, all running the same binary.
///
/// `SessionStart` is the process→session link; the rest are phase. Two are
/// deliberately absent: `MessageDisplay` would duplicate on the phone what
/// the transcript adapter already streams as blocks, and `PostToolUseFailure`
/// says nothing about the session's state that `PostToolUse` does not.
///
/// *Probed against 2.1.259: `PermissionRequest` fires only when a permission
/// dialog is actually displayed — it did NOT fire for an auto-allowed Bash —
/// so unlike grok's `permission_requested` it can be believed on its own.*
const HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "PermissionRequest",
    "Stop",
];

/// Claude Code counts hook timeouts in seconds. The work is one small file
/// write; anything approaching this is a machine in trouble, and claude
/// should not wait on us for it.
const HOOK_TIMEOUT_SECS: u64 = 5;

/// How long the phase drain waits before looking anyway. The inotify watch
/// is what actually delivers — this is only there for the case where the
/// watch could not be armed at all.
const DRAIN_FALLBACK: std::time::Duration = std::time::Duration::from_secs(2);

fn data_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("aiterm"))
}

fn spool_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("session-events"))
}

/// The phase spool, separate from the `SessionStart` one on purpose: that
/// one is polled by the frontend and keyed one-file-per-process, this one is
/// drained by the app and keyed one-file-per-event. Mixing them would mean
/// two readers racing over one directory for two different jobs.
fn hook_spool_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("hook-events"))
}

/// The one definition of where the hook settings file lives. Public because
/// the config panel (`claudecfg`) reads it too — it must show the path the
/// hook writer actually uses, not a guess that can drift under a non-default
/// `XDG_DATA_HOME`.
pub fn settings_path() -> Option<PathBuf> {
    Some(data_dir()?.join("claude-hook-settings.json"))
}

/// Write the settings file the hook flag points at. Called once at startup.
///
/// Rewritten every launch on purpose: the file embeds the absolute path of
/// this binary, and the binary moves — an RPM upgrade replaces it, a dev build
/// lives in `target/`. Stale paths would make the hooks silently do nothing,
/// which is this feature's only real failure mode.
pub fn install() {
    let Some(exe) = std::env::current_exe().ok().filter(|p| p.is_file()) else {
        crate::diag!("hook", "no current_exe — hooks not installed");
        return;
    };
    let Some(path) = settings_path() else {
        return;
    };
    for dir in [spool_dir(), hook_spool_dir()].into_iter().flatten() {
        let _ = std::fs::create_dir_all(dir);
    }
    // The hook command line is run by a shell, so the exe path is quoted the
    // same way agents.rs quotes everything headed for one.
    let cmd = format!(
        "'{}' {HOOK_REPORT_FLAG}",
        exe.to_string_lossy().replace('\'', "'\\''")
    );
    // Matcher-less: every entry of every event, whatever the tool. Filtering
    // is this side's job, and a matcher that stopped matching (a renamed
    // tool) would fail silently.
    let hooks: serde_json::Map<String, serde_json::Value> = HOOK_EVENTS
        .iter()
        .map(|event| {
            (
                (*event).to_string(),
                serde_json::json!([ {
                    "hooks": [ {
                        "type": "command",
                        "command": cmd,
                        "timeout": HOOK_TIMEOUT_SECS,
                    } ]
                } ]),
            )
        })
        .collect();
    let settings = serde_json::json!({ "hooks": hooks });
    match std::fs::write(&path, settings.to_string()) {
        Ok(()) => crate::diag!(
            "hook",
            "settings written ({} events): {}",
            HOOK_EVENTS.len(),
            path.display()
        ),
        Err(e) => crate::diag!("hook", "couldn't write {}: {e}", path.display()),
    }
}

/// The `--settings <file>` fragment for claude launches, shell-quoted, or
/// nothing if [`install`] never managed to write the file — a claude launched
/// without the flag merely falls back to the heuristics.
pub fn settings_flag() -> Option<String> {
    let path = settings_path().filter(|p| p.is_file())?;
    Some(format!(
        " --settings '{}'",
        path.to_string_lossy().replace('\'', "'\\''")
    ))
}

/// The `aiterm --hook-report` argv mode: stdin in, one spool file out.
///
/// Runs inside claude's own path — at session start, before and after every
/// tool call, at every prompt — so the bar is: fast, silent, and incapable of
/// failing anything. Everything is `let Some(..) else { return }`, and the
/// worst outcome of any missing piece is that one event goes unseen.
pub fn hook_report() {
    let mut raw = String::new();
    // Bounded read: a runaway stdin must not wedge claude behind us. Most
    // payloads are a few hundred bytes, but `PostToolUse` carries the whole
    // `tool_response` — a Read of a large file — and a payload clipped by
    // this bound is no longer JSON, so the bound has to sit well clear of
    // any real one rather than at a tidy 1 MB.
    if std::io::stdin()
        .take(16 * 1024 * 1024)
        .read_to_string(&mut raw)
        .is_err()
    {
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default();
    let session_id = get("session_id");
    if session_id.is_empty() {
        return;
    }
    // Our parent is the claude process itself — the fact that makes the whole
    // link exact. Verified: a SessionStart hook's PPID commandline was the
    // `claude` invocation, not a shell.
    let pid = std::os::unix::process::parent_id();
    let event = get("hook_event_name");
    // An older Claude Code that sends no event name can only be the one hook
    // there used to be.
    if event.is_empty() || event == "SessionStart" {
        spool_session_start(session_id, get("source"), get("cwd"), pid);
        return;
    }
    spool_hook_event(&v, pid);
}

/// The `SessionStart` spool: one file per (process, session) pair.
///
/// Write-then-rename so the drain never reads a half-written file. A later
/// event for the same pair overwriting an unread one is fine, because the
/// newest state of that pair is the only one worth acting on.
fn spool_session_start(session_id: &str, source: &str, cwd: &str, pid: u32) {
    let Some(dir) = spool_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let event = serde_json::json!({
        "sessionId": session_id,
        "source": source,
        "cwd": cwd,
        "pid": pid,
    });
    let final_path = dir.join(format!("{pid}-{session_id}.json"));
    let tmp = dir.join(format!(".{pid}-{session_id}.tmp"));
    if std::fs::write(&tmp, event.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &final_path);
    }
}

/// The phase spool: one file per event, named for the nanosecond it was
/// written so the drain can replay them in order — a `PreToolUse` and the
/// `PostToolUse` that closes it are two processes, and `read_dir` has no
/// opinion about which came first.
///
/// The payload is copied through under Claude Code's own field names,
/// trimmed rather than reshaped: [`hook_verdict`] then reads a spooled file
/// and a verbatim hook payload with the same code, which is what lets the
/// tests be written against the shapes claude actually sends.
fn spool_hook_event(v: &serde_json::Value, pid: u32) {
    let Some(dir) = hook_spool_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut out = serde_json::Map::new();
    for key in [
        "hook_event_name",
        "session_id",
        "cwd",
        "transcript_path",
        "notification_type",
        "message",
        "tool_name",
        "tool_use_id",
        "permission_mode",
    ] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            out.insert(key.to_string(), serde_json::json!(clip(s, 400)));
        }
    }
    if let Some(input) = v.get("tool_input") {
        out.insert("tool_input".to_string(), trim_tool_input(input));
    }
    out.insert("pid".to_string(), serde_json::json!(pid));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Zero-padded: the names are sorted as strings, and 20 digits is past
    // the year 5138.
    let name = format!("{stamp:020}-{pid}");
    let tmp = dir.join(format!(".{name}.tmp"));
    if std::fs::write(&tmp, serde_json::Value::Object(out).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(format!("{name}.json")));
    }
}

/// What a tool call is being made against, and nothing else. A `tool_input`
/// can be a whole file's contents (Write) or a multi-page prompt (Task);
/// none of that belongs in a spool file whose only use is one line of human
/// text.
fn trim_tool_input(v: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(obj) = v.as_object() {
        for key in TOOL_SUBJECT_KEYS {
            if let Some(s) = obj.get(key).and_then(|x| x.as_str()) {
                out.insert(key.to_string(), serde_json::json!(clip(s, 200)));
            }
        }
    }
    serde_json::Value::Object(out)
}

/// The `tool_input` fields that name what a tool is acting on, best first.
/// Bash's `command`, then the file/path/pattern/url the rest of the built-in
/// tools use. An MCP tool with none of them is named by its tool name alone.
const TOOL_SUBJECT_KEYS: [&str; 6] = [
    "command",
    "file_path",
    "path",
    "pattern",
    "url",
    "notebook_path",
];

/// Clip to `max` characters (not bytes — a clip mid-codepoint panics).
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// One line of at most `max` characters, ellipsis where it was cut. Hook
/// details are read on a phone's status line: a command's newlines and runs
/// of spaces would only make it taller.
fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    format!("{}…", flat.chars().take(max).collect::<String>())
}

/// "running Bash: npm test", "running Edit: src/main.rs", "running Task".
fn tool_detail(tool: &str, input: Option<&serde_json::Value>) -> String {
    if tool.is_empty() {
        return String::new();
    }
    let subject = input
        .and_then(|v| v.as_object())
        .and_then(|o| {
            TOOL_SUBJECT_KEYS
                .iter()
                .find_map(|k| o.get(*k).and_then(|x| x.as_str()))
        })
        .map(|s| one_line(s, 60))
        .filter(|s| !s.is_empty());
    match subject {
        Some(s) => format!("running {tool}: {s}"),
        None => format!("running {tool}"),
    }
}

/// "permission: Edit", from whichever of the two records names the tool.
///
/// `PermissionRequest` carries `tool_name`; `Notification` carries only
/// prose, and 2.1.259 phrases it "Claude needs your permission to use Bash"
/// — so the tool name is recoverable from it, and anything else is passed
/// through as the harness wrote it.
fn permission_detail(message: &str, tool: &str) -> String {
    let subject = if !tool.is_empty() {
        tool.to_string()
    } else {
        let m = message.trim().trim_end_matches('.');
        // 2.1.259 says "Claude needs your permission to use Bash", and
        // "…to read files outside the workspace" for the sandbox ones. Both
        // prefixes are the same sentence about the same thing; the phone
        // already knows whose permission is being asked for.
        let m = m
            .strip_prefix("Claude needs your permission to use ")
            .or_else(|| m.strip_prefix("Claude needs your permission to "))
            .unwrap_or(m);
        one_line(m, 60)
    };
    if subject.is_empty() {
        "permission".to_string()
    } else {
        format!("permission: {subject}")
    }
}

/// A hook payload → the session it belongs to and what it says that session
/// is doing. `None` for anything the spine has no use for.
///
/// The payloads, probed live against Claude Code 2.1.259 on 2026-09-02 (all
/// of them also carry `session_id`, `cwd`, `transcript_path`,
/// `hook_event_name`, `permission_mode` and `prompt_id`):
///
/// - `Notification`: `notification_type`, `message`. The type is one of
///   `permission_prompt`, `idle_prompt`, `auth_success`,
///   `elicitation_dialog`, `agent_needs_input`, `agent_completed`, … — only
///   the first two say anything about this session's state.
/// - `PermissionRequest`: `tool_name`, `tool_input`, `permission_suggestions`.
/// - `PreToolUse`: `tool_name`, `tool_input`, `tool_use_id`.
/// - `PostToolUse`: the same plus `tool_response` and `duration_ms` — the
///   docs' name for it is `tool_response`, not `tool_result`.
/// - `UserPromptSubmit`: `prompt`.
/// - `Stop`: `stop_hook_active`, `last_assistant_message`. There is no
///   `stop_reason` — which is why `Stopped` recomputes a verdict rather than
///   claiming one, and why hooks emit no `turn_ended`: the transcript's
///   `turn_duration` remains the single source of turn events.
pub(crate) fn hook_verdict(
    v: &serde_json::Value,
) -> Option<(String, crate::spine::registry::HookPhase)> {
    use crate::spine::registry::HookPhase;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default();
    let session_id = get("session_id");
    if session_id.is_empty() {
        return None;
    }
    let phase = match get("hook_event_name") {
        "Notification" => match get("notification_type") {
            "permission_prompt" => HookPhase::NeedsYou(permission_detail(get("message"), "")),
            // Claude asking to be spoken to. Not "attention" — nothing is
            // half-done behind it — just a session with nothing to do.
            "idle_prompt" => HookPhase::Idle,
            _ => return None,
        },
        "PermissionRequest" => {
            HookPhase::NeedsYou(permission_detail(get("message"), get("tool_name")))
        }
        "PreToolUse" => HookPhase::Working(tool_detail(get("tool_name"), v.get("tool_input"))),
        // The tool is done: working, and the empty detail is what clears the
        // tool's name from the phone.
        "PostToolUse" => HookPhase::Working(String::new()),
        // The same, plus the turn bracket — a person has spoken, and the
        // transcript's user line saying so is up to a poll behind.
        "UserPromptSubmit" => HookPhase::TurnOpened,
        "Stop" => HookPhase::Stopped,
        _ => return None,
    };
    Some((session_id.to_string(), phase))
}

/// Watch the phase spool and drain it as it fills.
///
/// inotify, not a poll: a permission dialog reaching the phone half a second
/// late is the whole difference this feature is for. The 2 s tick behind it
/// only matters on a system where the watch could not be armed.
pub fn start_hook_drain(app: tauri::AppHandle) {
    use notify::Watcher;
    let Some(dir) = hook_spool_dir() else { return };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::diag!("hook", "no phase spool at {}: {e}", dir.display());
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    // Kept alive alongside the watcher, so `rx.recv()` pends forever rather
    // than resolving instantly (and spinning the loop) when no watch armed.
    let keepalive = tx.clone();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })
    .ok()
    .and_then(|mut w| {
        match w.watch(&dir, notify::RecursiveMode::NonRecursive) {
            Ok(()) => Some(w),
            Err(e) => {
                crate::diag!("hook", "phase spool watch failed, falling back to poll: {e}");
                None
            }
        }
    });
    tauri::async_runtime::spawn(async move {
        let _watcher = watcher;
        let _keepalive = keepalive;
        let mut tick = tokio::time::interval(DRAIN_FALLBACK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = rx.recv() => {}
                _ = tick.tick() => {}
            }
            drain_hook_events(&app).await;
        }
    });
}

/// Every spooled hook event, oldest first, onto the spine as a phase.
///
/// A file is deleted as it is read, whatever comes of it: a phase is status,
/// and a status nobody could place is worth nothing a second later. Unlike
/// the `SessionStart` spool there is no retry — that one waits for a pty to
/// appear in the registry, where this one needs only a session id it already
/// has, and `push_hook_phase` drops what has no tail on its own.
async fn drain_hook_events(app: &tauri::AppHandle) {
    let Some(dir) = hook_spool_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    // The names are zero-padded nanosecond stamps: name order is time order.
    files.sort();
    for path in files {
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Ok(raw) = raw else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let Some((session_id, phase)) = hook_verdict(&v) else {
            continue;
        };
        crate::diag!(
            "hook",
            "{} {} → {phase:?}",
            &session_id[..8.min(session_id.len())],
            v.get("hook_event_name").and_then(|x| x.as_str()).unwrap_or("?")
        );
        crate::spine::registry::push_hook_phase(app, &session_id, phase).await;
    }
}

/// A session start the hook reported, resolved to its authoritative Rust tab.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvent {
    pub tab_id: crate::tabs::TabId,
    pub tab: crate::tabs::TabDescriptor,
    pub session_id: String,
    /// What began the session: "startup", "resume", "clear", "compact".
    pub source: String,
}

/// Deliver the spool: every event whose claude could be traced to one of our
/// ptys, deleting delivered and expired files as it goes.
///
/// An event that resolves to no pty is kept and retried — the file watcher can
/// outrun the pty registry by a few milliseconds — until it is a minute old,
/// at which point it is somebody else's claude (or a previous aiterm's) and is
/// discarded. Polled by the frontend; reading an empty directory is the whole
/// cost of the quiet case.
#[tauri::command]
pub fn drain_session_events(
    state: tauri::State<'_, std::sync::Arc<crate::tabs::TabRegistry>>,
) -> Vec<SessionEvent> {
    let Some(dir) = spool_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&raw).ok();
        let event = parsed.as_ref().and_then(|v| {
            let pid = v.get("pid")?.as_u64()? as u32;
            let session_id = v.get("sessionId")?.as_str()?.to_string();
            let source = v
                .get("source")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string();
            Some((pid, session_id, source))
        });
        match event.and_then(|(pid, session_id, source)| {
            let tab_id = state.tab_for_descendant(pid)?;
            let current = state.get(&tab_id).ok()?;
            let tab = if current.session_id().is_some()
                && current.session_id() != Some(session_id.as_str())
            {
                let new_conversation = source == "clear" || source == "fork";
                let mut update = crate::tabs::TabUpdate::new()
                    .session_id(session_id.clone())
                    .slot_id(session_id.clone());
                if new_conversation {
                    update = update.fresh(true);
                    if let Some(title) = current
                        .cwd()
                        .and_then(|cwd| std::path::Path::new(cwd).file_name())
                        .and_then(|name| name.to_str())
                        .filter(|name| !name.is_empty())
                    {
                        update = update.title(title);
                    }
                }
                state.update(&tab_id, update).ok()?
            } else {
                current
            };
            Some(SessionEvent {
                tab_id,
                tab,
                session_id,
                source,
            })
        }) {
            Some(event) => {
                out.push(event);
                let _ = std::fs::remove_file(&path);
            }
            None => {
                // Unreadable, unparseable, or unresolvable. Give a fresh file
                // time to resolve; expire the rest.
                let age = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok());
                if age.is_none_or(|a| a.as_secs() > 60) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    for e in &out {
        crate::diag!(
            "hook",
            "session {} started ({}) in tab {}",
            e.session_id,
            e.source,
            e.tab_id
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spine::registry::HookPhase;

    /// The spool write and the drain agree on a format; this pins the half a
    /// test can reach without a live pty: parsing and expiry are exercised
    /// through `drain_session_events` in integration, so here we hold the
    /// event JSON shape the hook writes to what the drain reads.
    #[test]
    fn spool_event_roundtrips() {
        let event = serde_json::json!({
            "sessionId": "abc-123",
            "source": "clear",
            "cwd": "/home/x",
            "pid": 4242u32,
        });
        let v: serde_json::Value = serde_json::from_str(&event.to_string()).unwrap();
        assert_eq!(v["sessionId"].as_str(), Some("abc-123"));
        assert_eq!(v["pid"].as_u64(), Some(4242));
        assert_eq!(v["source"].as_str(), Some("clear"));
    }

    /// Every hook aiterm asks for is one this side knows what to do with,
    /// and every event it maps is one it actually asked for. A hook added to
    /// the settings file with no arm here fires for nothing; an arm with no
    /// hook can never run.
    #[test]
    fn the_installed_hooks_and_the_understood_events_are_the_same_set() {
        let understood = [
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Notification",
            "PermissionRequest",
            "Stop",
        ];
        for event in understood {
            assert!(HOOK_EVENTS.contains(&event), "{event} is mapped but never installed");
            let payload = serde_json::json!({
                "session_id": "s1",
                "hook_event_name": event,
                "notification_type": "permission_prompt",
            });
            assert!(
                hook_verdict(&payload).is_some(),
                "{event} is installed but reaches no phase"
            );
        }
        // SessionStart is the one hook that is not phase at all: it takes
        // the other spool and must never come out of `hook_verdict`.
        assert!(HOOK_EVENTS.contains(&"SessionStart"));
        assert_eq!(
            hook_verdict(&serde_json::json!({
                "session_id": "s1",
                "hook_event_name": "SessionStart",
                "source": "startup",
            })),
            None
        );
    }

    /// Verbatim payloads, as Claude Code 2.1.259 wrote them to a probe hook
    /// on 2026-09-02 (`session_id`/`transcript_path` shortened). If a future
    /// version renames a field, this is where it shows.
    #[test]
    fn the_payloads_claude_actually_sends_map_to_a_phase() {
        let pre = serde_json::json!({
            "session_id": "9e2f1a44-0000-4000-8000-000000000001",
            "transcript_path": "/home/admin/.claude/projects/-tmp/9e2f1a44.jsonl",
            "cwd": "/tmp/probe/work",
            "prompt_id": "a897ed61-03aa-4748-b4a8-6937a2ad62ca",
            "permission_mode": "default",
            "effort": { "level": "high" },
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "echo hello-hooks", "description": "Print hello-hooks" },
            "tool_use_id": "toolu_016gTgVgopxeT9R5kbAetiJz"
        });
        assert_eq!(
            hook_verdict(&pre),
            Some((
                "9e2f1a44-0000-4000-8000-000000000001".to_string(),
                HookPhase::Working("running Bash: echo hello-hooks".to_string())
            ))
        );

        let post = serde_json::json!({
            "session_id": "s1",
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "echo hello-hooks" },
            "tool_response": { "stdout": "hello-hooks", "stderr": "", "interrupted": false },
            "tool_use_id": "toolu_016gTgVgopxeT9R5kbAetiJz",
            "duration_ms": 142
        });
        assert_eq!(
            hook_verdict(&post),
            Some(("s1".to_string(), HookPhase::Working(String::new()))),
            "the tool is done: working, and the detail cleared"
        );

        let prompt = serde_json::json!({
            "session_id": "s1",
            "hook_event_name": "UserPromptSubmit",
            "prompt": "run the bash command 'echo hello-hooks' and then say done"
        });
        assert_eq!(
            hook_verdict(&prompt),
            Some(("s1".to_string(), HookPhase::TurnOpened))
        );

        let stop = serde_json::json!({
            "session_id": "s1",
            "hook_event_name": "Stop",
            "stop_hook_active": false,
            "last_assistant_message": "done",
            "background_tasks": [],
            "session_crons": []
        });
        assert_eq!(hook_verdict(&stop), Some(("s1".to_string(), HookPhase::Stopped)));

        // Notification, from the shape the binary builds:
        // {hook_event_name, message, title, notification_type}.
        let ask = serde_json::json!({
            "session_id": "s1",
            "hook_event_name": "Notification",
            "notification_type": "permission_prompt",
            "title": "Claude Code",
            "message": "Claude needs your permission to use Bash"
        });
        assert_eq!(
            hook_verdict(&ask),
            Some(("s1".to_string(), HookPhase::NeedsYou("permission: Bash".to_string()))),
            "the tool is dug out of the harness's own sentence"
        );

        let idle = serde_json::json!({
            "session_id": "s1",
            "hook_event_name": "Notification",
            "notification_type": "idle_prompt",
            "message": "Claude is waiting for your input"
        });
        assert_eq!(hook_verdict(&idle), Some(("s1".to_string(), HookPhase::Idle)));

        // Every other notification type says nothing about this session.
        for kind in ["auth_success", "elicitation_dialog", "push_notification", ""] {
            let other = serde_json::json!({
                "session_id": "s1",
                "hook_event_name": "Notification",
                "notification_type": kind,
                "message": "…"
            });
            assert_eq!(hook_verdict(&other), None, "{kind} should be ignored");
        }

        let perm = serde_json::json!({
            "session_id": "s1",
            "hook_event_name": "PermissionRequest",
            "tool_name": "Edit",
            "tool_input": { "file_path": "/home/admin/src/main.rs" },
            "permission_suggestions": []
        });
        assert_eq!(
            hook_verdict(&perm),
            Some(("s1".to_string(), HookPhase::NeedsYou("permission: Edit".to_string())))
        );

        // No session id, no session to speak about.
        assert_eq!(
            hook_verdict(&serde_json::json!({ "hook_event_name": "Stop" })),
            None
        );
        // An event we never asked for, arriving anyway.
        assert_eq!(
            hook_verdict(&serde_json::json!({
                "session_id": "s1",
                "hook_event_name": "MessageDisplay",
                "message": "hi"
            })),
            None
        );
    }

    /// The detail is one line, clipped, and names what is being acted on.
    #[test]
    fn a_tool_detail_names_its_subject_in_one_short_line() {
        let bash = serde_json::json!({ "command": "npm test\n  --watch" });
        assert_eq!(tool_detail("Bash", Some(&bash)), "running Bash: npm test --watch");
        let edit = serde_json::json!({ "file_path": "src/main.rs", "old_string": "a" });
        assert_eq!(tool_detail("Edit", Some(&edit)), "running Edit: src/main.rs");
        // Nothing recognisable to name it by: the tool alone.
        let mcp = serde_json::json!({ "query": "x" });
        assert_eq!(tool_detail("mcp__x__y", Some(&mcp)), "running mcp__x__y");
        assert_eq!(tool_detail("Task", None), "running Task");
        assert_eq!(tool_detail("", None), "");
        // 60 characters and an ellipsis, never a panic mid-codepoint.
        let long = "é".repeat(200);
        let d = tool_detail("Bash", Some(&serde_json::json!({ "command": long })));
        assert_eq!(d.chars().count(), "running Bash: ".len() + 61);
        assert!(d.ends_with('…'));
    }

    /// The permission detail prefers the tool name, falls back to the
    /// harness's own words, and always says what it is.
    #[test]
    fn a_permission_detail_says_what_is_being_asked_for() {
        assert_eq!(permission_detail("", "Bash"), "permission: Bash");
        assert_eq!(
            permission_detail("Claude needs your permission to use Bash", ""),
            "permission: Bash"
        );
        assert_eq!(
            permission_detail("Claude needs your permission to read files outside the workspace.", ""),
            "permission: read files outside the workspace"
        );
        assert_eq!(permission_detail("", ""), "permission");
    }

    /// The spool file is a payload, trimmed — so the drain reads it with the
    /// same code that reads what claude sent, and the verdict is identical.
    #[test]
    fn a_spooled_file_verdicts_the_same_as_the_payload_it_came_from() {
        let payload = serde_json::json!({
            "session_id": "s1",
            "cwd": "/tmp",
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            // The whole point of trimming: this must not reach the spool.
            "tool_input": { "file_path": "/tmp/x.rs", "content": "x".repeat(100_000) },
            "tool_use_id": "toolu_1"
        });
        let mut trimmed = serde_json::Map::new();
        for key in ["hook_event_name", "session_id", "cwd", "tool_name", "tool_use_id"] {
            trimmed.insert(key.into(), payload[key].clone());
        }
        trimmed.insert("tool_input".into(), trim_tool_input(&payload["tool_input"]));
        let spooled = serde_json::Value::Object(trimmed);
        assert!(spooled.to_string().len() < 400, "a spool file stays small");
        assert_eq!(hook_verdict(&spooled), hook_verdict(&payload));
        assert_eq!(
            hook_verdict(&spooled).unwrap().1,
            HookPhase::Working("running Write: /tmp/x.rs".to_string())
        );
    }
}
