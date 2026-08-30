//! xAI's `grok` CLI as a backend.
//!
//! Grok keeps one directory per session under
//! `~/.grok/sessions/<url-encoded cwd>/<uuid>/`, and inside it a `summary.json`
//! that says everything a sidebar row needs — id, cwd, title, model, when —
//! from the moment the session starts. The conversation itself is
//! `chat_history.jsonl` beside it. Both shapes were read off grok 1.0.5 on
//! 2026-08-27; nothing here is guessed.
//!
//! Two things make it the easiest engine after claude to sit in a tab. It
//! takes `--session-id` for a *new* conversation ("must be a valid UUID and
//! must not already exist"), so aiterm mints the id and the row is real from
//! the first frame — verified: `grok --session-id <v4> -p …` wrote
//! `<cwd>/<that id>/summary.json`. And `--resume <id>` reopens one by id from
//! any directory, so ▶ is a one-flag command.
//!
//! What it does not get: 🗑. A session is a directory, and `session_delete`
//! renames a *file* into the trash — moving `chat_history.jsonl` alone would
//! leave `summary.json` behind for the next scan to list again. A directory
//! trash is a separate piece of work, so the button is withheld rather than
//! wired to something that half-works.

use crate::agents::{q, detect_cli, AgentBackend, Caps, Detection, LaunchSpec, ModelOption, PermissionMode};
use crate::sessions::{Session, SessionProvider};
use std::path::{Path, PathBuf};

pub struct GrokBackend;
pub struct GrokSessions;

/// Grok's permission presets, from `grok --help` 1.0.5: a `--permission-mode`
/// with the same value set as claude's, plus `--always-approve` for the
/// no-questions case.
///
/// The first — Grok's own default — passes nothing, so `[ui] permission_mode`
/// in the user's `~/.grok/config.toml` still decides, which is where grok's
/// mode lived before aiterm offered to override it. Picking anything else here
/// is the override.
pub const GROK_PERMISSION_MODES: &[PermissionMode] = &[
    PermissionMode {
        id: "default",
        label: "Grok's own default",
        note: "Whatever ~/.grok/config.toml's [ui] permission_mode says.",
        flags: &[],
    },
    PermissionMode {
        id: "acceptEdits",
        label: "Accept edits",
        note: "--permission-mode acceptEdits: file edits run without asking; commands still ask.",
        flags: &["--permission-mode acceptEdits"],
    },
    PermissionMode {
        id: "auto",
        label: "Auto",
        note: "--permission-mode auto: routine actions run, the rest ask.",
        flags: &["--permission-mode auto"],
    },
    PermissionMode {
        id: "plan",
        label: "Plan mode",
        note: "--permission-mode plan: reads and plans, changes nothing until told to.",
        flags: &["--permission-mode plan"],
    },
    PermissionMode {
        id: "bypassPermissions",
        label: "Skip all permissions",
        note: "--always-approve: every tool call runs without asking.",
        flags: &["--always-approve"],
    },
];

fn grok_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".grok"))
}

fn sessions_root() -> Option<PathBuf> {
    let dir = grok_home()?.join("sessions");
    dir.is_dir().then_some(dir)
}

/// What `summary.json` says about its session.
#[derive(Debug, PartialEq)]
pub struct Summary {
    pub id: String,
    pub cwd: String,
    /// `session_summary` — grok's own title, written once it has one. Empty
    /// until then.
    pub title: String,
    pub model: Option<String>,
}

pub fn parse_summary(text: &str) -> Option<Summary> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let info = v.get("info")?;
    Some(Summary {
        id: info.get("id")?.as_str()?.to_string(),
        cwd: info.get("cwd")?.as_str()?.to_string(),
        title: v
            .get("session_summary")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        model: v
            .get("current_model_id")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
            .map(String::from),
    })
}

fn mtime_ms(p: &Path) -> u64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The file a row stands for: the transcript when there is one, else the
/// summary — a session that has not been spoken to yet still exists.
fn session_file(dir: &Path) -> PathBuf {
    let chat = dir.join("chat_history.jsonl");
    if chat.is_file() { chat } else { dir.join("summary.json") }
}

