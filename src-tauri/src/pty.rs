use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, State};

pub struct PtyInstance {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// The pty's direct child — the login shell, not the command it runs.
    /// Killing only this leaves the real process orphaned; see `pty_kill`.
    child_pid: Option<u32>,
}

#[derive(Default)]
pub struct PtyManager {
    ptys: Mutex<HashMap<u32, PtyInstance>>,
    next_id: AtomicU32,
}

#[derive(Clone, Serialize)]
struct PtyExit {
    id: u32,
    /// The child's exit status, or `None` if it could not be reaped.
    ///
    /// This is the whole difference between "you left" and "something killed
    /// it". A shell you typed `exit` into leaves 0; a `claude` killed from
    /// `claude agents` — possibly from another terminal, possibly from a
    /// phone — does not. Without this the UI cannot tell the two apart, and it
    /// treated every death as a deliberate close: the tab vanished with no
    /// explanation and no way back but hunting the session down in the sidebar.
    code: Option<u32>,
}

#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    state: State<'_, PtyManager>,
    cwd: Option<String>,
    command: Option<String>,
    cols: u16,
    rows: u16,
    on_output: Channel<InvokeResponseBody>,
) -> Result<u32, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let mut cmd = match &command {
        // Run through the login shell so PATH/aliases resolve like a normal terminal.
        Some(c) => {
            let mut b = CommandBuilder::new(&shell);
            b.args(["-i", "-c", c]);
            b
        }
        None => CommandBuilder::new(&shell),
    };
    cmd.env("TERM", "xterm-256color");
    if let Some(dir) = cwd.filter(|d| std::path::Path::new(d).is_dir()) {
        cmd.cwd(dir);
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    let child_pid = child.process_id();
    let killer = child.clone_killer();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let id = state.next_id.fetch_add(1, Ordering::SeqCst);
    state.ptys.lock().unwrap().insert(
        id,
        PtyInstance {
            master: pair.master,
            writer,
            killer,
            child_pid,
        },
    );

    let app_reader = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Send raw bytes over a binary Channel — no JSON string
                    // serialization, and no per-chunk `from_utf8_lossy`, which
                    // used to corrupt any multibyte char (box-drawing borders,
                    // emoji) straddling the 8 KB read boundary. xterm decodes
                    // the byte stream with a persistent UTF-8 decoder, so a char
                    // split across two chunks is reassembled correctly.
                    let _ = on_output.send(InvokeResponseBody::Raw(buf[..n].to_vec()));
                }
            }
        }
        // Reap the child before announcing the exit. The read loop ends when
        // the pty closes, which says nothing about *why* — so wait for the
        // real status. This blocks only this pty's reader thread, and the
        // child is already gone by the time we get here in every path but a
        // detaching one, so the wait returns immediately.
        let code = child.wait().ok().map(|s| s.exit_code());
        let _ = app_reader.emit("pty://exit", PtyExit { id, code });
    });

    Ok(id)
}

