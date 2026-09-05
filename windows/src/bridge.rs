use aiterm_wsl_protocol::{read_frame, write_frame, Event, Request, VERSION};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// Built on Linux/WSL before compiling the desktop. No runtime download needed.
const BACKEND: &[u8] = include_bytes!("../resources/aiterm-wsl-backend");

#[derive(Clone, Serialize)]
pub struct Workspace {
    pub distribution: String,
    pub home: String,
    pub shell: String,
}

pub(crate) fn wsl() -> Command {
    let mut cmd = Command::new("wsl.exe");
    cmd.env("WSL_UTF8", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cmd
}

fn text(bytes: &[u8]) -> String {
    if bytes.contains(&0) {
        String::from_utf16_lossy(
            &bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn capture(mut command: Command, input: Vec<u8>) -> Result<String, String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start WSL: {e}"))?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let sender = std::thread::spawn(move || stdin.write_all(&input));
    let read = |pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe
                .take((aiterm_wsl_protocol::MAX_FRAME + 1) as u64)
                .read_to_end(&mut buf);
            buf
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("WSL did not finish starting. Try opening your Linux distribution, then retry. {result:?}"));
            }
        }
    };
    let out = text(&out.join().unwrap_or_default());
    let err = text(&err.join().unwrap_or_default());
    if !status.success() {
        return Err(format!(
            "WSL could not prepare the Linux workspace. {} {}",
            out.trim(),
            err.trim()
        ));
    }
    sender
        .join()
        .map_err(|_| "Workspace installation was interrupted")?
        .map_err(|e| e.to_string())?;
    Ok(out)
}

pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Session {
    pub fn send(&mut self, request: &Request) -> Result<(), String> {
        write_frame(self.stdin.as_mut().ok_or("Terminal is closed")?, request)
            .map_err(|e| e.to_string())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // EOF tells the companion to release its PTY process group. Do not shut
        // down the distribution: other user applications may be running there.
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn start(
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    command: Option<String>,
    events: impl Fn(Event) -> bool + Send + 'static,
) -> Result<(Session, Workspace), String> {
    if !(2..=1000).contains(&cols) || !(1..=1000).contains(&rows) {
        return Err("Invalid terminal dimensions".into());
    }
    let (distribution, digest) = prepare()?;
    let launch = format!("exec \"$HOME/.local/share/aiterm/backends/{digest}\"");
    let mut child = wsl()
        .args([
            "--distribution",
            &distribution,
            "--exec",
            "sh",
            "-c",
            &launch,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdin = child.stdin.take();
    let mut session = Session { child, stdin };
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut ready = false;
        let mut ended = false;
        loop {
            match read_frame::<Event>(&mut reader) {
                Ok(Some(event)) => {
                    if !ready {
                        match &event {
                            Event::Ready {
                                version,
                                home,
                                shell,
                                ..
                            } if *version == VERSION => {
                                let _ = ready_tx.send(Ok((home.clone(), shell.clone())));
                                ready = true;
                            }
                            other => {
                                let _ = ready_tx.send(Err(format!(
                                    "Linux workspace startup failed: {other:?}"
                                )));
                                break;
                            }
                        }
                    }
                    ended |= matches!(event, Event::Exit { .. } | Event::Error { .. });
                    if !events(event) {
                        break;
                    }
                }
                result => {
                    if !ended {
                        events(Event::Error { message: format!("The Linux workspace connection ended. Open a new terminal to reconnect. {result:?}") });
                    }
                    break;
                }
            }
        }
    });
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.take(64 * 1024).read_to_string(&mut buf);
        if !buf.trim().is_empty() {
            eprintln!("WSL: {}", buf.trim());
        }
    });
    session.send(&Request::Start {
        version: VERSION,
        cols,
        rows,
        cwd,
        command,
    })?;
    let (home, shell) = ready_rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "Linux terminal did not become ready. Open your distribution and check that its shell starts.")??;
    Ok((
        session,
        Workspace {
            distribution,
            home,
            shell,
        },
    ))
}

pub(crate) fn prepare() -> Result<(String, String), String> {
    // Cache a successful installation, while retaining an explicit reconnect.
    static READY: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);
    let mut cached = READY.lock().map_err(|e| e.to_string())?;
    if let Some(value) = &*cached {
        return Ok(value.clone());
    }
    // Ask the default distribution itself, rather than parsing localized list
    // output or accidentally choosing Docker's internal distribution.
    let mut probe = wsl();
    probe.args(["--exec", "sh", "-c", "printf '%s\\n' \"$WSL_DISTRO_NAME\""]);
    let distribution = capture(probe, Vec::new())?.trim().to_owned();
    if distribution.is_empty() || distribution.starts_with("docker-desktop") {
        return Err(
            "Choose a Linux distribution such as Ubuntu as your WSL default, then retry.".into(),
        );
    }
    let digest = format!("{:x}", Sha256::digest(BACKEND));
    // The only interpolated component is our hex digest, never a user path.
    let install_script = format!("set -eu; umask 077; d=\"$HOME/.local/share/aiterm/backends\"; mkdir -p \"$d\"; if [ -x \"$d/{digest}\" ]; then cat >/dev/null; else t=$(mktemp \"$d/.install.XXXXXX\"); trap 'rm -f \"$t\"' EXIT; cat >\"$t\"; chmod 700 \"$t\"; mv \"$t\" \"$d/{digest}\"; fi");
    let mut install = wsl();
    install.args([
        "--distribution",
        &distribution,
        "--exec",
        "sh",
        "-c",
        &install_script,
    ]);
    capture(install, BACKEND.to_vec())?;

    let value = (distribution, digest);
    *cached = Some(value.clone());
    Ok(value)
}

pub fn rpc(command: String, args: serde_json::Value) -> Result<serde_json::Value, String> {
    let (distribution, digest) = prepare()?;
    if command == "open_path" {
        let path = args["path"].as_str().ok_or("Missing path")?;
        if path.starts_with("https://") || path.starts_with("http://") {
            Command::new("explorer.exe")
                .arg(path)
                .spawn()
                .map_err(|e| e.to_string())?;
            return Ok(serde_json::Value::Null);
        }
        if !path.starts_with('/') || path.contains('\\') || path.contains('\0') {
            return Err("Expected an absolute Linux path".into());
        }
        let unc = format!(
            "\\\\wsl.localhost\\{}{}",
            distribution,
            path.replace('/', "\\")
        );
        Command::new("explorer.exe")
            .arg(unc)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(serde_json::Value::Null);
    }
    let launch = format!("exec \"$HOME/.local/share/aiterm/backends/{digest}\" --rpc");
    let mut cmd = wsl();
    cmd.args([
        "--distribution",
        &distribution,
        "--exec",
        "sh",
        "-c",
        &launch,
    ]);
    let mut input = Vec::new();
    write_frame(
        &mut input,
        &serde_json::json!({"command":command,"args":args}),
    )
    .map_err(|e| e.to_string())?;
    let output = capture(cmd, input)?;
    let reply: serde_json::Value =
        serde_json::from_str(&output).map_err(|e| format!("Invalid workspace response: {e}"))?;
    if let Some(error) = reply["error"].as_str() {
        return Err(error.into());
    }
    Ok(reply["value"].clone())
}
