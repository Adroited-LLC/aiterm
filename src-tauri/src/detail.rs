//! Everything worth remembering about a session, in one read of its
//! transcript — for the flyout that opens when a sidebar row is hovered.
//!
//! The row says a title, a path and an age. That is enough to find a session
//! you had in mind, and nothing like enough to recognise one you have
//! forgotten: what was asked, what came of it, which model, how long it ran,
//! which files it touched, how much of the context window it had used when it
//! stopped. Those are the things that make "oh, *that* one" happen, so this
//! collects them.
//!
//! One pass over the file, no JSON kept: transcripts run to tens of megabytes
//! and the hover has to answer while the pointer is still there. Claude's
//! shape is read in full; Codex's for what it records (session meta, turn
//! context, token counts, function calls); every other engine through its
//! provider's `messages()`, which gives the conversation and nothing else —
//! counts and the first and last exchange are still worth having.

use crate::sessions::{is_system_meta_prompt, line_may_hold_message, line_message, strip_system_tags};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct ToolCount {
    pub name: String,
    pub count: u32,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct SessionDetail {
    pub id: String,
    /// ISO timestamps off the transcript: the first line's and the last's.
    pub started: Option<String>,
    pub last_active: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub cli_version: Option<String>,
    /// In order of first use. More than one means the model was switched.
    pub models: Vec<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub user_messages: u32,
    pub assistant_messages: u32,
    pub tool_calls: u32,
    /// The most-used tools, most first, at most six.
    pub tools: Vec<ToolCount>,
    /// Input side of the last assistant turn — prompt, cache reads and cache
    /// writes together — which is what the context window held at the end.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub output_tokens: u64,
    /// The engine's own title for the conversation, where it wrote one.
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub last_user: Option<String>,
    pub last_assistant: Option<String>,
    /// Files written or edited, most recent first, at most eight.
    pub files: Vec<String>,
    pub pr_links: Vec<String>,
    pub compactions: u32,
}

const FIRST_MAX: usize = 240;
const LAST_MAX: usize = 320;
const TOOLS_KEPT: usize = 6;
const FILES_KEPT: usize = 8;

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max).collect();
    s.push('…');
    s
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !s.is_empty() && !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

/// Most recent first, no repeats, capped — a file edited ten times is one
/// line, and it is the one at the top.
fn touch_file(files: &mut Vec<String>, path: &str) {
    if path.is_empty() {
        return;
    }
    files.retain(|f| f != path);
    files.insert(0, path.to_string());
    files.truncate(FILES_KEPT);
}

#[tauri::command]
pub async fn session_detail(session_id: String) -> Option<SessionDetail> {
    crate::run_blocking(move || session_detail_sync(session_id)).await
}

fn session_detail_sync(session_id: String) -> Option<SessionDetail> {
    let list = crate::agents::backends();
    let (backend, path) = crate::agents::owner_in(&list, &session_id)?;
    if let Some(msgs) = backend.sessions().messages(&session_id) {
        return Some(from_messages(session_id, msgs));
    }
    let file = File::open(&path).ok()?;
    let mut d = SessionDetail { id: session_id, ..Default::default() };
    let mut tools: HashMap<String, u32> = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // The cheap gate is for messages only; the bookkeeping lines
        // (permission-mode, ai-title, pr-link, token_count) are short and
        // parse for nothing.
        if !line_may_hold_message(&line) && !is_bookkeeping(&line) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        read_line(&mut d, &mut tools, &v);
    }
    d.tools = top_tools(tools);
    Some(d)
}

/// Lines the message gate skips that still say something worth reading.
fn is_bookkeeping(line: &str) -> bool {
    [
        "\"permission-mode\"", "\"ai-title\"", "\"pr-link\"", "\"compact_boundary\"",
        "\"session_meta\"", "\"turn_context\"", "\"token_count\"", "\"task_started\"",
        "\"function_call\"",
    ]
    .iter()
    .any(|k| line.contains(k))
}