/// One session directory → its row. `None` for anything that is not one.
fn read_row(dir: &Path) -> Option<(Session, PathBuf)> {
    let text = std::fs::read_to_string(dir.join("summary.json")).ok()?;
    let s = parse_summary(&text)?;
    let file = session_file(dir);
    // Whichever moved last: the transcript grows with the conversation, the
    // summary is rewritten as grok re-titles it.
    let last_active = mtime_ms(&file).max(mtime_ms(&dir.join("summary.json")));
    let title = if s.title.is_empty() {
        // The directory, matching how codex and untitled claude rows read.
        Path::new(&s.cwd)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| s.cwd.clone())
    } else {
        s.title
    };
    Some((
        Session {
            id: s.id,
            // Stamped by the registry; see `scan_backends`.
            agent: String::new(),
            title,
            project_path: s.cwd.clone(),
            group_path: s.cwd,
            branch: None,
            forked: false,
            background: false,
            fork_parent: None,
            last_active,
        },
        file,
    ))
}

/// The body of [`GrokSessions::scan_with_paths`], over an explicit root so it
/// can be tested against a directory built for the purpose.
pub fn scan_dir(root: &Path) -> Vec<(Session, PathBuf)> {
    let mut budget = crate::sessions::DiscoveryBudget::new();
    scan_dir_bounded(root, &mut budget)
}

pub(crate) fn scan_dir_bounded(
    root: &Path,
    budget: &mut crate::sessions::DiscoveryBudget,
) -> Vec<(Session, PathBuf)> {
    let Ok(cwds) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut out = Vec::new();
    for cwd in cwds.flatten() {
        if budget.remaining() == 0 {
            break;
        }
        // `session_search.sqlite` sits beside the cwd directories.
        let Ok(cwd_type) = cwd.file_type() else { continue };
        if cwd_type.is_symlink() || !cwd_type.is_dir() {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(cwd.path()) else {
            continue;
        };
        for e in sessions.flatten() {
            if budget.remaining() == 0 {
                break;
            }
            let Ok(file_type) = e.file_type() else { continue };
            if !file_type.is_symlink() && file_type.is_dir() && budget.claim_file() {
                if let Some(row) = read_row(&e.path()) {
                    out.push(row);
                }
            }
        }
    }
    out
}

/// The directory for `session_id`, if this is a grok session.
fn session_dir(session_id: &str) -> Option<PathBuf> {
    // Cheap enough to search: a handful of cwd directories, one readdir each.
    // The id is not the path — the cwd component is url-encoded, and
    // reproducing grok's encoding to skip the search would be a second
    // implementation of it to keep right.
    let root = sessions_root()?;
    let cwds = std::fs::read_dir(root).ok()?;
    cwds.flatten()
        .map(|c| c.path().join(session_id))
        .find(|d| d.join("summary.json").is_file())
}

/// The conversation from `chat_history.jsonl`, as `(role, text)`.
///
/// Only what a person would call the conversation: user turns and assistant
/// prose. Reasoning, tool calls and tool results are the engine talking to
/// itself and would swamp a preview.
pub fn parse_messages(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let role = v.get("type")?.as_str()?;
            if role != "user" && role != "assistant" {
                return None;
            }
            let body = match v.get("content")? {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(parts) => parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => return None,
            };
            let body = if role == "user" { user_query(&body) } else { body };
            let body = body.trim();
            (!body.is_empty()).then(|| (role.to_string(), body.to_string()))
        })
        .collect()
}

/// What the person typed, out of a user turn.
///
/// Grok wraps the first prompt in the environment it injects — `<user_info>`,
/// `<git_status>`, the agents files — and puts the words themselves inside
/// `<user_query>`. A turn with no such tag is the words already.
fn user_query(body: &str) -> String {
    match (body.find("<user_query>"), body.rfind("</user_query>")) {
        (Some(a), Some(b)) if a + "<user_query>".len() <= b => {
            body[a + "<user_query>".len()..b].to_string()
        }
        _ => body.to_string(),
    }
}