#[tauri::command]
pub fn pty_write(state: State<'_, PtyManager>, id: u32, data: String) -> Result<(), String> {
    let mut ptys = state.ptys.lock().unwrap();
    let pty = ptys.get_mut(&id).ok_or("no such pty")?;
    pty.writer
        .write_all(data.as_bytes())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_resize(state: State<'_, PtyManager>, id: u32, cols: u16, rows: u16) -> Result<(), String> {
    let ptys = state.ptys.lock().unwrap();
    let pty = ptys.get(&id).ok_or("no such pty")?;
    pty.master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

/// True while `pid` names a *running* process. A killed child keeps its
/// `/proc/<pid>` entry until its parent reaps it, so presence alone would call
/// every zombie alive — and a "did the kill work?" check built on that never
/// succeeds. State `Z` counts as dead.
pub fn pid_alive(pid: u32) -> bool {
    let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
        return false;
    };
    !status
        .lines()
        .any(|l| l.starts_with("State:") && l.contains('Z'))
}

/// Every descendant of `root` (not including `root`), deepest last — read from
/// /proc rather than from process groups. `zsh -i -c <cmd>` runs with job
/// control on, so the command lands in its **own** process group; signalling
/// the shell's group misses it entirely.
fn descendants(root: u32) -> Vec<u32> {
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in procs.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        if let Some(ppid) = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .and_then(|v| v.trim().parse::<u32>().ok())
        {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut out = Vec::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for &kid in children.get(&pid).into_iter().flatten() {
            out.push(kid);
            queue.push(kid);
        }
    }
    out
}

/// Stop `root` and everything under it: SIGTERM, then SIGKILL whatever is left
/// after `grace`. Children go first so a shell can't respawn or reparent one
/// mid-teardown. Returns false if anything is still alive at the end.
pub fn kill_tree(root: u32, grace: std::time::Duration) -> bool {
    let mut tree = descendants(root);
    tree.reverse(); // deepest first
    tree.push(root);
    for &pid in &tree {
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if !tree.iter().any(|&p| pid_alive(p)) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for &pid in &tree {
        if pid_alive(pid) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    !tree.iter().any(|&p| pid_alive(p))
}

#[tauri::command]
pub fn pty_kill(state: State<'_, PtyManager>, id: u32) -> Result<(), String> {
    // Take the instance out under the lock, then release it: the kill below
    // can block for over a second, and holding the map would stall every other
    // tab's writes and resizes for that whole time.
    let taken = state.ptys.lock().unwrap().remove(&id);
    if let Some(mut pty) = taken {
        // `killer.kill()` only reaches the pty's direct child — the login
        // shell. zsh forks the command rather than exec'ing it, so killing the
        // shell orphaned every `claude` aiterm ever launched: they stayed in
        // `claude agents` forever, which made their rows permanently "running"
        // and left fork-a-copy as the only action the UI would offer.
        if let Some(pid) = pty.child_pid {
            kill_tree(pid, std::time::Duration::from_millis(1500));
        }
        let _ = pty.killer.kill();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The regression that started all of this: killing a shell does not kill
    /// what the shell forked. `zsh -i -c claude …` forks rather than execs, so
    /// every `claude` aiterm launched outlived its tab and stayed in the
    /// roster forever, which made its row permanently "running".
    #[test]
    fn kill_tree_reaps_grandchildren() {
        let mut sh = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .spawn()
            .expect("spawn sh");
        let root = sh.id();
        // Give the shell a moment to fork the `sleep`.
        let mut kids = Vec::new();
        for _ in 0..40 {
            kids = descendants(root);
            if !kids.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!kids.is_empty(), "sh never forked a child to test against");

        assert!(kill_tree(root, Duration::from_millis(1500)), "tree survived");
        assert!(!pid_alive(root), "shell still alive");
        for kid in kids {
            assert!(!pid_alive(kid), "grandchild {kid} outlived the kill");
        }
        let _ = sh.wait();
    }

    /// The whole "keep the tab when something killed it" behaviour rests on one
    /// assumption: that waiting on the pty's child after the read loop ends
    /// yields a status that tells clean exits apart from everything else. If
    /// this ever reports 0 for a killed process, the UI silently goes back to
    /// dropping tabs with no explanation — which is the bug it was built for.
    #[test]
    fn exit_status_separates_leaving_from_dying() {
        for (script, want) in [("exit 0", 0u32), ("exit 7", 7)] {
            let pair = native_pty_system()
                .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
                .expect("openpty");
            let mut cmd = CommandBuilder::new("sh");
            cmd.args(["-c", script]);
            let mut child = pair.slave.spawn_command(cmd).expect("spawn");
            drop(pair.slave);
            // Drain, exactly as the reader thread does: the wait can block
            // forever behind a pty whose output nobody is reading.
            let mut reader = pair.master.try_clone_reader().expect("reader");
            std::thread::spawn(move || {
                let mut sink = Vec::new();
                let _ = reader.read_to_end(&mut sink);
            });
            let status = child.wait().expect("wait");
            assert_eq!(status.exit_code(), want, "`{script}` reported the wrong status");
        }
    }

    /// A process killed by a signal must not look like a clean exit — that is
    /// the case a session stopped from `claude agents` actually takes.
    #[test]
    fn a_signalled_child_is_not_reported_as_clean() {
        let pair = native_pty_system()
            .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
            .expect("openpty");
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);
        let pid = child.process_id().expect("pid");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        std::thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = reader.read_to_end(&mut sink);
        });
        assert!(kill_tree(pid, Duration::from_millis(1500)), "tree survived");
        let status = child.wait().expect("wait");
        assert_ne!(status.exit_code(), 0, "a killed child looked like a clean exit");
    }

    #[test]
    fn pid_alive_tracks_reality() {
        assert!(pid_alive(std::process::id()));
        // Not a pid Linux will hand out (default pid_max is 4194304).
        assert!(!pid_alive(u32::MAX));
    }

    #[test]
    fn descendants_excludes_the_root_itself() {
        let me = std::process::id();
        assert!(!descendants(me).contains(&me));
    }
}
