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

// ── The transcript ──────────────────────────────────────────────────────
//
// One jsonl per chat in ~/.local/share/aiterm/chats/<id>.jsonl. The first
// line is aiterm's own meta record; every message after it is written in the
// same `{"type":"user","message":{…}}` shape Claude Code uses, so the
// sessions panel's preview and the search indexer read a chat without
// learning anything new. Our own record types are prefixed `aiterm-chat` and
// skipped by anything that doesn't know them.

pub fn chats_dir() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".local/share/aiterm/chats"))
}

/// The transcript path for a chat id, if the file exists.
pub fn chat_file_if_exists(id: &str) -> Option<std::path::PathBuf> {
    if id.contains('/') || id.contains("..") {
        return None;
    }
    let p = chats_dir()?.join(format!("{id}.jsonl"));
    p.is_file().then_some(p)
}

/// What a transcript loads back to: where it points, and the conversation as
/// the model should see it now — messages since the last `/clear`, under the
/// most recent `/model` choice.
pub struct Loaded {
    pub provider_id: String,
    pub model: String,
    pub messages: Vec<Msg>,
    /// Turns dropped by `/clear` markers — still on disk, no longer context.
    pub cleared: usize,
}

pub fn parse_transcript(lines: impl Iterator<Item = String>) -> Option<Loaded> {
    let mut provider_id = None;
    let mut model = None;
    let mut messages: Vec<Msg> = Vec::new();
    let mut cleared = 0usize;
    for line in lines {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("aiterm-chat") => {
                provider_id = v.get("provider").and_then(|p| p.as_str()).map(String::from);
                model = model.or_else(|| v.get("model").and_then(|m| m.as_str()).map(String::from));
            }
            Some("aiterm-chat-model") => {
                model = v.get("model").and_then(|m| m.as_str()).map(String::from).or(model);
            }
            Some("aiterm-chat-clear") => {
                cleared += messages.len();
                messages.clear();
            }
            Some(role @ ("user" | "assistant")) => {
                if let Some(text) = v.pointer("/message/content").and_then(|c| c.as_str()) {
                    messages.push(Msg {
                        role: if role == "user" { "user" } else { "assistant" },
                        content: text.to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    Some(Loaded {
        provider_id: provider_id?,
        model: model?,
        messages,
        cleared,
    })
}

/// A message line in the claude-compatible shape the preview already reads.
pub fn msg_line(role: &str, text: &str, at: u64) -> String {
    serde_json::json!({
        "type": role,
        "message": { "role": role, "content": text },
        "at": at,
    })
    .to_string()
}

/// If `line` asks to continue (one trailing backslash), the text to keep —
/// otherwise `None` and the message is done. A doubled backslash is an
/// escaped literal one, not a continuation.
pub fn continuation(line: &str) -> Option<&str> {
    if line.ends_with('\\') && !line.ends_with("\\\\") {
        Some(&line[..line.len() - 1])
    } else {
        None
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Sidebar rows for every stored chat — stamped `api`, the socket icon.
/// Header-read only, like the codex scanner: one line per file, never the
/// conversation itself.
pub fn scan_chats() -> Vec<(crate::sessions::Session, std::path::PathBuf)> {
    let Some(dir) = chats_dir() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "jsonl") {
                return None;
            }
            let file = std::fs::File::open(&path).ok()?;
            let mut first = String::new();
            std::io::BufReader::new(file).read_line(&mut first).ok()?;
            let v: serde_json::Value = serde_json::from_str(&first).ok()?;
            if v.get("type").and_then(|t| t.as_str()) != Some("aiterm-chat") {
                return None;
            }
            let id = v.get("id")?.as_str()?.to_string();
            let model = v.get("model")?.as_str()?.to_string();
            let cwd = v.get("cwd").and_then(|c| c.as_str()).unwrap_or("").to_string();
            let last_active = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            Some((
                crate::sessions::Session {
                    id,
                    agent: "api".into(),
                    // The model IS the identity of a chat — a row named after
                    // its directory would make ten chats indistinguishable.
                    title: model.rsplit('/').next().unwrap_or(&model).to_string(),
                    project_path: cwd.clone(),
                    group_path: cwd,
                    branch: None,
                    forked: false,
                    background: false,
                    fork_parent: None,
                    last_active,
                },
                path,
            ))
        })
        .collect()
}

/// The request body for one exchange: full history, streaming on, routing when
/// the model has any, and usage accounting so a reply can report its cost.
pub fn chat_body(model: &str, messages: &[Msg], routing: Option<&serde_json::Value>) -> String {
    let mut v = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if let (Some(r), Some(map)) = (routing, v.as_object_mut()) {
        map.insert("provider".into(), r.clone());
    }
    v.to_string()
}

/// What one `data:` line carries. Any field may be absent; absent is absent.
#[derive(Debug, Default, PartialEq)]
pub struct Frame {
    pub text: Option<String>,
    /// The host that served this reply — a top-level `provider` on every
    /// chunk. Undeclared in OpenRouter's OpenAPI document, present on the
    /// wire, so it is read where offered and never required.
    pub provider: Option<String>,
    /// USD for the exchange, on the final chunk when `include_usage` is set.
    pub cost: Option<f64>,
}

/// One line of a streaming reply → what it carries.
///
/// The stream is server-sent events: `data: {json}` lines with token deltas,
/// `data: [DONE]` at the end, and bare `: comment` keep-alives (OpenRouter
/// sends `: OPENROUTER PROCESSING` while a model spins up) that carry
/// nothing. A line that is plain JSON with an `error` object is the provider
/// failing the request — returned as `Err` so the loop can say so.
pub fn frame(line: &str) -> Result<Frame, String> {
    let line = line.trim();
    if let Some(data) = line.strip_prefix("data:") {
        let data = data.trim();
        if data == "[DONE]" {
            return Ok(Frame::default());
        }
        let v: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(Frame::default()), // half a frame; nothing to print
        };
        if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
            return Err(msg.to_string());
        }
        return Ok(Frame {
            text: v
                .pointer("/choices/0/delta/content")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(String::from),
            provider: v
                .get("provider")
                .and_then(|p| p.as_str())
                .filter(|p| !p.is_empty())
                .map(String::from),
            cost: v.pointer("/usage/cost").and_then(|c| c.as_f64()),
        });
    }
    // A non-SSE JSON line is how a request that failed outright comes back.
    if line.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
                return Err(msg.to_string());
            }
        }
    }
    Ok(Frame::default())
}