/// The session's task list, from `todo_write` calls in the transcript.
/// `merge:false` replaces the whole list — the common case — and
/// `merge:true` upserts by id, so the list is replayed rather than
/// last-write-wins.
pub fn parse_tasks(text: &str) -> Vec<crate::sessions::SessionTask> {
    let mut tasks: Vec<crate::sessions::SessionTask> = Vec::new();
    for line in text.lines() {
        if !line.contains("todo_write") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(calls) = v.get("tool_calls").and_then(|t| t.as_array()) else { continue };
        for call in calls {
            if call.get("name").and_then(|n| n.as_str()) != Some("todo_write") {
                continue;
            }
            // Arguments arrive as a JSON string, not an object.
            let Some(args) = call
                .get("arguments")
                .and_then(|a| a.as_str())
                .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
            else {
                continue;
            };
            let Some(todos) = args.get("todos").and_then(|t| t.as_array()) else { continue };
            if !args.get("merge").and_then(|m| m.as_bool()).unwrap_or(false) {
                tasks.clear();
            }
            for t in todos {
                let (Some(id), Some(content)) = (
                    t.get("id").and_then(|x| x.as_str()),
                    t.get("content").and_then(|x| x.as_str()),
                ) else {
                    continue;
                };
                let status = t
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("pending")
                    .to_string();
                if let Some(existing) = tasks.iter_mut().find(|x| x.id == id) {
                    existing.subject = content.to_string();
                    existing.status = status;
                } else {
                    tasks.push(crate::sessions::SessionTask {
                        id: id.to_string(),
                        subject: content.to_string(),
                        status,
                        active_form: None,
                        blocked_by: vec![],
                    });
                }
            }
        }
    }
    tasks
}

/// Files the session wrote, from `write` and `search_replace` calls, newest
/// touch first. Grok records carry no timestamps, so `at` stays empty and
/// the order is the transcript's own; the panel shows no time for these
/// rather than a made-up one.
pub fn parse_artifacts(text: &str) -> Vec<crate::sessions::Artifact> {
    let mut order: Vec<String> = Vec::new();
    let mut tool_of: std::collections::HashMap<String, &'static str> = Default::default();
    for line in text.lines() {
        if !line.contains("file_path") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(calls) = v.get("tool_calls").and_then(|t| t.as_array()) else { continue };
        for call in calls {
            let tool = match call.get("name").and_then(|n| n.as_str()) {
                Some("write") | Some("create_file") => "Write",
                Some("search_replace") | Some("edit_file") | Some("apply_patch") => "Edit",
                _ => continue,
            };
            let Some(fp) = call
                .get("arguments")
                .and_then(|a| a.as_str())
                .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
                .and_then(|args| args.get("file_path").and_then(|f| f.as_str()).map(String::from))
            else {
                continue;
            };
            if let Some(pos) = order.iter().position(|p| p == &fp) {
                order.remove(pos);
            }
            tool_of.insert(fp.clone(), tool);
            order.push(fp);
        }
    }
    order
        .into_iter()
        .rev()
        .map(|path| crate::sessions::Artifact {
            tool: tool_of[&path].to_string(),
            path,
            at: String::new(),
        })
        .collect()
}

