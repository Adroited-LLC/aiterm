//! `aiterm chat` — the console harness behind an API-model session.
//!
//! Runs as its own process inside the tab's PTY, exactly the way `claude`
//! does: aiterm the GUI spawns the user's shell with this command typed into
//! it, and everything conversational happens here, in a terminal, over
//! stdin/stdout. No GUI thread, no Tauri, no async runtime.
//!
//! The provider's key is read from `~/.config/aiterm/providers.json` by this
//! process itself — it is never on the argv, which `/proc` publishes to every
//! process on the machine. The request goes out through `curl`, matching the
//! rest of the project (no TLS stack of our own), with the bearer token passed
//! on curl's stdin via `--config -` for the same reason.

use std::io::{BufRead, Write};

/// One turn of the conversation, as the wire wants it.
#[derive(serde::Serialize, Clone)]
pub struct Msg {
    pub role: &'static str,
    pub content: String,
}

/// The request body for one exchange: full history, streaming on.
pub fn chat_body(model: &str, messages: &[Msg]) -> String {
    serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
    })
    .to_string()
}

/// One line of a streaming reply → the text it carries, if any.
///
/// The stream is server-sent events: `data: {json}` lines with token deltas,
/// `data: [DONE]` at the end, and bare `: comment` keep-alives (OpenRouter
/// sends `: OPENROUTER PROCESSING` while a model spins up) that carry
/// nothing. A line that is plain JSON with an `error` object is the provider
/// failing the request — returned as `Err` so the loop can say so.
pub fn sse_delta(line: &str) -> Result<Option<String>, String> {
    let line = line.trim();
    if let Some(data) = line.strip_prefix("data:") {
        let data = data.trim();
        if data == "[DONE]" {
            return Ok(None);
        }
        let v: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(None), // half a frame; nothing to print
        };
        if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
            return Err(msg.to_string());
        }
        return Ok(v
            .pointer("/choices/0/delta/content")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
            .map(String::from));
    }
    // A non-SSE JSON line is how a request that failed outright comes back.
    if line.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
                return Err(msg.to_string());
            }
        }
    }
    Ok(None)
}

/// Entry point for the `chat` argv mode. Never returns to Tauri.
pub fn run(provider_id: &str, model: &str) -> i32 {
    let providers = crate::providers::load_providers();
    let Some(p) = providers.iter().find(|p| p.id == provider_id) else {
        eprintln!("aiterm chat: no provider '{provider_id}' in ~/.config/aiterm/providers.json");
        return 1;
    };
    if p.api_key.is_empty() {
        eprintln!("aiterm chat: provider '{}' has no API key saved", p.name);
        return 1;
    }
    let url = format!("{}/chat/completions", p.base_url);

    // Dim banner, plain prompt. The terminal's own line editing does the rest.
    println!("\x1b[2m── {model} · via {} · aiterm chat\x1b[0m", p.name);
    println!("\x1b[2m   Enter sends · /clear starts over · /quit or Ctrl+D leaves\x1b[0m");

    let stdin = std::io::stdin();
    let mut messages: Vec<Msg> = Vec::new();
    loop {
        print!("\n\x1b[36m❯\x1b[0m ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => break, // Ctrl+D or a closed pty
            Ok(_) => {}
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        match text {
            "/quit" | "/exit" => break,
            "/clear" => {
                messages.clear();
                println!("\x1b[2mFresh conversation — the model remembers nothing above.\x1b[0m");
                continue;
            }
            _ => {}
        }
        messages.push(Msg { role: "user", content: text.to_string() });
        match stream_reply(&url, &p.api_key, model, &messages) {
            Ok(reply) if reply.is_empty() => {
                eprintln!("\x1b[2m(the model returned nothing)\x1b[0m");
                messages.pop();
            }
            Ok(reply) => {
                println!();
                messages.push(Msg { role: "assistant", content: reply });
            }
            Err(e) => {
                eprintln!("\x1b[31m{e}\x1b[0m");
                messages.pop(); // the turn didn't happen; let it be retyped
            }
        }
    }
    0
}

/// Send the conversation, print the reply as it streams, return it whole.
fn stream_reply(url: &str, key: &str, model: &str, messages: &[Msg]) -> Result<String, String> {
    // The body goes through a 0600 file rather than the argv: prompts are the
    // user's own text and `/proc/<pid>/cmdline` is world-readable.
    let body_path = std::env::temp_dir().join(format!("aiterm-chat-{}.json", std::process::id()));
    crate::providers::write_private(&body_path, &chat_body(model, messages))
        .map_err(|e| format!("could not stage the request: {e}"))?;

    let mut child = std::process::Command::new("curl")
        .args([
            "-sS",
            "-N", // stream: no buffering, print frames as they arrive
            "--connect-timeout",
            "10",
            "--max-time",
            "600",
            "-H",
            "Content-Type: application/json",
            "--config",
            "-",
            "--data-binary",
        ])
        .arg(format!("@{}", body_path.display()))
        .arg(url)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;

    child
        .stdin
        .take()
        .ok_or("curl took no stdin")?
        .write_all(crate::providers::curl_auth_config(key).as_bytes())
        .map_err(|e| format!("could not pass the key to curl: {e}"))?;

    let mut reply = String::new();
    let mut failed: Option<String> = None;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            match sse_delta(&line) {
                Ok(Some(chunk)) => {
                    print!("{chunk}");
                    let _ = std::io::stdout().flush();
                    reply.push_str(&chunk);
                }
                Ok(None) => {}
                Err(e) => failed = Some(e),
            }
        }
    }
    let status = child.wait();
    let _ = std::fs::remove_file(&body_path);
    if let Some(e) = failed {
        return Err(format!("The provider refused: {e}"));
    }
    match status {
        Ok(s) if s.success() => Ok(reply),
        _ => {
            if reply.is_empty() {
                Err("curl could not reach the provider.".into())
            } else {
                // The stream broke mid-reply; what printed is what there is.
                Ok(reply)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_delta_yields_its_text() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#;
        assert_eq!(sse_delta(line).unwrap(), Some("Hel".into()));
    }

    #[test]
    fn done_markers_keepalives_and_role_frames_yield_nothing() {
        assert_eq!(sse_delta("data: [DONE]").unwrap(), None);
        assert_eq!(sse_delta(": OPENROUTER PROCESSING").unwrap(), None);
        assert_eq!(
            sse_delta(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#).unwrap(),
            None
        );
        assert_eq!(sse_delta("").unwrap(), None);
    }

    /// Providers fail two ways: an error object inside the stream, or a plain
    /// JSON error body when the request never became a stream at all. Both
    /// must surface as errors, not silence.
    #[test]
    fn provider_errors_surface_from_both_shapes() {
        let inline = r#"data: {"error":{"message":"Rate limited"}}"#;
        assert_eq!(sse_delta(inline).unwrap_err(), "Rate limited");
        let flat = r#"{"error":{"message":"Invalid model"}}"#;
        assert_eq!(sse_delta(flat).unwrap_err(), "Invalid model");
    }

    #[test]
    fn the_body_carries_model_history_and_streaming() {
        let msgs = vec![
            Msg { role: "user", content: "hi".into() },
            Msg { role: "assistant", content: "hello".into() },
        ];
        let v: serde_json::Value = serde_json::from_str(&chat_body("a/b", &msgs)).unwrap();
        assert_eq!(v["model"], "a/b");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][1]["role"], "assistant");
        assert_eq!(v["messages"][1]["content"], "hello");
    }
}
