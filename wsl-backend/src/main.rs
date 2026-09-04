//! One desktop-owned Linux terminal per companion process. No network listener.
use aiterm_wsl_protocol::{read_frame, write_frame, Event, Request, OUTPUT_WINDOW, VERSION};
use base64::{engine::general_purpose::STANDARD, Engine};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::{Arc, Condvar, Mutex};
#[allow(dead_code)]
#[path = "../../src-tauri/src/fsx.rs"]
mod fsx;
#[allow(dead_code)]
#[path = "../../src-tauri/src/git.rs"]
mod git;
mod rpc;

// RPCs already run in a separate companion process; no GUI thread to block.
async fn run_blocking<T>(work: impl FnOnce() -> T) -> T {
    work()
}

#[derive(Default)]
struct Credits {
    pending: VecDeque<(u64, usize)>,
    bytes: usize,
    sent: u64,
    closed: bool,
}
type Window = Arc<(Mutex<Credits>, Condvar)>;

fn emit(event: &Event) -> io::Result<()> {
    write_frame(&mut io::stdout().lock(), event)
}

// A terminal is an owned process group. Closing a desktop terminal must also
// release commands that ignore SIGHUP, not leave them running invisibly in WSL.
struct SessionGuard {
    pid: u32,
    window: Window,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        let (lock, wake) = &*self.window;
        lock.lock().unwrap().closed = true;
        wake.notify_all();
        // Interactive shells put foreground jobs in separate process groups.
        // Release every process still in this PTY's session, not just the shell
        // group. A pidfd pins each target while we verify its session identity.
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<i32>().ok())
                else {
                    continue;
                };
                unsafe {
                    let fd = libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32;
                    if fd >= 0 {
                        if libc::getsid(pid) == self.pid as i32 {
                            libc::syscall(
                                libc::SYS_pidfd_send_signal,
                                fd,
                                libc::SIGKILL,
                                std::ptr::null::<libc::siginfo_t>(),
                                0,
                            );
                        }
                        libc::close(fd);
                    }
                }
            }
        }
    }
}

fn size(cols: u16, rows: u16) -> Result<PtySize, String> {
    if !(2..=1000).contains(&cols) || !(1..=1000).contains(&rows) {
        return Err("Invalid terminal dimensions".into());
    }
    Ok(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = io::stdin().lock();
    let Some(Request::Start {
        version,
        cols,
        rows,
        cwd,
        command: launch,
    }) = read_frame(&mut input)?
    else {
        return Err("Expected startup handshake".into());
    };
    if version != VERSION {
        return Err("Desktop and Linux workspace versions do not match".into());
    }
    let home = std::env::var("HOME")?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let pair = native_pty_system().openpty(size(cols, rows)?)?;
    let mut command = CommandBuilder::new(&shell);
    command.arg("-l");
    if let Some(launch) = launch {
        command.arg("-c");
        command.arg(launch);
    }
    command.cwd(cwd.as_deref().unwrap_or(&home));
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    // Match desktop PTY isolation: keep user configuration, drop parent markers.
    for key in [
        "CLAUDECODE",
        "CLAUDE_CODE_AGENT",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_EXECPATH",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_EFFORT",
        "CLAUDE_JOB_DIR",
        "CLAUDE_PID",
    ] {
        command.env_remove(key);
    }
    let mut child = pair.slave.spawn_command(command)?;
    let pid = child
        .process_id()
        .ok_or("Shell did not report its process ID")?;
    let window: Window = Default::default();
    let guard = SessionGuard {
        pid,
        window: window.clone(),
    };
    drop(pair.slave);
    let mut output = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    emit(&Event::Ready {
        version: VERSION,
        home,
        shell,
        pid,
    })?;
    let (exit_tx, exit_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = exit_tx.send(child.wait());
    });
    let output_window = window.clone();
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            let n = match output.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let (lock, wake) = &*output_window;
            let mut credits = lock.lock().unwrap();
            while !credits.closed && credits.bytes + n > OUTPUT_WINDOW {
                credits = wake.wait(credits).unwrap();
            }
            if credits.closed {
                return;
            }
            credits.sent += 1;
            let sequence = credits.sent;
            credits.pending.push_back((sequence, n));
            credits.bytes += n;
            drop(credits);
            if emit(&Event::Output {
                sequence,
                data: STANDARD.encode(&buffer[..n]),
            })
            .is_err()
            {
                return;
            }
        }
        // All PTY bytes precede the final exit event, including the last chunk.
        let status = exit_rx.recv().ok().and_then(Result::ok);
        let _ = emit(&Event::Exit {
            code: status.as_ref().map(|s| s.exit_code()),
            signal: status.as_ref().and_then(|s| s.signal().map(str::to_owned)),
        });
    });
    while let Some(request) = read_frame(&mut input)? {
        match request {
            Request::Input { data } => {
                writer.write_all(&STANDARD.decode(data)?)?;
                writer.flush()?;
            }
            Request::Resize { cols, rows } => pair.master.resize(size(cols, rows)?)?,
            Request::Ack { sequence } => {
                let (lock, wake) = &*window;
                let mut credits = lock.lock().unwrap();
                if sequence > credits.sent {
                    return Err("Invalid output acknowledgement".into());
                }
                while credits.pending.front().is_some_and(|(s, _)| *s <= sequence) {
                    let (_, n) = credits.pending.pop_front().unwrap();
                    credits.bytes -= n;
                }
                wake.notify_all();
            }
            Request::Close => break,
            Request::Start { .. } => return Err("Terminal is already started".into()),
        }
    }
    drop(guard);
    Ok(())
}

fn main() {
    if std::env::args().any(|arg| arg == "--rpc") {
        rpc::serve();
        return;
    }
    if let Err(error) = run() {
        let _ = emit(&Event::Error {
            message: error.to_string(),
        });
        std::process::exit(1);
    }
}