/// What the flyout shows for a grok session, from the two files that hold
/// it. `summary.json` says the model, the effort, the sandbox, the branch
/// and when — from the first frame, before a word is exchanged — and
/// `chat_history.jsonl` says the rest: every assistant line carries its
/// `model_id` and `reasoning_effort`, and its `tool_calls` name the tool
/// and, for `write`/`search_replace`, the file. Both shapes as read off
/// grok 1.0.5. Grok records no token counts and no per-line timestamps, so
/// context stays unknown rather than invented.
pub fn parse_detail(id: &str, summary: &str, chat: &str) -> crate::detail::SessionDetail {
    use crate::detail::{note_message, push_unique, top_tools, touch_file, SessionDetail};
    let mut d = SessionDetail { id: id.to_string(), ..Default::default() };
    let mut summary_model = None;
    if let Ok(s) = serde_json::from_str::<serde_json::Value>(summary) {
        let str_of = |k: &str| s.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty()).map(String::from);
        d.started = str_of("created_at");
        d.last_active = str_of("updated_at");
        d.cwd = s.pointer("/info/cwd").and_then(|v| v.as_str()).map(String::from);
        d.branch = str_of("head_branch");
        d.title = str_of("session_summary");
        d.effort = str_of("reasoning_effort");
        d.permission_mode = str_of("sandbox_profile").map(|p| format!("sandbox {p}"));
        summary_model = str_of("current_model_id");
    }
    let mut tools: std::collections::HashMap<String, u32> = Default::default();
    for line in chat.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(kind) = v.get("type").and_then(|t| t.as_str()) else { continue };
        if kind != "user" && kind != "assistant" {
            continue;
        }
        if kind == "assistant" {
            if let Some(m) = v.get("model_id").and_then(|m| m.as_str()) {
                push_unique(&mut d.models, m);
            }
            if let Some(e) = v.get("reasoning_effort").and_then(|e| e.as_str()) {
                d.effort = Some(e.to_string());
            }
            for call in v.get("tool_calls").and_then(|t| t.as_array()).into_iter().flatten() {
                d.tool_calls += 1;
                let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                *tools.entry(name.to_string()).or_insert(0) += 1;
                if matches!(name, "write" | "create_file" | "search_replace" | "edit_file" | "apply_patch") {
                    if let Some(fp) = call
                        .get("arguments")
                        .and_then(|a| a.as_str())
                        .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
                        .and_then(|a| a.get("file_path").and_then(|f| f.as_str()).map(String::from))
                    {
                        touch_file(&mut d.files, &fp);
                    }
                }
            }
        } else if v.get("synthetic_reason").is_some() {
            // The engine talking to itself in the user's seat.
            continue;
        }
        let body = match v.get("content") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => continue,
        };
        // The first user line is often a preamble alone — `<user_info>`,
        // `<git_status>`, `<rules>` — with no query in it; stripped, it is
        // nothing, and nothing is what it should count as.
        let body = if kind == "user" {
            crate::sessions::strip_system_tags(&user_query(&body))
        } else {
            body
        };
        if !body.trim().is_empty() {
            note_message(&mut d, kind, &body);
        }
    }
    if d.models.is_empty() {
        if let Some(m) = summary_model {
            d.models.push(m);
        }
    }
    d.tools = top_tools(tools);
    d
}

impl SessionProvider for GrokSessions {
    fn scan_with_paths(&self) -> Vec<(Session, PathBuf)> {
        sessions_root().map(|r| scan_dir(&r)).unwrap_or_default()
    }

    fn scan_with_paths_bounded(
        &self,
        budget: &mut crate::sessions::DiscoveryBudget,
    ) -> Vec<(Session, PathBuf)> {
        sessions_root()
            .map(|root| scan_dir_bounded(&root, budget))
            .unwrap_or_default()
    }

    fn find_session_file(&self, session_id: &str) -> Option<PathBuf> {
        session_dir(session_id).map(|d| session_file(&d))
    }

    /// Answered here rather than left to the transcript path: the file is
    /// jsonl, but not claude's jsonl, and the default reader would make
    /// nothing of it.
    fn messages(&self, session_id: &str) -> Option<Vec<(String, String)>> {
        let dir = session_dir(session_id)?;
        let text = std::fs::read_to_string(dir.join("chat_history.jsonl")).ok()?;
        Some(parse_messages(&text))
    }

    fn detail(&self, session_id: &str) -> Option<crate::detail::SessionDetail> {
        let dir = session_dir(session_id)?;
        let summary = std::fs::read_to_string(dir.join("summary.json")).ok()?;
        let chat = std::fs::read_to_string(dir.join("chat_history.jsonl")).unwrap_or_default();
        Some(parse_detail(session_id, &summary, &chat))
    }

    fn tasks(&self, session_id: &str) -> Option<Vec<crate::sessions::SessionTask>> {
        let dir = session_dir(session_id)?;
        let text = std::fs::read_to_string(dir.join("chat_history.jsonl")).ok()?;
        Some(parse_tasks(&text))
    }

