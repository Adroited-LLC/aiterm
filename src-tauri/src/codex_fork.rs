//! Branch through Codex's own history machinery, without starting a turn or
//! attaching to the original terminal. The child is persisted before replying.
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub(super) fn fork(session_id: &str) -> Result<String, String> {
    let binary = super::which("codex")
        .or_else(|| super::which_via_login_shell("codex"))
        .ok_or("Codex is not installed or could not be found")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(binary);
    command.arg("app-server");
    runtime.block_on(fork_with(command, session_id, Duration::from_secs(30)))
}

// Separate process group: a helper must never stop the GUI's terminal group.
// Cleanup includes any helper descendants and runs on errors/timeouts too.
struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.0 != 0 {
            unsafe {
                libc::kill(-(self.0 as libc::pid_t), libc::SIGKILL);
            }
        }
    }
}

async fn fork_with(
    mut command: tokio::process::Command,
    session_id: &str,
    timeout: Duration,
) -> Result<String, String> {
    uuid::Uuid::parse_str(session_id).map_err(|_| "Invalid Codex session id")?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Couldn't start Codex branching helper: {e}"))?;
    let group = ProcessGroup(child.id().unwrap_or(0));
    let mut stdin = child.stdin.take().ok_or("Codex helper input unavailable")?;
    let mut stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or("Codex helper output unavailable")?,
    );
    let result = tokio::time::timeout(timeout, async {
        send(&mut stdin, json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"aiterm","version":env!("CARGO_PKG_VERSION")}}})).await?;
        reply(&mut stdout, &mut stdin, 1).await?;
        send(&mut stdin, json!({"method":"initialized","params":{}})).await?;
        send(&mut stdin, json!({"id":2,"method":"thread/fork","params":{"threadId":session_id,"ephemeral":false,"excludeTurns":true}})).await?;
        let response = reply(&mut stdout, &mut stdin, 2).await?;
        let id = persisted_branch(&response, session_id)?;
        Ok(id)
    }).await.unwrap_or_else(|_| Err("Codex branching timed out. Check the session list before trying again; Codex may have saved the branch.".into()));
    drop(stdin);
    drop(group);
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

async fn send(output: &mut (impl AsyncWriteExt + Unpin), value: Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    output
        .write_all(&bytes)
        .await
        .map_err(|e| format!("Codex helper disconnected: {e}"))
}

async fn reply(
    input: &mut (impl AsyncBufReadExt + Unpin),
    output: &mut (impl AsyncWriteExt + Unpin),
    id: u64,
) -> Result<Value, String> {
    const MAX_FRAME: usize = 8 * 1024 * 1024;
    for _ in 0..512 {
        let mut bytes = Vec::new();
        let count = (&mut *input)
            .take((MAX_FRAME + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|e| format!("Couldn't read Codex branching response: {e}"))?;
        if count == 0 {
            return Err("Codex branching helper exited before responding".into());
        }
        if count > MAX_FRAME {
            return Err("Codex branching response exceeded the size limit".into());
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid Codex branching response")?;
        if value.get("method").is_some() {
            // No turns or approval decisions are allowed in this one-shot helper.
            if let Some(request) = value.get("id") {
                send(output, json!({"id":request,"error":{"code":-32601,"message":"Interactive requests are not supported while branching"}})).await?;
            }
            continue;
        }
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(format!(
                "Codex couldn't branch the session: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unsupported fork request; update Codex and try again")
            ));
        }
        return value
            .get("result")
            .cloned()
            .ok_or("Codex returned no branching result".into());
    }
    Err("Codex returned too many messages while branching".into())
}