/// What a reply cost and who served it, as one line — or `None` when the
/// stream said neither, which is every provider that is not OpenRouter.
fn attribution(host: Option<&str>, cost: Option<f64>) -> Option<String> {
    let mut bits = Vec::new();
    if let Some(h) = host {
        bits.push(format!("via {h}"));
    }
    if let Some(c) = cost {
        bits.push(format!("${c:.4}"));
    }
    (!bits.is_empty()).then(|| bits.join(" · "))
}

/// The note under a failed request for a pinned model. A pin sends
/// `allow_fallbacks: false`, so OpenRouter having nothing to route to is the
/// pin working, not the session breaking — but only if we say which pin.
/// `None` for an unpinned model, whose failure is an ordinary failure.
fn pinned_note(model: &str, order: &[String]) -> Option<String> {
    if order.is_empty() {
        return None;
    }
    Some(format!(
        "{model} is pinned to {} with no fallback — OpenRouter had nothing to route to.",
        order.join(", ")
    ))
}

/// How a chat comes into being: fresh under a minted id, or resumed from a
/// transcript that already knows its provider and model.
pub enum Start {
    Fresh { provider_id: String, model: String, session_id: Option<String> },
    Resume { session_id: String },
}

/// Entry point for the `chat` argv mode. Never returns to Tauri.
pub fn run(start: Start) -> i32 {
    let (provider_id, mut model, session_id, mut messages, cleared) = match start {
        Start::Fresh { provider_id, model, session_id } => {
            (provider_id, model, session_id, Vec::new(), 0)
        }
        Start::Resume { session_id } => {
            let Some(path) = chat_file_if_exists(&session_id) else {
                eprintln!("aiterm chat: no stored chat '{session_id}'");
                return 1;
            };
            let Ok(file) = std::fs::File::open(&path) else {
                eprintln!("aiterm chat: could not read {}", path.display());
                return 1;
            };
            let lines = std::io::BufReader::new(file).lines().map_while(Result::ok);
            let Some(loaded) = parse_transcript(lines) else {
                eprintln!("aiterm chat: {} is not a chat transcript", path.display());
                return 1;
            };
            (
                loaded.provider_id,
                loaded.model,
                Some(session_id),
                loaded.messages,
                loaded.cleared,
            )
        }
    };

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

    // The transcript, where there is an id to keep one under. Opened for
    // append: a resume continues the same file it loaded.
    let mut log = session_id.as_deref().and_then(|id| open_transcript(id, &p.id, &model));

    // Dim banner, plain prompt. The terminal's own line editing does the rest.
    println!("\x1b[2m── {model} · via {} · aiterm chat\x1b[0m", p.name);
    println!(
        "\x1b[2m   Enter sends · /model swaps · /clear starts over · /quit or Ctrl+D leaves\x1b[0m"
    );
    if !messages.is_empty() || cleared > 0 {
        replay_tail(&messages, cleared);
    }

    let stdin = std::io::stdin();
    loop {
        print!("\n\x1b[36m❯\x1b[0m ");
        let _ = std::io::stdout().flush();
        // A line ending in a single backslash continues onto the next one —
        // the same gesture claude's composer uses, and what aiterm's terminal
        // types on Shift+Enter. The message sends when a line ends plainly.
        let mut buf = String::new();
        let mut ended = false;
        loop {
            let mut line = String::new();
            match stdin.lock().read_line(&mut line) {
                Ok(0) | Err(_) => {
                    ended = true; // Ctrl+D or a closed pty
                    break;
                }
                Ok(_) => {}
            }
            let piece = line.trim_end_matches(['\r', '\n']);
            match continuation(piece) {
                Some(kept) => {
                    buf.push_str(kept);
                    buf.push('\n');
                    print!("\x1b[2m…\x1b[0m ");
                    let _ = std::io::stdout().flush();
                }
                None => {
                    buf.push_str(piece);
                    break;
                }
            }
        }
        if ended && buf.trim().is_empty() {
            break;
        }
        let text = buf.trim();
        if text.is_empty() {
            continue;
        }
        match text {
            "/quit" | "/exit" => break,
            "/clear" => {
                messages.clear();
                append(&mut log, &serde_json::json!({"type":"aiterm-chat-clear"}).to_string());
                println!("\x1b[2mFresh conversation — the model remembers nothing above.\x1b[0m");
                continue;
            }
            "/model" => {
                println!("\x1b[2m{model} — `/model <id>` swaps, history kept\x1b[0m");
                continue;
            }
            _ => {}
        }
        if let Some(next) = text.strip_prefix("/model ") {
            model = next.trim().to_string();
            append(
                &mut log,
                &serde_json::json!({"type":"aiterm-chat-model","model":model}).to_string(),
            );
            println!("\x1b[2mNow talking to {model} — it sees the whole conversation.\x1b[0m");
            continue;
        }
        messages.push(Msg { role: "user", content: text.to_string() });
        // Rebuilt per send: `/model` swaps the model mid-chat, and the route
        // has to swap with it.
        let routing = crate::providers::routing_block(p, &model);
        match stream_reply(&url, &p.api_key, &model, &messages, routing.as_ref()) {
            Ok(reply) if reply.text.is_empty() => {
                eprintln!("\x1b[2m(the model returned nothing)\x1b[0m");
                messages.pop();
            }
            Ok(reply) => {
                println!();
                if let Some(line) = attribution(reply.host.as_deref(), reply.cost) {
                    println!("\x1b[2m· {line}\x1b[0m");
                }
                append(&mut log, &msg_line("user", text, now_ms()));
                append(&mut log, &msg_line("assistant", &reply.text, now_ms()));
                messages.push(Msg { role: "assistant", content: reply.text });
            }
            Err(e) => {
                eprintln!("\x1b[31m{e}\x1b[0m");
                // A pin is "only that host": say so, or a refusal from a
                // constraint the user set reads as a broken session.
                let order = p.routes.get(&model).map(|r| r.order.as_slice()).unwrap_or(&[]);
                if let Some(note) = pinned_note(&model, order) {
                    eprintln!("\x1b[2m  {note}\x1b[0m");
                }
                messages.pop(); // the turn didn't happen; let it be retyped
            }
        }
    }
    0
}