    fn artifacts(&self, session_id: &str) -> Option<Vec<crate::sessions::Artifact>> {
        let dir = session_dir(session_id)?;
        let text = std::fs::read_to_string(dir.join("chat_history.jsonl")).ok()?;
        Some(parse_artifacts(&text))
    }
}

/// Parse `~/.grok/models_cache.json`, which the CLI writes from its own
/// `/models` call: slug, display name, the reasoning efforts each model takes
/// and which is default. Split out so the shape can be tested against a
/// captured copy without grok installed.
pub fn parse_models(text: &str) -> Option<Vec<ModelOption>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let models = v.get("models")?.as_object()?;
    let out: Vec<ModelOption> = models
        .iter()
        .filter_map(|(slug, entry)| {
            let info = entry.get("info")?;
            if info.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false) {
                return None;
            }
            let display_name = info
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or(slug)
                .to_string();
            let supports = info
                .get("supports_reasoning_effort")
                .and_then(|s| s.as_bool())
                .unwrap_or(false);
            let levels: Vec<&serde_json::Value> = if supports {
                info.get("reasoning_efforts")
                    .and_then(|l| l.as_array())
                    .map(|a| a.iter().collect())
                    .unwrap_or_default()
            } else {
                vec![]
            };
            let efforts: Vec<String> = levels
                .iter()
                .filter_map(|l| l.get("value").or(l.get("id")).and_then(|x| x.as_str()))
                .map(String::from)
                .collect();
            let default_effort = levels
                .iter()
                .find(|l| l.get("default").and_then(|d| d.as_bool()).unwrap_or(false))
                .and_then(|l| l.get("value").and_then(|x| x.as_str()))
                .or_else(|| info.get("reasoning_effort").and_then(|x| x.as_str()))
                .filter(|_| !efforts.is_empty())
                .map(String::from);
            Some(ModelOption { id: slug.clone(), display_name, efforts, default_effort })
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

fn cached_models() -> Option<Vec<ModelOption>> {
    let text = std::fs::read_to_string(grok_home()?.join("models_cache.json")).ok()?;
    parse_models(&text)
}

impl AgentBackend for GrokBackend {
    fn id(&self) -> &'static str {
        "grok"
    }
    fn display_name(&self) -> &'static str {
        "Grok"
    }
    fn detect(&self) -> Detection {
        Detection { caps: self.caps(), ..detect_cli(self.id(), self.display_name(), "grok") }
    }
    fn sessions(&self) -> &dyn SessionProvider {
        &GrokSessions
    }

    /// `--session-id` names a new conversation, and the directory appears
    /// under that name at launch — see the module doc.
    fn mints_session_id(&self) -> bool {
        true
    }

    /// Resume and re-key. Fork is real in grok (`--resume <id> --fork-session
    /// --session-id <new>`) but ⑂'s flow was written against claude's job
    /// state and is not claimed until it has been walked for this engine. No
    /// TUI driving, no transcript panels: both read claude's shapes. No
    /// delete — see the module doc.
    fn caps(&self) -> Caps {
        Caps { resume: true, clear: true, tasks: true, ..Default::default() }
    }

    fn permission_modes(&self) -> &'static [PermissionMode] {
        GROK_PERMISSION_MODES
    }

    /// `grok --resume <id>`: "UUID-shaped values always mean IDs".
    fn resume(&self, session_id: &str) -> Option<String> {
        Some(format!("grok --resume {}", q(session_id)))
    }

    fn clear(&self, session_id: &str) -> Option<String> {
        Some(self.launch(&LaunchSpec {
            session_id: Some(session_id.to_string()),
            ..Default::default()
        }))
    }

    fn models(&self) -> Vec<ModelOption> {
        cached_models().unwrap_or_default()
    }

    /// `-m/--model`, `--reasoning-effort` and `--session-id`, all straight
    /// from `grok --help` 1.0.5. Nothing about permissions: grok's mode is in
    /// its own config (`[ui] permission_mode`), which is the user's to set.
    fn launch(&self, spec: &LaunchSpec) -> String {
        let mut cmd = String::from("grok");
        if let Some(m) = spec.model.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --model {}", q(m)));
        }
        if let Some(e) = spec.effort.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --reasoning-effort {}", q(e)));
        }
        if let Some(id) = spec.session_id.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --session-id {}", q(id)));
        }
        if let Some(p) = crate::agents::prompt_of(spec) {
            cmd.push_str(&format!(" {}", q(p)));
        }
        cmd
    }
}