fn top_tools(tools: HashMap<String, u32>) -> Vec<ToolCount> {
    let mut v: Vec<ToolCount> =
        tools.into_iter().map(|(name, count)| ToolCount { name, count }).collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    v.truncate(TOOLS_KEPT);
    v
}

fn str_at<'a>(v: &'a serde_json::Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(|x| x.as_str())
}

fn u64_at(v: &serde_json::Value, ptr: &str) -> Option<u64> {
    v.pointer(ptr).and_then(|x| x.as_u64())
}

/// One transcript line into the running detail. Claude's and Codex's shapes
/// both land here; a line that is neither is a no-op.
pub(crate) fn read_line(d: &mut SessionDetail, tools: &mut HashMap<String, u32>, v: &serde_json::Value) {
    if let Some(ts) = str_at(v, "/timestamp") {
        if d.started.is_none() {
            d.started = Some(ts.to_string());
        }
        d.last_active = Some(ts.to_string());
    }
    if let Some(c) = str_at(v, "/cwd") {
        d.cwd = Some(c.to_string());
    }
    if let Some(b) = str_at(v, "/gitBranch") {
        if !b.is_empty() {
            d.branch = Some(b.to_string());
        }
    }
    if let Some(ver) = str_at(v, "/version") {
        d.cli_version = Some(ver.to_string());
    }

    let kind = str_at(v, "/type").unwrap_or("");
    match kind {
        // ---- claude bookkeeping ----
        "permission-mode" => {
            if let Some(m) = str_at(v, "/permissionMode") {
                d.permission_mode = Some(m.to_string());
            }
        }
        "ai-title" => {
            if let Some(t) = str_at(v, "/aiTitle") {
                d.title = Some(t.to_string());
            }
        }
        "pr-link" => {
            if let Some(u) = str_at(v, "/prUrl") {
                push_unique(&mut d.pr_links, u);
            }
        }
        "system" => {
            if str_at(v, "/subtype").is_some_and(|s| s.contains("compact")) {
                d.compactions += 1;
            }
        }
        // ---- codex bookkeeping ----
        "session_meta" => {
            if let Some(c) = str_at(v, "/payload/cwd") {
                d.cwd = Some(c.to_string());
            }
            if let Some(ver) = str_at(v, "/payload/cli_version") {
                d.cli_version = Some(ver.to_string());
            }
        }
        "turn_context" => {
            if let Some(m) = str_at(v, "/payload/model") {
                push_unique(&mut d.models, m);
            }
            if let Some(p) = str_at(v, "/payload/approval_policy") {
                d.permission_mode = Some(p.to_string());
            }
            if let Some(e) = str_at(v, "/payload/effort") {
                d.effort = Some(e.to_string());
            }
        }
        "event_msg" => match str_at(v, "/payload/type") {
            Some("task_started") => {
                if let Some(w) = u64_at(v, "/payload/model_context_window") {
                    d.context_window = Some(w);
                }
            }
            Some("token_count") => {
                if let Some(o) = u64_at(v, "/payload/info/total_token_usage/output_tokens") {
                    d.output_tokens = o;
                }
                let last = "/payload/info/last_token_usage";
                if let Some(i) = u64_at(v, &format!("{last}/input_tokens")) {
                    d.context_tokens = Some(i);
                }
                if let Some(w) = u64_at(v, "/payload/info/model_context_window") {
                    d.context_window = Some(w);
                }
            }
            _ => {}
        },
        "response_item" => {
            if str_at(v, "/payload/type") == Some("function_call") {
                d.tool_calls += 1;
                let name = str_at(v, "/payload/name").unwrap_or("tool");
                *tools.entry(name.to_string()).or_insert(0) += 1;
            }
        }
        _ => {}
    }

    // ---- claude turns ----
    if kind == "assistant" && v.get("isSidechain").and_then(|b| b.as_bool()) != Some(true) {
        if let Some(m) = str_at(v, "/message/model") {
            push_unique(&mut d.models, m);
        }
        if let Some(e) = str_at(v, "/effort") {
            d.effort = Some(e.to_string());
        }
        if let Some(u) = v.pointer("/message/usage") {
            let input = u64_at(u, "/input_tokens").unwrap_or(0)
                + u64_at(u, "/cache_read_input_tokens").unwrap_or(0)
                + u64_at(u, "/cache_creation_input_tokens").unwrap_or(0);
            if input > 0 {
                d.context_tokens = Some(input);
            }
            d.output_tokens += u64_at(u, "/output_tokens").unwrap_or(0);
        }
        if let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array()) {
            for b in blocks {
                if str_at(b, "/type") != Some("tool_use") {
                    continue;
                }
                d.tool_calls += 1;
                let name = str_at(b, "/name").unwrap_or("tool");
                *tools.entry(name.to_string()).or_insert(0) += 1;
                if matches!(name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") {
                    if let Some(p) = str_at(b, "/input/file_path").or(str_at(b, "/input/notebook_path")) {
                        touch_file(&mut d.files, p);
                    }
                }
            }
        }
    }

    // ---- the conversation, either shape ----
    if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
        return;
    }
    if let Some((role, text)) = line_message(v) {
        let text = strip_system_tags(&text);
        if text.trim().is_empty() {
            return;
        }
        note_message(d, &role, &text);
    }
}

