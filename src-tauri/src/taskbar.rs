//! A count on the taskbar icon, for sessions waiting while aiterm is behind
//! another window.
//!
//! Not Tauri's `set_badge_count`: on Linux that goes through libunity, which it
//! dlopens and then gates on Unity actually running. Neither is true on a KDE
//! or GNOME desktop, so the call succeeds and does nothing. What Plasma (and
//! Unity's descendants, and GNOME with an extension) actually listen for is the
//! `com.canonical.Unity.LauncherEntry.Update` signal on the session bus, which
//! is a plain D-Bus emission any process can make. Verified live on Plasma
//! before this was written: the icon took the number.
//!
//! Emitted by shelling out to `gdbus` rather than linking a D-Bus crate — the
//! same trade as reading OpenCode's database with `sqlite3` and talking to
//! OpenRouter with `curl`. The payload carries no user text, only a number, so
//! there is nothing here that could be made to escape the argument.

use std::{
    io,
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc, Arc, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// The desktop file this badge attaches to. Derived from the crate name so a
/// rename cannot silently detach the badge from the installed
/// `/usr/share/applications/<name>.desktop`.
const DESKTOP_ID: &str = concat!("application://", env!("CARGO_PKG_NAME"), ".desktop");

const OBJECT_PATH: &str = concat!(
    "/com/canonical/unity/launcherentry/",
    env!("CARGO_PKG_NAME")
);

/// The GVariant dictionary Plasma reads. Zero hides the badge rather than
/// drawing a "0", which would be a worse lie than showing nothing.
pub fn payload(count: u32) -> String {
    if count == 0 {
        "{'count': <int64 0>, 'count-visible': <false>}".to_string()
    } else {
        format!("{{'count': <int64 {count}>, 'count-visible': <true>}}")
    }
}

/// A single worker serializes badge updates. A one-slot wakeup and a latest-value
/// register coalesce bursts without blocking the UI or accumulating subprocesses.
struct BadgeWorker {
    latest: Arc<AtomicU32>,
    wake: mpsc::SyncSender<()>,
}

impl BadgeWorker {
    fn start(emit: impl Fn(u32) + Send + 'static) -> io::Result<Self> {
        let latest = Arc::new(AtomicU32::new(0));
        let count = latest.clone();
        let (wake, updates) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("aiterm-taskbar".into())
            .spawn(move || {
                while updates.recv().is_ok() {
                    emit(count.load(Ordering::Acquire));
                }
            })?;
        Ok(Self { latest, wake })
    }

    fn submit(&self, count: u32) {
        self.latest.store(count, Ordering::Release);
        // Full means a wakeup is already pending; that wakeup reads the newest count.
        let _ = self.wake.try_send(());
    }
}

/// Set (or with 0, clear) the number on the taskbar icon. Never wait for D-Bus
/// from Tauri's UI thread. Missing/unsupported desktop badges are best-effort.
#[tauri::command]
pub fn taskbar_badge(count: u32) {
    static WORKER: OnceLock<Option<BadgeWorker>> = OnceLock::new();
    if let Some(worker) = WORKER.get_or_init(|| BadgeWorker::start(emit_badge).ok()) {
        worker.submit(count);
    }
}

fn emit_badge(count: u32) {
    let mut command = Command::new("gdbus");
    command.args([
        "emit",
        "--session",
        "--object-path",
        OBJECT_PATH,
        "--signal",
        "com.canonical.Unity.LauncherEntry.Update",
        DESKTOP_ID,
        &payload(count),
    ]);
    if let Err(error) = run_helper(command, Duration::from_secs(2)) {
        tracing::debug!(%error, "taskbar badge helper unavailable");
    }
}

fn run_helper(mut command: Command, timeout: Duration) -> io::Result<ExitStatus> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // A helper (or descendant) touching /dev/tty must not stop AITerm's process
    // group with SIGTTIN/SIGTTOU. Null stdin alone cannot protect /dev/tty access.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let started = Instant::now();
    let error = loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                break io::Error::new(io::ErrorKind::TimedOut, "taskbar badge helper timed out")
            }
            Err(error) => break error,
        }
    };
    // Kill the helper group, including a stopped helper and any children it
    // spawned, then reap our child. This group never includes AITerm itself.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_is_shown_as_a_visible_number() {
        assert_eq!(payload(3), "{'count': <int64 3>, 'count-visible': <true>}");
    }

    #[test]
    fn zero_hides_the_badge_rather_than_drawing_a_zero() {
        let p = payload(0);
        assert!(p.contains("'count-visible': <false>"), "{p}");
    }

    #[test]
    fn the_badge_targets_the_desktop_file_that_ships_with_the_package() {
        // If the rpm installs aiterm.desktop, this must name it exactly, or
        // the signal is emitted for an application nothing on the bar matches.
        assert_eq!(DESKTOP_ID, "application://aiterm.desktop");
    }

    #[test]
    fn the_only_thing_interpolated_is_a_decimal_number() {
        // Why this can be a shell-out with no quoting worries: the sole
        // substitution is a u32, so no input can reach the argument.
        for n in [1u32, 42, u32::MAX] {
            assert_eq!(
                payload(n),
                format!("{{'count': <int64 {n}>, 'count-visible': <true>}}")
            );
        }
    }
    #[test]
    fn busy_worker_returns_immediately_and_eventually_emits_the_latest_count() {
        let (seen, received) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let worker = BadgeWorker::start(move |count| {
            seen.send(count).unwrap();
            if count == 1 {
                blocked.recv().unwrap();
            }
        })
        .unwrap();
        worker.submit(1);
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        // The helper is blocked, but these UI-side calls must still return.
        for count in 2..=100 {
            worker.submit(count);
        }
        release.send(()).unwrap();
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), 100);
        worker.submit(0);
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn helper_has_null_stdin_and_its_own_process_group() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#"test "$(readlink /proc/self/fd/0)" = /dev/null && test "$(ps -o pgid= -p $$ | tr -d ' ')" = "$$""#]);
        assert!(run_helper(command, Duration::from_secs(2))
            .unwrap()
            .success());
    }

    #[cfg(unix)]
    #[test]
    fn stopped_helper_is_timed_out_and_reaped_without_stopping_the_caller() {
        let mut command = Command::new("/bin/sh");
        // Signal exactly the helper's group. With isolation missing, that group
        // does not exist: the command exits and the timeout assertion fails.
        command.args(["-c", "kill -TTIN -$$; exit 17"]);
        let error = run_helper(command, Duration::from_millis(300)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // A later badge still works after the stopped helper was killed/reaped.
        let mut next = Command::new("/bin/sh");
        next.args(["-c", "exit 0"]);
        assert!(run_helper(next, Duration::from_secs(2)).unwrap().success());
    }
}