fn persisted_branch(response: &Value, parent: &str) -> Result<String, String> {
    let id = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .ok_or("Codex did not return a branch id")?;
    if id == parent || uuid::Uuid::parse_str(id).is_err() {
        return Err("Codex did not create a distinct branch".into());
    }
    let path = response
        .pointer("/thread/path")
        .and_then(Value::as_str)
        .ok_or("Codex did not persist the branch")?;
    if !Path::new(path).is_file() {
        return Err("Codex branch transcript is not available on disk".into());
    }
    Ok(id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Opt-in integration check with the installed CLI, using only synthetic
    /// history in a temporary CODEX_HOME. No model turn or credentials needed.
    #[tokio::test]
    #[ignore = "requires an installed Codex CLI with app-server fork support"]
    async fn native_codex_fork_persists_context_and_lineage_without_touching_parent() {
        let root =
            std::env::temp_dir().join(format!("aiterm-codex-native-{}", uuid::Uuid::new_v4()));
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let parent = uuid::Uuid::new_v4().to_string();
        let source = sessions.join(format!("rollout-2026-09-12T01-00-00-{parent}.jsonl"));
        let text = [
            json!({"timestamp":"2026-09-12T01:00:00Z","type":"session_meta","payload":{"id":parent,"timestamp":"2026-09-12T01:00:00Z","cwd":root,"originator":"codex_cli_rs","cli_version":"0.154.0","source":"cli","model_provider":"openai"}}),
            json!({"timestamp":"2026-09-12T01:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic branch context"}]}}),
        ].iter().map(|line| format!("{line}\n")).collect::<String>();
        std::fs::write(&source, &text).unwrap();
        let binary = super::super::which("codex")
            .or_else(|| super::super::which_via_login_shell("codex"))
            .expect("Codex installed");
        let command = || {
            let mut c = tokio::process::Command::new(&binary);
            c.arg("app-server")
                .current_dir(&root)
                .env("CODEX_HOME", &root)
                .env_remove("OPENAI_API_KEY");
            c
        };
        let branch = fork_with(command(), &parent, Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&source).unwrap(), text);
        let rows = super::super::scan_codex_dir(&sessions);
        let (row, path) = rows
            .iter()
            .find(|(row, _)| row.id == branch)
            .expect("fork appears in AiTerm's session list");
        assert!(row.forked);
        assert_eq!(row.fork_parent.as_deref(), Some(parent.as_str()));
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .contains("Synthetic branch context"));
        // A fresh Codex process must be able to load the persisted branch again.
        let next = fork_with(command(), &branch, Duration::from_secs(30))
            .await
            .unwrap();
        assert_ne!(next, branch);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), text);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn responses_skip_notifications_and_reject_interactive_requests() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (read, mut write) = tokio::io::split(client);
        let task = tokio::spawn(async move {
            server.write_all(b"{\"method\":\"notice\"}\n{\"id\":99,\"method\":\"approval\"}\n{\"id\":2,\"result\":{\"ok\":true}}\n").await.unwrap();
            let mut response = String::new();
            BufReader::new(server)
                .read_line(&mut response)
                .await
                .unwrap();
            let value: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(value["error"]["code"], -32601);
        });
        assert_eq!(
            reply(&mut BufReader::new(read), &mut write, 2)
                .await
                .unwrap(),
            json!({"ok":true})
        );
        task.await.unwrap();
    }
    #[tokio::test]
    async fn rpc_errors_and_eof_are_reported() {
        for bytes in [
            b"{\"id\":2,\"error\":{\"message\":\"fork unsupported\"}}\n".as_slice(),
            b"",
        ] {
            assert!(reply(&mut BufReader::new(bytes), &mut tokio::io::sink(), 2)
                .await
                .is_err());
        }
    }
    #[test]
    fn invalid_or_ephemeral_branches_are_never_reported_as_success() {
        let id = uuid::Uuid::new_v4().to_string();
        for value in [
            json!({}),
            json!({"thread":{"id":id}}),
            json!({"thread":{"id":"invalid","path":"/tmp"}}),
        ] {
            assert!(persisted_branch(&value, &id).is_err());
        }
    }
    #[tokio::test]
    #[cfg(unix)]
    async fn stalled_helper_is_killed_and_reaped() {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "exec sleep 60"]);
        let started = std::time::Instant::now();
        let error = fork_with(
            command,
            &uuid::Uuid::new_v4().to_string(),
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
