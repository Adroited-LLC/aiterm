//! A log file that survives the thing that wrote it.
//!
//! aiterm's diagnostics went to stderr, and a desktop launcher throws stderr
//! away. That is fine until something goes wrong once, at which point there is
//! nothing to read and the only way to see anything is to relaunch from a
//! terminal — which is exactly when the fault stops reproducing. The session
//! that exited with status 143 last night left no trace anywhere for this
//! reason.
//!
//! So: every diagnostic goes to `~/.local/share/aiterm/aiterm.log` as well as
//! stderr, timestamped, with the previous run kept alongside as `.log.1`.
//! Deliberately not a logging framework — this is a few lines that always work
//! over a dependency that needs configuring to do the same thing.
//!
//! Nothing here logs message text, file contents or credentials. It records
//! what aiterm did, not what you said.

use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;

fn log_dir() -> Option<std::path::PathBuf> {
    Some(dirs::data_dir()?.join("aiterm"))
}

/// The open log file, or `None` if we could not make one.
///
/// A machine where the log cannot be opened is one where aiterm should still
/// start: losing diagnostics is annoying, refusing to run over it is not a
/// trade anybody would take.
fn sink() -> Option<&'static Mutex<std::fs::File>> {
    static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let dir = log_dir()?;
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("aiterm.log");
        // Keep exactly one previous run. Enough to answer "what happened just
        // before I restarted it", which is the question that actually gets
        // asked, without growing without bound on a machine nobody prunes.
        let _ = std::fs::rename(&path, dir.join("aiterm.log.1"));
        std::fs::File::create(&path).ok().map(Mutex::new)
    })
    .as_ref()
}

/// Wall-clock, as `HH:MM:SS.mmm`, from the one clock we can read without a
/// date crate. Enough to line an entry up against something you just did.
fn stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let ms = now.subsec_millis();
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// Write one diagnostic line. Use through [`crate::diag!`].
pub fn write(area: &str, msg: &str) {
    let line = format!("{} [{area}] {msg}", stamp());
    eprintln!("[aiterm] {line}");
    if let Some(file) = sink() {
        if let Ok(mut f) = file.lock() {
            // Flushed per line on purpose: the entries that matter most are
            // the ones written immediately before the process dies, and a
            // buffered writer is precisely how those get lost.
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

/// Log a line to the file and stderr: `diag!("pty", "spawned {pid}")`.
#[macro_export]
macro_rules! diag {
    ($area:expr, $($arg:tt)*) => {
        $crate::diag::write($area, &format!($($arg)*))
    };
}

/// Where the log is, for the UI to offer to open it.
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub fn diag_log_path() -> Option<String> {
    log_dir().map(|d| d.join("aiterm.log").to_string_lossy().to_string())
}

/// The tail of the current log, for pasting into a bug report.
///
/// Bounded because the point is to hand somebody the last thing that happened,
/// and a log that has to be truncated before it can be read is one nobody
/// sends.
#[cfg_attr(not(aiterm_headless), tauri::command(async))]
pub fn diag_log_tail(lines: usize) -> String {
    let Some(path) = log_dir().map(|d| d.join("aiterm.log")) else {
        return String::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines.clamp(1, 5000))..].join("\n")
}

/// What aiterm is, where it came from, and what it can see.
///
/// The first three questions of any "it is behaving oddly" conversation,
/// answered in one place so nobody has to be talked through finding them.
#[cfg_attr(not(aiterm_headless), tauri::command(async))]
pub fn diag_environment() -> Vec<(String, String)> {
    let mut out = vec![
        ("version".into(), env!("CARGO_PKG_VERSION").to_string()),
        ("pid".into(), std::process::id().to_string()),
        (
            "shell".into(),
            std::env::var("SHELL").unwrap_or_else(|_| "<unset>".into()),
        ),
        (
            "desktop".into(),
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "<unset>".into()),
        ),
        (
            "session type".into(),
            std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "<unset>".into()),
        ),
    ];
    // Which agents aiterm can actually see, which is the answer to most
    // "why is it not offering X" questions.
    for d in crate::services::ApplicationServices::desktop()
        .agents
        .detect()
    {
        out.push((
            format!("agent: {}", d.display_name),
            match (d.available, d.version.as_deref()) {
                (true, Some(v)) => format!("{v} at {}", d.path.clone().unwrap_or_default()),
                (true, None) => d.path.clone().unwrap_or_else(|| "installed".into()),
                (false, _) => "not installed".into(),
            },
        ));
    }
    out
}