fn note_message(d: &mut SessionDetail, role: &str, text: &str) {
    match role {
        "user" => {
            if is_system_meta_prompt(text) {
                return;
            }
            d.user_messages += 1;
            if d.first_prompt.is_none() {
                d.first_prompt = Some(clip(text, FIRST_MAX));
            }
            d.last_user = Some(clip(text, LAST_MAX));
        }
        "assistant" => {
            d.assistant_messages += 1;
            d.last_assistant = Some(clip(text, LAST_MAX));
        }
        _ => {}
    }
}

/// A conversation handed over whole by a provider that keeps no transcript
/// file: what can be said from `(role, text)` alone.
fn from_messages(id: String, msgs: Vec<(String, String)>) -> SessionDetail {
    let mut d = SessionDetail { id, ..Default::default() };
    for (role, text) in msgs {
        let text = strip_system_tags(&text);
        if text.trim().is_empty() {
            continue;
        }
        note_message(&mut d, &role, &text);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed(lines: &[serde_json::Value]) -> SessionDetail {
        let mut d = SessionDetail::default();
        let mut tools = HashMap::new();
        for v in lines {
            read_line(&mut d, &mut tools, v);
        }
        d.tools = top_tools(tools);
        d
    }

    #[test]
    fn claude_transcript_yields_the_lot() {
        let d = feed(&[
            json!({"type":"permission-mode","permissionMode":"auto"}),
            json!({"type":"user","timestamp":"2026-08-28T10:00:00Z","cwd":"/w","gitBranch":"5lime","version":"2.1.0",
                   "message":{"role":"user","content":"make the icons bigger"}}),
            json!({"type":"assistant","timestamp":"2026-08-28T10:00:05Z","effort":"high",
                   "message":{"model":"claude-fable-5","usage":{"input_tokens":10,"cache_read_input_tokens":90,"cache_creation_input_tokens":0,"output_tokens":40},
                   "content":[{"type":"text","text":"On it."},{"type":"tool_use","name":"Edit","input":{"file_path":"/w/a.css"}},{"type":"tool_use","name":"Bash","input":{}}]}}),
            json!({"type":"assistant","timestamp":"2026-08-28T10:01:00Z","isSidechain":true,
                   "message":{"model":"claude-haiku","usage":{"input_tokens":999,"output_tokens":1},"content":[{"type":"text","text":"sub"}]}}),
            json!({"type":"assistant","timestamp":"2026-08-28T10:02:00Z",
                   "message":{"model":"claude-fable-5","usage":{"input_tokens":20,"cache_read_input_tokens":180,"cache_creation_input_tokens":5,"output_tokens":60},
                   "content":[{"type":"text","text":"Done — a.css and b.ts."},{"type":"tool_use","name":"Edit","input":{"file_path":"/w/b.ts"}},{"type":"tool_use","name":"Edit","input":{"file_path":"/w/a.css"}}]}}),
            json!({"type":"ai-title","aiTitle":"Bigger icons"}),
            json!({"type":"pr-link","prUrl":"https://github.com/x/y/pull/13"}),
            json!({"type":"system","subtype":"compact_boundary","timestamp":"2026-08-28T10:03:00Z"}),
        ]);
        assert_eq!(d.started.as_deref(), Some("2026-08-28T10:00:00Z"));
        assert_eq!(d.last_active.as_deref(), Some("2026-08-28T10:03:00Z"));
        assert_eq!(d.cwd.as_deref(), Some("/w"));
        assert_eq!(d.branch.as_deref(), Some("5lime"));
        assert_eq!(d.cli_version.as_deref(), Some("2.1.0"));
        assert_eq!(d.models, vec!["claude-fable-5"], "sidechain model is not the session's");
        assert_eq!(d.effort.as_deref(), Some("high"));
        assert_eq!(d.permission_mode.as_deref(), Some("auto"));
        assert_eq!((d.user_messages, d.assistant_messages), (1, 2));
        assert_eq!(d.tool_calls, 4);
        assert_eq!(d.tools[0], ToolCount { name: "Edit".into(), count: 3 });
        assert_eq!(d.context_tokens, Some(205), "last main-thread turn's input side");
        assert_eq!(d.output_tokens, 100, "sidechain output not counted");
        assert_eq!(d.title.as_deref(), Some("Bigger icons"));
        assert_eq!(d.first_prompt.as_deref(), Some("make the icons bigger"));
        assert_eq!(d.last_assistant.as_deref(), Some("Done — a.css and b.ts."));
        assert_eq!(d.files, vec!["/w/a.css", "/w/b.ts"], "most recent first, no repeats");
        assert_eq!(d.pr_links, vec!["https://github.com/x/y/pull/13"]);
        assert_eq!(d.compactions, 1);
    }

    #[test]
    fn codex_transcript_yields_what_it_records() {
        let d = feed(&[
            json!({"timestamp":"2026-08-28T23:18:39Z","type":"session_meta","payload":{"cwd":"/c","cli_version":"0.150.1"}}),
            json!({"timestamp":"2026-08-28T23:18:39Z","type":"event_msg","payload":{"type":"task_started","model_context_window":258400}}),
            json!({"timestamp":"2026-08-28T23:18:39Z","type":"turn_context","payload":{"model":"gpt-5-codex","approval_policy":"on-request"}}),
            json!({"timestamp":"2026-08-28T23:18:40Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}),
            json!({"timestamp":"2026-08-28T23:18:41Z","type":"response_item","payload":{"type":"function_call","name":"shell"}}),
            json!({"timestamp":"2026-08-28T23:18:42Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hey John!"}]}}),
            json!({"timestamp":"2026-08-28T23:18:42Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"output_tokens":14},"last_token_usage":{"input_tokens":15612}}}}),
        ]);
        assert_eq!(d.cwd.as_deref(), Some("/c"));
        assert_eq!(d.cli_version.as_deref(), Some("0.150.1"));
        assert_eq!(d.models, vec!["gpt-5-codex"]);
        assert_eq!(d.permission_mode.as_deref(), Some("on-request"));
        assert_eq!(d.context_window, Some(258400));
        assert_eq!(d.context_tokens, Some(15612));
        assert_eq!(d.output_tokens, 14);
        assert_eq!(d.tool_calls, 1);
        assert_eq!(d.tools, vec![ToolCount { name: "shell".into(), count: 1 }]);
        assert_eq!(d.first_prompt.as_deref(), Some("hi"));
        assert_eq!(d.last_assistant.as_deref(), Some("Hey John!"));
    }

    #[test]
    fn clip_marks_the_cut() {
        assert_eq!(clip("  short  ", 10), "short");
        assert_eq!(clip("abcdefghij", 5), "abcde…");
    }
}