#[cfg(test)]
mod detail_tests {
    use super::*;

    #[test]
    fn the_flyout_gets_the_lot() {
        let summary = r#"{"info":{"id":"89fc","cwd":"/home/admin/AI-OS"},"session_summary":"Closing House research",
            "created_at":"2026-08-29T01:32:24.650674083Z","updated_at":"2026-08-29T01:40:00.000000000Z",
            "num_chat_messages":2,"current_model_id":"grok-4.6","git_root_dir":"/home/admin/AI-OS/",
            "head_commit":"6137e27","head_branch":"master","agent_name":"grok-build-plan","sandbox_profile":"off","reasoning_effort":"high"}"#;
        let chat = [
            r#"{"type":"system","content":"You are Grok."}"#,
            r#"{"type":"user","content":[{"type":"text","text":"<user_info>\nOS Version: linux\n</user_info>\n\n<rules>\n<user_rule>be brief</user_rule>\n</rules>"}]}"#,
            r#"{"type":"user","content":"<user_query>look at closing.house</user_query>","prompt_index":0}"#,
            r#"{"type":"reasoning","encrypted_content":"x","id":"r1","status":"completed","summary":[],"type":"reasoning"}"#,
            r#"{"type":"assistant","content":"I'll start with the site.","tool_calls":[{"id":"c1","name":"web_fetch","arguments":"{\"url\":\"https://closing.house\"}"}],"model_id":"grok-4.6-build","model_fingerprint":"fp","reasoning_effort":"high"}"#,
            r#"{"type":"tool_result","content":"<html>","tool_call_id":"c1"}"#,
            r#"{"type":"user","content":"(subagent finished)","synthetic_reason":"subagent"}"#,
            r#"{"type":"assistant","content":"","tool_calls":[{"id":"c2","name":"write","arguments":"{\"file_path\":\"/home/admin/AI-OS/a.md\",\"content\":\"x\"}"},{"id":"c3","name":"search_replace","arguments":"{\"file_path\":\"/home/admin/AI-OS/b.md\"}"},{"id":"c4","name":"write","arguments":"{\"file_path\":\"/home/admin/AI-OS/a.md\"}"}],"model_id":"grok-4.6-build","reasoning_effort":"high"}"#,
            r#"{"type":"assistant","content":"Here is what I took from closing.house.","model_id":"grok-4.6-build","reasoning_effort":"high"}"#,
        ].join("\n");
        let d = parse_detail("89fc", summary, &chat);
        assert_eq!(d.started.as_deref(), Some("2026-08-29T01:32:24.650674083Z"));
        assert_eq!(d.last_active.as_deref(), Some("2026-08-29T01:40:00.000000000Z"));
        assert_eq!(d.cwd.as_deref(), Some("/home/admin/AI-OS"));
        assert_eq!(d.branch.as_deref(), Some("master"));
        assert_eq!(d.title.as_deref(), Some("Closing House research"));
        assert_eq!(d.models, vec!["grok-4.6-build"], "the transcript's model, not the summary's");
        assert_eq!(d.effort.as_deref(), Some("high"));
        assert_eq!(d.permission_mode.as_deref(), Some("sandbox off"));
        assert_eq!((d.user_messages, d.assistant_messages), (1, 2), "the preamble, synthetic user turns and empty assistant turns are not the conversation");
        assert_eq!(d.tool_calls, 4);
        assert_eq!(d.tools[0].name, "write");
        assert_eq!(d.tools[0].count, 2);
        assert_eq!(d.files, vec!["/home/admin/AI-OS/a.md", "/home/admin/AI-OS/b.md"], "most recent touch first, once each");
        assert_eq!(d.first_prompt.as_deref(), Some("look at closing.house"));
        assert_eq!(d.last_assistant.as_deref(), Some("Here is what I took from closing.house."));
        assert_eq!(d.context_tokens, None, "grok records no usage; nothing is invented");
    }

    #[test]
    fn an_empty_session_still_says_model_and_when() {
        let summary = r#"{"info":{"id":"x","cwd":"/w"},"session_summary":"","created_at":"2026-08-29T01:32:24Z","updated_at":"2026-08-29T01:32:24Z","current_model_id":"grok-4.6","sandbox_profile":"off","reasoning_effort":"medium"}"#;
        let d = parse_detail("x", summary, "");
        assert_eq!(d.models, vec!["grok-4.6"]);
        assert_eq!(d.effort.as_deref(), Some("medium"));
        assert_eq!(d.title, None);
        assert_eq!(d.user_messages, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARY: &str = r#"{
      "info": {"id": "01a03132-548d-7422-b3b1-f966d5acd37a", "cwd": "/home/x/AI-OS"},
      "session_summary": "Affiliate campaign flow deep-dive",
      "created_at": "2026-08-24T00:36:17.174135835Z",
      "updated_at": "2026-08-24T00:53:23.314210708Z",
      "current_model_id": "grok-4.6"
    }"#;

    #[test]
    fn a_summary_becomes_a_row() {
        let s = parse_summary(SUMMARY).unwrap();
        assert_eq!(s.id, "01a03132-548d-7422-b3b1-f966d5acd37a");
        assert_eq!(s.cwd, "/home/x/AI-OS");
        assert_eq!(s.title, "Affiliate campaign flow deep-dive");
        assert_eq!(s.model.as_deref(), Some("grok-4.6"));
    }

    #[test]
    fn a_session_directory_lists_under_its_cwd_and_titles_itself() {
        let root = std::env::temp_dir().join(format!("aiterm-grok-{}", uuid::Uuid::new_v4()));
        let dir = root.join("%2Fhome%2Fx%2FAI-OS").join("01a03132-548d-7422-b3b1-f966d5acd37a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("summary.json"), SUMMARY).unwrap();
        std::fs::write(dir.join("chat_history.jsonl"), "").unwrap();
        // The search index sits beside the cwd directories and is not one.
        std::fs::write(root.join("session_search.sqlite"), "").unwrap();
        // An untitled session reads as its directory.
        let bare = root.join("%2Ftmp%2Fprobe").join("e63b0f22-7d69-4084-aaf3-733816255e8e");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(
            bare.join("summary.json"),
            r#"{"info":{"id":"e63b0f22-7d69-4084-aaf3-733816255e8e","cwd":"/tmp/probe"},"session_summary":""}"#,
        )
        .unwrap();

        let mut rows = scan_dir(&root);
        rows.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0.title, "Affiliate campaign flow deep-dive");
        assert_eq!(rows[0].0.project_path, "/home/x/AI-OS");
        assert!(rows[0].1.ends_with("chat_history.jsonl"));
        assert_eq!(rows[1].0.title, "probe");
        assert!(rows[1].1.ends_with("summary.json"), "no transcript yet: the summary stands in");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_preview_is_the_conversation_and_not_the_machinery() {
        let log = concat!(
            r#"{"type":"system","content":"You are Grok."}"#, "\n",
            r#"{"type":"user","content":[{"type":"text","text":"<user_info>\nOS: linux\n</user_info>\n\n<user_query>\nfix the bug\n</user_query>"}]}"#, "\n",
            r#"{"type":"reasoning","summary":[{"type":"summary_text","text":"thinking"}]}"#, "\n",
            r#"{"type":"assistant","content":"","tool_calls":[{"name":"read_file"}]}"#, "\n",
            r#"{"type":"tool_result","content":"file contents"}"#, "\n",
            r#"{"type":"assistant","content":"Fixed it."}"#, "\n",
            r#"{"type":"user","content":"thanks"}"#, "\n",
        );
        let msgs = parse_messages(log);
        assert_eq!(
            msgs,
            vec![
                ("user".to_string(), "fix the bug".to_string()),
                ("assistant".to_string(), "Fixed it.".to_string()),
                ("user".to_string(), "thanks".to_string()),
            ]
        );
    }

    #[test]
    fn models_come_from_the_cache_with_their_efforts() {
        let cache = r#"{"models": {
          "grok-4.6": {"info": {"name": "Grok 4.6", "hidden": false,
            "supports_reasoning_effort": true, "reasoning_effort": "high",
            "reasoning_efforts": [
              {"id": "xhigh", "value": "xhigh", "default": false},
              {"id": "high", "value": "high", "default": true},
              {"id": "low", "value": "low", "default": false}]}},
          "grok-4.5": {"info": {"name": "Grok 4.5", "supports_reasoning_effort": false}},
          "grok-secret": {"info": {"name": "Hidden", "hidden": true}}
        }}"#;
        let m = parse_models(cache).unwrap();
        assert_eq!(m.len(), 2, "hidden models are not offered");
        assert_eq!(m[0].id, "grok-4.6");
        assert_eq!(m[0].display_name, "Grok 4.6");
        assert_eq!(m[0].efforts, vec!["xhigh", "high", "low"]);
        assert_eq!(m[0].default_effort.as_deref(), Some("high"));
        assert!(m[1].efforts.is_empty());
        assert_eq!(m[1].default_effort, None);
    }

    #[test]
    fn launch_spells_the_flags_grok_documents() {
        assert_eq!(GrokBackend.launch(&LaunchSpec::default()), "grok");
        let cmd = GrokBackend.launch(&LaunchSpec {
            model: Some("grok-4.6".into()),
            effort: Some("xhigh".into()),
            session_id: Some("e63b0f22-7d69-4084-aaf3-733816255e8e".into()),
            provider: None,
            prompt: None,
        });
        assert_eq!(
            cmd,
            "grok --model 'grok-4.6' --reasoning-effort 'xhigh' --session-id 'e63b0f22-7d69-4084-aaf3-733816255e8e'"
        );
        assert_eq!(
            GrokBackend.resume("e63b0f22-7d69-4084-aaf3-733816255e8e").unwrap(),
            "grok --resume 'e63b0f22-7d69-4084-aaf3-733816255e8e'"
        );
        assert_eq!(GrokBackend.clear("abc").unwrap(), "grok --session-id 'abc'");
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    #[test]
    fn todo_write_replays_replace_and_merge() {
        let log = concat!(
            r#"{"type":"assistant","content":"","tool_calls":[{"name":"todo_write","arguments":"{\"todos\":[{\"id\":\"1\",\"content\":\"Load context\",\"status\":\"in_progress\"},{\"id\":\"2\",\"content\":\"Pull data\",\"status\":\"pending\"}],\"merge\":false}"}]}"#, "\n",
            r#"{"type":"tool_result","content":"ok"}"#, "\n",
            r#"{"type":"assistant","content":"","tool_calls":[{"name":"todo_write","arguments":"{\"todos\":[{\"id\":\"1\",\"content\":\"Load context\",\"status\":\"completed\"}],\"merge\":true}"}]}"#, "\n",
        );
        let tasks = parse_tasks(log);
        assert_eq!(tasks.len(), 2, "merge:true updates in place, it does not truncate");
        assert_eq!(tasks[0].status, "completed");
        assert_eq!(tasks[1].subject, "Pull data");
    }

    #[test]
    fn artifacts_read_newest_first_with_the_last_tool_kept() {
        let log = concat!(
            r#"{"type":"assistant","content":"","tool_calls":[{"name":"write","arguments":"{\"file_path\":\"/tmp/a.py\",\"content\":\"x\"}"}]}"#, "\n",
            r#"{"type":"assistant","content":"","tool_calls":[{"name":"write","arguments":"{\"file_path\":\"/tmp/b.md\",\"content\":\"y\"}"}]}"#, "\n",
            r#"{"type":"assistant","content":"","tool_calls":[{"name":"search_replace","arguments":"{\"file_path\":\"/tmp/a.py\",\"new_string\":\"z\"}"}]}"#, "\n",
        );
        let arts = parse_artifacts(log);
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].path, "/tmp/a.py", "last touched leads");
        assert_eq!(arts[0].tool, "Edit", "the write was later edited");
        assert_eq!(arts[1].tool, "Write");
        assert_eq!(arts[0].at, "", "grok records carry no timestamps — none is invented");
    }
}