/// Open (creating if new) the transcript for `id`, positioned to append.
/// A fresh file gets its meta line; failure to persist degrades to an
/// ephemeral chat with a warning, never to a dead console.
fn open_transcript(id: &str, provider_id: &str, model: &str) -> Option<std::fs::File> {
    let dir = chats_dir()?;
    if std::fs::create_dir_all(&dir).is_err() {
        eprintln!("\x1b[2m(transcript off — could not create {})\x1b[0m", dir.display());
        return None;
    }
    let path = dir.join(format!("{id}.jsonl"));
    let fresh = !path.is_file();
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path);
    match file {
        Ok(mut f) => {
            if fresh {
                let cwd = std::env::current_dir()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let meta = serde_json::json!({
                    "type": "aiterm-chat",
                    "id": id,
                    "provider": provider_id,
                    "model": model,
                    "cwd": cwd,
                    "at": now_ms(),
                })
                .to_string();
                let _ = writeln!(f, "{meta}");
            }
            Some(f)
        }
        Err(e) => {
            eprintln!("\x1b[2m(transcript off — {e})\x1b[0m");
            None
        }
    }
}

fn append(log: &mut Option<std::fs::File>, line: &str) {
    if let Some(f) = log {
        if writeln!(f, "{line}").is_err() {
            eprintln!("\x1b[2m(transcript stopped — disk said no)\x1b[0m");
            *log = None;
        }
    }
}

