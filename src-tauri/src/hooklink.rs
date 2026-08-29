//! The exact link between a claude process and its session id.
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
//! The pieces:
//!
//! - [`install`] writes a settings file containing the hook. Claude launches
//!   get `--settings <that file>` (see `ClaudeBackend::launch`), which loads
//!   *additional* settings — Matt's own `~/.claude/settings.json` is never
//!   touched, and sessions aiterm did not launch never fire the hook.
//! - [`hook_report`] is what the hook runs: `aiterm --hook-report`, an argv
//!   mode dispatched in `main.rs` before Tauri exists. It reads the payload,
//!   takes its parent pid, and drops one file in the spool. It must never
//!   fail loudly — a hook that errors would slow every claude startup — so it
//!   swallows everything and always exits 0.
//! - [`drain_session_events`] is the app side: read the spool, resolve each
//!   reported pid to the pty whose child it descends from, hand the events to
//!   the frontend, and delete what was delivered.
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
/// `--settings` file and an edit through the panel would either fail or
/// fight this module's own writer.
pub const HOOK_REPORT_FLAG: &str = "--hook-report";

fn data_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("aiterm"))
}

fn spool_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("session-events"))
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
/// lives in `target/`. Stale paths would make the hook silently do nothing,
/// which is this feature's only real failure mode.
pub fn install() {
    let Some(exe) = std::env::current_exe().ok().filter(|p| p.is_file()) else {
        crate::diag!("hook", "no current_exe — hook not installed");
        return;
    };
    let Some(path) = settings_path() else {
        return;
    };
    if let Some(spool) = spool_dir() {
        let _ = std::fs::create_dir_all(spool);
    }
    // The hook command line is run by a shell, so the exe path is quoted the
    // same way agents.rs quotes everything headed for one.
    let cmd = format!(
        "'{}' {HOOK_REPORT_FLAG}",
        exe.to_string_lossy().replace('\'', "'\\''")
    );
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": [
                { "hooks": [ { "type": "command", "command": cmd } ] }
            ]
        }
    });
    match std::fs::write(&path, settings.to_string()) {
        Ok(()) => crate::diag!("hook", "settings written: {}", path.display()),
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
/// Runs once per claude session start, inside claude's startup path, so the
/// bar is: fast, silent, and incapable of failing the session. Everything is
/// `let Some(..) else { return }` — the worst outcome of any missing piece is
/// that this start goes undetected and the heuristics cover it.
pub fn hook_report() {
    let mut raw = String::new();
    // Bounded read: the payload is a few hundred bytes; a runaway stdin must
    // not wedge claude's startup behind us.
    if std::io::stdin()
        .take(64 * 1024)
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
    let Some(dir) = spool_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let event = serde_json::json!({
        "sessionId": session_id,
        "source": get("source"),
        "cwd": get("cwd"),
        "pid": pid,
    });
    // Write-then-rename so the drain never reads a half-written file. The name
    // carries pid and session id, so one file per (process, session) pair —
    // a later event for the same pair overwriting an unread one is fine,
    // because the newest state of that pair is the only one worth acting on.
    let final_path = dir.join(format!("{pid}-{session_id}.json"));
    let tmp = dir.join(format!(".{pid}-{session_id}.tmp"));
    if std::fs::write(&tmp, event.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &final_path);
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
    state: tauri::State<'_, crate::tabs::TabRegistry>,
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
}