/// On resume, show where the conversation stands: the last exchange verbatim,
/// the rest as a count. The model sees everything; the human needs bearings.
fn replay_tail(messages: &[Msg], cleared: usize) {
    let shown = messages.len().min(2);
    let earlier = messages.len() - shown;
    if earlier > 0 || cleared > 0 {
        let mut parts = Vec::new();
        if earlier > 0 {
            parts.push(format!("{earlier} earlier messages in context"));
        }
        if cleared > 0 {
            parts.push(format!("{cleared} cleared"));
        }
        println!("\x1b[2m   … {}\x1b[0m", parts.join(" · "));
    }
    for m in &messages[messages.len() - shown..] {
        match m.role {
            "user" => println!("\n\x1b[36m❯\x1b[0m {}", m.content),
            _ => println!("{}", m.content),
        }
    }
}

/// One finished exchange: the reply, and what the stream said about serving
/// it. `host` and `cost` are absent unless the provider volunteers them.
struct Reply {
    text: String,
    host: Option<String>,
    cost: Option<f64>,
}

/// Send the conversation, print the reply as it streams, return it whole —
/// with whatever the stream said about who served it and what it cost.
fn stream_reply(
    url: &str,
    key: &str,
    model: &str,
    messages: &[Msg],
    routing: Option<&serde_json::Value>,
) -> Result<Reply, String> {
    // The body goes through a 0600 file rather than the argv: prompts are the
    // user's own text and `/proc/<pid>/cmdline` is world-readable.
    let body_path = std::env::temp_dir().join(format!("aiterm-chat-{}.json", std::process::id()));
    crate::providers::write_private(&body_path, &chat_body(model, messages, routing))
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
    // The host names itself on every chunk and the cost arrives on the last
    // one: keep the first host offered, and the newest cost seen.
    let mut host: Option<String> = None;
    let mut cost: Option<f64> = None;
    let mut failed: Option<String> = None;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            match frame(&line) {
                Ok(f) => {
                    if let Some(chunk) = f.text {
                        print!("{chunk}");
                        let _ = std::io::stdout().flush();
                        reply.push_str(&chunk);
                    }
                    if host.is_none() {
                        host = f.provider;
                    }
                    cost = f.cost.or(cost);
                }
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
        Ok(s) if s.success() => Ok(Reply { text: reply, host, cost }),
        _ => {
            if reply.is_empty() {
                Err("curl could not reach the provider.".into())
            } else {
                // The stream broke mid-reply; what printed is what there is.
                Ok(Reply { text: reply, host, cost })
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
        assert_eq!(frame(line).unwrap().text.as_deref(), Some("Hel"));
    }

    #[test]
    fn done_markers_keepalives_and_role_frames_yield_nothing() {
        assert_eq!(frame("data: [DONE]").unwrap(), Frame::default());
        assert_eq!(frame(": OPENROUTER PROCESSING").unwrap(), Frame::default());
        assert_eq!(
            frame(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#).unwrap(),
            Frame::default()
        );
        assert_eq!(frame("").unwrap(), Frame::default());
    }

    /// Providers fail two ways: an error object inside the stream, or a plain
    /// JSON error body when the request never became a stream at all. Both
    /// must surface as errors, not silence.
    #[test]
    fn provider_errors_surface_from_both_shapes() {
        let inline = r#"data: {"error":{"message":"Rate limited"}}"#;
        assert_eq!(frame(inline).unwrap_err(), "Rate limited");
        let flat = r#"{"error":{"message":"Invalid model"}}"#;
        assert_eq!(frame(flat).unwrap_err(), "Invalid model");
    }

    #[test]
    fn a_chunk_names_the_host_that_served_it() {
        // Verified live 2026-08-10: `provider` rides every chunk, and is absent
        // from OpenRouter's own OpenAPI schema. Read it where present, never
        // require it.
        let line = r#"data: {"id":"x","provider":"Novita","choices":[{"delta":{"content":"hi"}}]}"#;
        let f = frame(line).unwrap();
        assert_eq!(f.text.as_deref(), Some("hi"));
        assert_eq!(f.provider.as_deref(), Some("Novita"));
    }

    #[test]
    fn a_chunk_without_a_provider_is_not_an_error() {
        let line = r#"data: {"id":"x","choices":[{"delta":{"content":"hi"}}]}"#;
        let f = frame(line).unwrap();
        assert_eq!(f.text.as_deref(), Some("hi"));
        assert_eq!(f.provider, None);
    }

    #[test]
    fn the_final_usage_chunk_carries_the_cost() {
        let line = r#"data: {"id":"x","provider":"Novita","choices":[],
                      "usage":{"prompt_tokens":14,"completion_tokens":5,"cost":0.0021}}"#;
        let f = frame(line).unwrap();
        assert_eq!(f.text, None);
        assert_eq!(f.cost, Some(0.0021));
    }

    #[test]
    fn the_body_carries_the_routing_block_when_there_is_one() {
        let routing = serde_json::json!({"ignore": ["baidu"], "order": ["novita"],
                                         "allow_fallbacks": false});
        let body = chat_body("z-ai/glm-5.2", &[], Some(&routing));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["provider"]["order"], serde_json::json!(["novita"]));
        assert_eq!(v["stream"], serde_json::json!(true));
        // Cost per reply, which is why include_usage goes on unconditionally.
        assert_eq!(v["stream_options"]["include_usage"], serde_json::json!(true));
    }

    #[test]
    fn an_unrouted_model_sends_no_provider_key_at_all() {
        let body = chat_body("z-ai/glm-5.2", &[], None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v.get("provider").is_none());
    }

    /// The line under a reply says only what the stream actually reported —
    /// and with nothing reported there is no line to print.
    #[test]
    fn the_attribution_names_only_the_parts_that_arrived() {
        let both = attribution(Some("Novita"), Some(0.0021));
        assert_eq!(both.as_deref(), Some("via Novita · $0.0021"));
        assert_eq!(attribution(Some("Novita"), None).as_deref(), Some("via Novita"));
        assert_eq!(attribution(None, Some(0.0021)).as_deref(), Some("$0.0021"));
        assert_eq!(attribution(None, None), None);
    }

    /// A pinned model has one host and no fallback, so a refusal is the pin
    /// doing its job. Say which pin, or "only that host" reads as a break.
    #[test]
    fn a_refused_pin_names_the_hosts_it_was_held_to() {
        let order = vec!["novita".to_string(), "deepinfra".to_string()];
        let note = pinned_note("z-ai/glm-5.2", &order).unwrap();
        assert!(note.contains("z-ai/glm-5.2"), "{note}");
        assert!(note.contains("novita, deepinfra"), "{note}");
        assert_eq!(pinned_note("z-ai/glm-5.2", &[]), None);
    }

    /// One trailing backslash continues the message; a doubled one is a
    /// literal backslash and sends. Anything else sends as typed.
    #[test]
    fn a_single_trailing_backslash_continues_the_line() {
        assert_eq!(continuation("first part \\"), Some("first part "));
        assert_eq!(continuation("plain line"), None);
        assert_eq!(continuation("ends in literal \\\\"), None);
        assert_eq!(continuation("\\"), Some(""));
    }

    fn lines(v: &[&str]) -> impl Iterator<Item = String> {
        v.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter()
    }

    /// A transcript loads back to exactly the context the model should see:
    /// its own written lines round-trip, `/clear` markers drop what came
    /// before (counting it), and the latest `/model` swap wins.
    #[test]
    fn a_transcript_reloads_to_the_context_the_model_should_see() {
        let meta = r#"{"type":"aiterm-chat","id":"x","provider":"openrouter","model":"a/b","cwd":"/tmp","at":1}"#;
        let loaded = parse_transcript(lines(&[
            meta,
            &msg_line("user", "first", 2),
            &msg_line("assistant", "reply one", 3),
            r#"{"type":"aiterm-chat-clear"}"#,
            &msg_line("user", "second", 4),
            &msg_line("assistant", "reply two", 5),
            r#"{"type":"aiterm-chat-model","model":"c/d"}"#,
        ]))
        .unwrap();
        assert_eq!(loaded.provider_id, "openrouter");
        assert_eq!(loaded.model, "c/d", "the swap wins over the meta model");
        assert_eq!(loaded.cleared, 2);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content, "second");
        assert_eq!(loaded.messages[1].role, "assistant");
    }

    /// A file without our meta line is not a chat — `None`, not a panic and
    /// not an empty conversation pretending to be one.
    #[test]
    fn a_foreign_jsonl_is_not_a_chat() {
        assert!(parse_transcript(lines(&[r#"{"type":"user","message":{"role":"user","content":"hi"}}"#])).is_none());
    }

    #[test]
    fn the_body_carries_model_history_and_streaming() {
        let msgs = vec![
            Msg { role: "user", content: "hi".into() },
            Msg { role: "assistant", content: "hello".into() },
        ];
        let v: serde_json::Value = serde_json::from_str(&chat_body("a/b", &msgs, None)).unwrap();
        assert_eq!(v["model"], "a/b");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][1]["role"], "assistant");
        assert_eq!(v["messages"][1]["content"], "hello");
    }
}
