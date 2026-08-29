//! The librarian: a small model that names, tags and threads sessions.
//!
//! A session's own title is its first prompt, or whatever the engine wrote,
//! which is why the sidebar reads "AI-OS", "hi", and "i think we already have
//! a clone of this repo: https://..." three times over. This module hands a
//! cheap model an excerpt of each session — opening prompt, last exchange,
//! files touched — together with the threads it has already named, and
//! writes back a short name, a few tags, which thread the session belongs to,
//! where it left off, and what would come next.
//!
//! Everything it learns lives in `~/.config/aiterm/librarian.json`, keyed by
//! session id. Nothing here runs unless the frontend asks it to: which model,
//! which provider, and which sessions are the caller's decisions, and the
//! provider's key never leaves this process (the same curl-config channel the
//! chat harness uses).

use std::collections::BTreeMap;
use std::io::Write;

use serde::{Deserialize, Serialize};

use crate::providers::Provider;

/// What the librarian wrote about one session.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Entry {
    /// A short name, six words or fewer.
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Id of the thread in `Store::threads` this session belongs to.
    #[serde(default)]
    pub thread: String,
    /// Where the session left off, in a sentence.
    #[serde(default)]
    pub summary: String,
    /// What would come next, if the transcript says; empty otherwise.
    #[serde(default)]
    pub next: String,
    /// The session's `last_active` when this was written. Newer activity
    /// makes the entry stale, and a run brings it up to date.
    #[serde(default)]
    pub seen: i64,
    /// When this was written, ms since the epoch.
    #[serde(default)]
    pub at: i64,
    #[serde(default)]
    pub model: String,
    /// Tags the person set by hand. Kept apart from the model's so no run
    /// can drop them, and shown to the model as facts.
    #[serde(default)]
    pub user_tags: Vec<String>,
}

/// A thread: a bundle of related sessions, named once and reused.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Thread {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created: i64,
    /// Tags the person set by hand — see `Entry::user_tags`.
    #[serde(default)]
    pub user_tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Store {
    #[serde(default)]
    pub sessions: BTreeMap<String, Entry>,
    #[serde(default)]
    pub threads: BTreeMap<String, Thread>,
    /// Total spend the providers have reported, in dollars, where they do.
    #[serde(default)]
    pub spent: f64,
    /// How many sessions the store held when it was last tidied — the
    /// second pass that merges threads. More than this now means it is due.
    #[serde(default)]
    pub tidied_sessions: usize,
    #[serde(default)]
    pub tidied_at: i64,
}

fn store_path() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".config/aiterm/librarian.json"))
}

pub fn load_store() -> Store {
    let Some(p) = store_path() else { return Store::default() };
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_store(s: &Store) -> Result<(), String> {
    let p = store_path().ok_or("no home directory")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| e.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A session the frontend wants looked at: its id, and the activity stamp
/// that decides whether an existing entry is still current.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub last_active: i64,
}

/// Which model reads the sessions, and how it is reached.
///
/// An installed CLI in its print mode runs on whatever plan the user already
/// pays for — `claude -p` on Haiku costs nothing extra — and an API provider
/// is there for a model none of the CLIs serve. Either way the prompt goes
/// through a private file or stdin, never the argv.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Engine {
    Api { provider_id: String, model: String },
    /// `agent` is a backend id: claude, codex or grok. `model` in that CLI's
    /// spelling, or none for its default.
    Cli { agent: String, model: Option<String> },
}

impl Engine {
    fn label(&self) -> String {
        match self {
            Engine::Api { model, .. } => model.clone(),
            Engine::Cli { agent, model } => format!("{agent}:{}", model.as_deref().unwrap_or("default")),
        }
    }
}

/// Where the CLIs run. A directory of their own, so an engine that saves a
/// transcript even in print mode (grok does) files it somewhere the session
/// list knows to skip — the frontend hides this path. Keep the two in step:
/// `LIBRARIAN_DIR` in `librarian.ts`.
fn lib_dir() -> Option<std::path::PathBuf> {
    let d = dirs::home_dir()?.join(".config/aiterm/librarian");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// What one run did.
#[derive(Serialize, Debug, Default, Clone, PartialEq)]
pub struct RunReport {
    /// Sessions written this run.
    pub done: usize,
    /// Sessions that still need a look — the run stops at `max`.
    pub remaining: usize,
    /// Spend the provider reported for this run, where it did.
    pub cost: f64,
    pub errors: Vec<String>,
}

/// An entry is current while nothing has happened in the session since it
/// was written. A little slack, because engines touch the transcript after
/// the last real exchange (title lines, token counts).
fn is_current(e: &Entry, last_active: i64) -> bool {
    last_active <= e.seen + 60_000
}

/// Which of the candidates a run would look at, oldest activity first so a
/// capped run works through the backlog in order.
pub fn pending<'a>(store: &Store, cands: &'a [Candidate]) -> Vec<&'a Candidate> {
    let mut v: Vec<&Candidate> = cands
        .iter()
        .filter(|c| store.sessions.get(&c.id).map_or(true, |e| !is_current(e, c.last_active)))
        .collect();
    v.sort_by_key(|c| c.last_active);
    v
}

/* ---- the excerpt a model sees ---------------------------------------- */

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

fn excerpt_of(d: &crate::detail::SessionDetail, agent: &str) -> serde_json::Value {
    let files: Vec<String> = d
        .files
        .iter()
        .take(10)
        .map(|f| f.rsplit('/').next().unwrap_or(f).to_string())
        .collect();
    serde_json::json!({
        "id": d.id,
        "engine": agent,
        "directory": d.cwd,
        "branch": d.branch,
        "title": clip(d.title.as_deref().unwrap_or(""), 120),
        "opening_prompt": clip(d.first_prompt.as_deref().unwrap_or(""), 700),
        "last_user_message": clip(d.last_user.as_deref().unwrap_or(""), 300),
        "last_assistant_message": clip(d.last_assistant.as_deref().unwrap_or(""), 500),
        "files_touched": files,
        "exchanges": d.user_messages,
    })
}

const SYSTEM: &str = "You are the librarian for a developer's AI coding sessions. You are given \
excerpts of sessions and the list of threads already named. A thread is a body of work a person \
would recognise as one thing — an app being built, a device being wired, a research question, a \
campaign — and usually spans several sessions and sometimes several directories. Name what the \
work IS, not what the tool is.\n\n\
For every session return one object, in the order given. Rules:\n\
- id: the session id, copied exactly.\n\
- name: 2–6 words, specific, sentence case, no trailing period. Never the raw prompt. A session that only says hi or tests the tool gets a plain name like \"Quick check\".\n\
- tags: 2–4 lowercase single words or hyphenated words naming the technology, product or domain (esp32, affiliate, real-estate, radarr, kalshi). Never generic words: automation, test, admin, development, setup, integration, tooling, research, ui. Reuse specific tags already in use where they fit.\n\
- thread: EITHER {\"id\": \"<existing thread id>\"} to file it under an existing thread, OR {\"new\": {\"name\": \"2–4 words\", \"description\": \"one sentence\", \"tags\": [...]}} when nothing existing fits, OR null for a one-off or a throwaway (a greeting, a smoke test, a quick question). Prefer an existing thread when the work is plainly the same body of work, even across directories. Two sessions in this batch that are the same work share one new thread: name it in the first and refer to it by that same name in the others.\n\
- summary: one sentence, ≤ 25 words, on where the session left off — what was done or decided last.\n\
- next: ≤ 15 words on the obvious next step, or \"\" if the session is finished or it is not clear.\n\n\
Reply with a JSON array only — no prose, no code fence.";

fn build_prompt(store: &Store, batch: &[serde_json::Value]) -> String {
    let threads: Vec<serde_json::Value> = store
        .threads
        .iter()
        .map(|(id, t)| serde_json::json!({"id": id, "name": t.name, "description": t.description, "tags": t.tags, "user_tags": t.user_tags}))
        .collect();
    let mut tags: Vec<&String> = store.sessions.values().flat_map(|e| e.tags.iter().chain(e.user_tags.iter())).collect();
    tags.sort();
    tags.dedup();
    let mut user: Vec<&String> = store.threads.values().flat_map(|t| t.user_tags.iter()).chain(store.sessions.values().flat_map(|e| e.user_tags.iter())).collect();
    user.sort();
    user.dedup();
    format!(
        "Existing threads (file sessions under these where they belong):\n{}\n\nTags already in use: {}\nTags the person set by hand (reuse these where they fit; they are how the person thinks about the work): {}\n\nSessions to catalogue:\n{}",
        serde_json::to_string_pretty(&threads).unwrap_or_default(),
        tags.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", "),
        user.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", "),
        serde_json::to_string_pretty(batch).unwrap_or_default(),
    )
}

/* ---- one non-streaming call ------------------------------------------- */

/// Ask once, whichever way the engine is reached.
fn ask(engine: &Engine, providers: &[Provider], system: &str, user: &str) -> Result<(String, Option<f64>), String> {
    match engine {
        Engine::Api { provider_id, model } => {
            let p = providers.iter().find(|p| &p.id == provider_id).ok_or("that provider is not configured")?;
            if p.api_key.is_empty() {
                return Err(format!("{} has no API key saved", p.name));
            }
            ask_api(p, model, system, user)
        }
        Engine::Cli { agent, model } => ask_cli(agent, model.as_deref(), system, user).map(|t| (t, None)),
    }
}

/// Ask an installed CLI in its print mode. Each one is told to use no tools
/// and to keep no session where it can be; the prompt goes in on stdin or a
/// private file.
fn ask_cli(agent: &str, model: Option<&str>, system: &str, user: &str) -> Result<String, String> {
    let dir = lib_dir().ok_or("no home directory")?;
    let combined = format!("{system}\n\n{user}");
    let mut cmd;
    let mut prompt_file: Option<std::path::PathBuf> = None;
    let mut last_file: Option<std::path::PathBuf> = None;
    match agent {
        "claude" => {
            cmd = std::process::Command::new("claude");
            cmd.args(["-p", "--output-format", "text", "--no-session-persistence", "--tools", "", "--system-prompt", system]);
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                cmd.args(["--model", m]);
            }
        }
        "codex" => {
            // Its stdout carries a token count; the reply itself goes to a
            // file of our naming.
            let f = dir.join(format!("codex-last-{}.txt", std::process::id()));
            cmd = std::process::Command::new("codex");
            cmd.args(["exec", "--ephemeral", "--skip-git-repo-check", "-o"]).arg(&f).arg("-");
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                cmd.args(["-m", m]);
            }
            last_file = Some(f);
        }
        "grok" => {
            let f = dir.join(format!("grok-prompt-{}.txt", std::process::id()));
            crate::providers::write_private(&f, &combined).map_err(|e| format!("could not stage the prompt: {e}"))?;
            cmd = std::process::Command::new("grok");
            cmd.args(["--output-format", "plain", "--prompt-file"]).arg(&f);
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                cmd.args(["--model", m]);
            }
            prompt_file = Some(f);
        }
        other => return Err(format!("{other} has no print mode aiterm knows")),
    }
    cmd.current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not run {agent}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // claude and codex read the prompt here; grok has its file and gets
        // an empty stdin so it cannot wait on a terminal.
        if prompt_file.is_none() {
            let text = if agent == "claude" { user.to_string() } else { combined.clone() };
            let _ = stdin.write_all(text.as_bytes());
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if let Some(f) = &prompt_file {
        let _ = std::fs::remove_file(f);
    }
    let text = match &last_file {
        Some(f) => {
            let t = std::fs::read_to_string(f).unwrap_or_default();
            let _ = std::fs::remove_file(f);
            t
        }
        None => String::from_utf8_lossy(&out.stdout).into_owned(),
    };
    if !out.status.success() && text.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{agent} failed: {}", err.lines().last().unwrap_or("no output").trim()));
    }
    if text.trim().is_empty() {
        return Err(format!("{agent} replied with nothing"));
    }
    Ok(text)
}

/// Ask an API provider once. The key goes to curl over its config stdin,
/// never the argv, exactly as the chat harness does it.
fn ask_api(p: &Provider, model: &str, system: &str, user: &str) -> Result<(String, Option<f64>), String> {
    let mut body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": 0.2,
    });
    if let Some(map) = body.as_object_mut() {
        if let Some(r) = crate::providers::routing_block(p, model) {
            map.insert("provider".into(), r);
        }
        if p.is_openrouter() {
            map.insert("usage".into(), serde_json::json!({"include": true}));
        }
    }
    let body_path = std::env::temp_dir().join(format!("aiterm-librarian-{}.json", std::process::id()));
    crate::providers::write_private(&body_path, &body.to_string())
        .map_err(|e| format!("could not stage the request: {e}"))?;
    let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
    let mut child = std::process::Command::new("curl")
        .args([
            "-sS", "--connect-timeout", "10", "--max-time", "180",
            "-H", "Content-Type: application/json",
            "--config", "-", "--data-binary",
        ])
        .arg(format!("@{}", body_path.display()))
        .arg(&url)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("curl took no stdin")?
        .write_all(crate::providers::curl_auth_config(&p.api_key).as_bytes())
        .map_err(|e| format!("could not pass the key to curl: {e}"))?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&body_path);
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!("curl could not reach {}: {}", p.name, String::from_utf8_lossy(&out.stderr).trim()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| format!("{} sent something that is not JSON", p.name))?;
    if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
        return Err(format!("{} refused: {msg}", p.name));
    }
    let text = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .ok_or_else(|| format!("{} sent no reply", p.name))?
        .to_string();
    let cost = v.pointer("/usage/cost").and_then(|c| c.as_f64());
    Ok((text, cost))
}

/// The reply as JSON, tolerating a code fence or a sentence around it.
pub fn parse_reply(text: &str) -> Result<Vec<serde_json::Value>, String> {
    let t = text.trim();
    let start = t.find('[').ok_or("no JSON array in the reply")?;
    let end = t.rfind(']').ok_or("no JSON array in the reply")?;
    if end < start {
        return Err("no JSON array in the reply".into());
    }
    serde_json::from_str::<Vec<serde_json::Value>>(&t[start..=end]).map_err(|e| format!("reply is not valid JSON: {e}"))
}

fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "thread".into() } else { s }
}

fn strings(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).take(5).collect())
        .unwrap_or_default()
}

/// Fold one batch's reply into the store. Pure, so it is testable without a
/// model: given what came back, this is what the store becomes.
pub fn apply(store: &mut Store, reply: &[serde_json::Value], asked: &[String], seen: &BTreeMap<String, i64>, model: &str) -> usize {
    let now = now_ms();
    let mut n = 0;
    // A reply that names no ids at all but answers one object per session,
    // in order, is taken in order — a small model drops the id field more
    // often than it reorders.
    let positional = reply.len() == asked.len() && reply.iter().all(|r| r.get("id").and_then(|v| v.as_str()).is_none());
    // Threads named by their name, or by the id of a session already filed
    // under them, resolve to the thread id.
    let mut by_name: BTreeMap<String, String> = store.threads.iter().map(|(id, t)| (t.name.to_lowercase(), id.clone())).collect();
    for (i, r) in reply.iter().enumerate() {
        let id: String = match r.get("id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None if positional => asked[i].clone(),
            None => continue,
        };
        let id = id.as_str();
        let Some(&last) = seen.get(id) else { continue };
        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        // The thread: an existing id, or a new one minted from its name — and
        // a "new" thread whose slug already exists is the existing one.
        let mut thread_id = String::new();
        if let Some(t) = r.get("thread").filter(|t| !t.is_null()) {
            if let Some(tid) = t.get("id").and_then(|v| v.as_str()) {
                if store.threads.contains_key(tid) {
                    thread_id = tid.to_string();
                } else if let Some(e) = store.sessions.get(tid).filter(|e| !e.thread.is_empty()) {
                    thread_id = e.thread.clone();
                } else if let Some(found) = by_name.get(&tid.to_lowercase()) {
                    thread_id = found.clone();
                } else if let Some(found) = by_name.get(&slug(tid).replace('-', " ")) {
                    thread_id = found.clone();
                }
            }
            if thread_id.is_empty() {
                if let Some(nt) = t.get("new") {
                    let tname = nt.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if !tname.is_empty() {
                        let tid = by_name.get(&tname.to_lowercase()).cloned().unwrap_or_else(|| slug(&tname));
                        store.threads.entry(tid.clone()).or_insert_with(|| Thread {
                            name: tname.clone(),
                            description: nt.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
                            tags: strings(nt.get("tags")),
                            created: now,
                            user_tags: Vec::new(),
                        });
                        by_name.insert(tname.to_lowercase(), tid.clone());
                        thread_id = tid;
                    }
                }
            }
        }
        store.sessions.insert(
            id.to_string(),
            Entry {
                name,
                tags: strings(r.get("tags")),
                thread: thread_id,
                summary: r.get("summary").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
                next: r.get("next").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
                seen: last,
                at: now,
                model: model.to_string(),
                // A re-read never loses what the person set by hand.
                user_tags: store.sessions.get(id).map(|e| e.user_tags.clone()).unwrap_or_default(),
            },
        );
        n += 1;
    }
    n
}

/* ---- the second pass: one look at everything, and a final organisation --- */

const TIDY_SYSTEM: &str = "You are finalising a catalogue of a developer's AI coding sessions. You are given every \
thread so far, each with the sessions filed under it, and the sessions filed under none. The threads were named \
a few sessions at a time, so the same body of work is often split across several — a device wired in one thread \
and programmed in another, a product's build and its marketing apart, a server's media stack in two. Your job is \
the organisation a person would recognise.\n\n\
Rules:\n\
- Merge threads that are the same body of work: the same project, device, product, site, campaign or bot — \
including all its parts. Wiring, programming, peripherals (a camera, a display, buttons, a laser) and debugging of \
one board or one physical rig are ONE thread. The build, the research and the marketing of one product or one \
site are ONE thread. The download clients, indexers and webhooks of one home server are ONE thread. Sessions in \
the same project directory almost always belong together. Keep apart only work on genuinely different things: two \
different products, two different clients, two unrelated devices.\n\
- name: 2–4 words naming what the work IS — the product, device, site or bot — sentence case.\n\
- description: one sentence.\n\
- tags: 2–4 lowercase words naming the technology, product or domain. Never generic words: automation, test, \
admin, development, setup, integration, tooling, research, ui.\n\
- add: file a loose session under a thread when it plainly belongs (a session about Radarr belongs with the \
media server). Leave greetings, smoke tests and true one-offs loose — do not invent a thread for them.\n\
- Every existing thread id must appear in exactly one merge list. A thread that stands alone is a merge list of \
one.\n\
- user_tags were set by the person by hand and are facts, not guesses: sessions or threads sharing a user tag are \
the same body of work and belong together; a thread's user tags say what it is about. Never contradict them.\n\n\
Reply with a JSON object only — no prose, no code fence: \
{\"threads\": [{\"name\": \"...\", \"description\": \"...\", \"tags\": [...], \"merge\": [\"thread-id\", ...], \"add\": [\"session-id\", ...]}]}";

fn tidy_prompt(store: &Store, dirs: &BTreeMap<String, String>) -> String {
    let brief = |id: &str, e: &Entry| serde_json::json!({
        "id": id, "name": e.name, "summary": clip(&e.summary, 160),
        "directory": dirs.get(id).cloned().unwrap_or_default(),
        "user_tags": e.user_tags,
    });
    let threads: Vec<serde_json::Value> = store.threads.iter().map(|(id, t)| {
        let ss: Vec<serde_json::Value> = store.sessions.iter().filter(|(_, e)| &e.thread == id).map(|(sid, e)| brief(sid, e)).collect();
        serde_json::json!({"id": id, "name": t.name, "description": t.description, "tags": t.tags, "user_tags": t.user_tags, "sessions": ss})
    }).collect();
    let loose: Vec<serde_json::Value> = store.sessions.iter().filter(|(_, e)| e.thread.is_empty() || !store.threads.contains_key(&e.thread)).map(|(sid, e)| brief(sid, e)).collect();
    format!(
        "Threads so far:\n{}\n\nSessions under no thread:\n{}",
        serde_json::to_string_pretty(&threads).unwrap_or_default(),
        serde_json::to_string_pretty(&loose).unwrap_or_default(),
    )
}

/// Fold the tidy reply into the store. Pure. A thread the reply never
/// mentions is kept as it was — a model that forgot one must not lose it.
pub fn apply_tidy(store: &mut Store, reply: &serde_json::Value) -> Result<(usize, usize), String> {
    let finals = reply.get("threads").and_then(|t| t.as_array()).ok_or("no threads in the reply")?;
    let now = now_ms();
    let before = store.threads.len();
    let mut new_threads: BTreeMap<String, Thread> = BTreeMap::new();
    // old thread id -> new thread id
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    let mut filed = 0usize;
    for f in finals {
        let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        let merge: Vec<String> = strings_raw(f.get("merge")).into_iter().filter(|id| store.threads.contains_key(id) && !moved.contains_key(id)).collect();
        let add: Vec<String> = strings_raw(f.get("add"));
        if merge.is_empty() && add.is_empty() {
            continue;
        }
        // Keep an id where one thread carries on — the card keeps its colour
        // and its folds — and mint one where several become one.
        let id = if merge.len() == 1 { merge[0].clone() } else { slug(&name) };
        let id = if new_threads.contains_key(&id) { format!("{id}-{}", new_threads.len()) } else { id };
        let created = merge.iter().filter_map(|m| store.threads.get(m)).map(|t| t.created).filter(|c| *c > 0).min().unwrap_or(now);
        // The person's tags survive a merge, all of them.
        let mut user_tags: Vec<String> = merge.iter().filter_map(|m| store.threads.get(m)).flat_map(|t| t.user_tags.clone()).collect();
        user_tags.sort();
        user_tags.dedup();
        let mut tags = strings(f.get("tags"));
        if tags.is_empty() {
            tags = merge.iter().filter_map(|m| store.threads.get(m)).flat_map(|t| t.tags.clone()).collect();
            tags.dedup();
        }
        new_threads.insert(id.clone(), Thread {
            name,
            description: f.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
            tags,
            created,
            user_tags,
        });
        for m in &merge {
            moved.insert(m.clone(), id.clone());
        }
        for sid in add {
            if let Some(e) = store.sessions.get_mut(&sid) {
                if e.thread.is_empty() || !store.threads.contains_key(&e.thread) {
                    e.thread = id.clone();
                    filed += 1;
                }
            }
        }
    }
    // Threads the reply left out carry on untouched.
    for (id, t) in &store.threads {
        if !moved.contains_key(id) {
            new_threads.entry(id.clone()).or_insert_with(|| t.clone());
            moved.insert(id.clone(), id.clone());
        }
    }
    for e in store.sessions.values_mut() {
        if let Some(to) = moved.get(&e.thread) {
            e.thread = to.clone();
        }
    }
    // A thread with nothing under it is not a thread.
    let used: std::collections::BTreeSet<&String> = store.sessions.values().map(|e| &e.thread).collect();
    new_threads.retain(|id, _| used.contains(id));
    store.threads = new_threads;
    store.tidied_sessions = store.sessions.len();
    store.tidied_at = now;
    Ok((before, filed))
}

fn strings_raw(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

#[derive(Serialize, Debug, Default, Clone, PartialEq)]
pub struct TidyReport {
    pub threads_before: usize,
    pub threads_after: usize,
    /// Loose sessions filed under a thread.
    pub filed: usize,
    pub cost: f64,
}

fn tidy_sync(engine: Engine) -> Result<TidyReport, String> {
    let _one_at_a_time = RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let providers = crate::providers::load_providers();
    let mut store = load_store();
    if store.threads.is_empty() {
        return Err("nothing to tidy yet".into());
    }
    // Directory names help the model tell a body of work apart — cheap to
    // look up, and the store does not carry them.
    let dirs: BTreeMap<String, String> = store.sessions.keys().filter_map(|id| {
        let d = crate::detail::session_detail_sync(id.clone())?.cwd.unwrap_or_default();
        Some((id.clone(), d.rsplit('/').next().unwrap_or("").to_string()))
    }).collect();
    let (text, cost) = ask(&engine, &providers, TIDY_SYSTEM, &tidy_prompt(&store, &dirs))?;
    let t = text.trim();
    let start = t.find('{').ok_or("no JSON object in the reply")?;
    let end = t.rfind('}').ok_or("no JSON object in the reply")?;
    let reply: serde_json::Value = serde_json::from_str(&t[start..=end]).map_err(|e| format!("reply is not valid JSON: {e}"))?;
    store = load_store();
    let (before, filed) = apply_tidy(&mut store, &reply)?;
    if let Some(c) = cost {
        store.spent += c;
    }
    save_store(&store)?;
    Ok(TidyReport { threads_before: before, threads_after: store.threads.len(), filed, cost: cost.unwrap_or(0.0) })
}

#[tauri::command]
pub async fn librarian_tidy(engine: Engine) -> Result<TidyReport, String> {
    crate::run_blocking(move || tidy_sync(engine)).await
}

/// Sessions per model call. Several at once is what lets the model see that
/// two sessions are the same work; too many and a small model loses the
/// thread list.
const BATCH: usize = 8;

/// One run at a time. Two overlapping runs each loaded the store, each
/// saved it, and the second save dropped what the first had written.
static RUN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn run_sync(engine: Engine, cands: Vec<Candidate>, max: usize) -> Result<RunReport, String> {
    let _one_at_a_time = RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let providers = crate::providers::load_providers();
    let model = engine.label();
    let mut store = load_store();
    let todo: Vec<Candidate> = pending(&store, &cands).into_iter().cloned().collect();
    let total = todo.len();
    let mut report = RunReport::default();
    let list = crate::agents::backends();
    for chunk in todo.iter().take(max).collect::<Vec<_>>().chunks(BATCH) {
        let mut batch = Vec::new();
        let mut asked = Vec::new();
        let mut seen = BTreeMap::new();
        for c in chunk {
            let Some(d) = crate::detail::session_detail_sync(c.id.clone()) else { continue };
            let agent = crate::agents::owner_in(&list, &c.id).map(|(b, _)| b.id().to_string()).unwrap_or_default();
            batch.push(excerpt_of(&d, &agent));
            asked.push(c.id.clone());
            seen.insert(c.id.clone(), c.last_active);
        }
        if batch.is_empty() {
            continue;
        }
        match ask(&engine, &providers, SYSTEM, &build_prompt(&store, &batch)) {
            Ok((text, cost)) => match parse_reply(&text) {
                Ok(reply) => {
                    // Re-read before writing: the model took a minute, and
                    // anything written meanwhile — by hand, by a test — is
                    // kept rather than overwritten with this run's copy.
                    store = load_store();
                    let n = apply(&mut store, &reply, &asked, &seen, &model);
                    if n == 0 {
                        report.errors.push("the model answered, but about none of the sessions it was asked about".into());
                    }
                    report.done += n;
                    if let Some(c) = cost {
                        report.cost += c;
                        store.spent += c;
                    }
                    save_store(&store)?;
                }
                Err(e) => report.errors.push(e),
            },
            Err(e) => {
                report.errors.push(e);
                break; // a provider that refused once will refuse again
            }
        }
    }
    report.remaining = total.saturating_sub(report.done).min(total);
    Ok(report)
}

/* ---- commands ---------------------------------------------------------- */

#[tauri::command]
pub async fn librarian_state() -> Store {
    crate::run_blocking(load_store).await
}

#[tauri::command]
pub async fn librarian_run(engine: Engine, sessions: Vec<Candidate>, max: usize) -> Result<RunReport, String> {
    crate::run_blocking(move || run_sync(engine, sessions, max)).await
}

/// How many of these sessions a run would look at, without running.
#[tauri::command]
pub async fn librarian_pending(sessions: Vec<Candidate>) -> usize {
    crate::run_blocking(move || pending(&load_store(), &sessions).len()).await
}

/// Forget everything and start over — a different model, or a bad first pass.
#[tauri::command]
pub async fn librarian_forget() -> Result<(), String> {
    crate::run_blocking(|| save_store(&Store::default())).await
}

/// One tag the person sets or clears, on a thread or a session.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TagTarget {
    /// "thread" or "session".
    pub kind: String,
    pub id: String,
}

fn set_tag(list: &mut Vec<String>, tag: &str, on: bool) {
    let tag = tag.trim().to_lowercase().replace(' ', "-");
    if tag.is_empty() {
        return;
    }
    list.retain(|t| t != &tag);
    if on {
        list.push(tag);
    }
    list.sort();
}

#[tauri::command]
pub async fn librarian_tag(target: TagTarget, tag: String, on: bool) -> Result<(), String> {
    crate::run_blocking(move || {
        let mut s = load_store();
        match target.kind.as_str() {
            "thread" => set_tag(&mut s.threads.get_mut(&target.id).ok_or("no such thread")?.user_tags, &tag, on),
            "session" => set_tag(&mut s.sessions.get_mut(&target.id).ok_or("that session has not been read yet")?.user_tags, &tag, on),
            _ => return Err("tag what?".into()),
        }
        save_store(&s)
    })
    .await
}

/// Rename a thread by hand; the model's names are a first draft.
#[tauri::command]
pub async fn librarian_rename_thread(id: String, name: String) -> Result<(), String> {
    crate::run_blocking(move || {
        let mut s = load_store();
        let t = s.threads.get_mut(&id).ok_or("no such thread")?;
        t.name = name.trim().to_string();
        save_store(&s)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real run through `claude -p` on Haiku over the six most recent
    /// Claude sessions on this machine. Rides the subscription, writes to the
    /// real store, so it is opt-in:
    /// `cargo test --lib librarian::tests::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live() {
        let dir = dirs::home_dir().unwrap().join(".claude/projects/-home-admin-AI-OS");
        let mut files: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |x| x == "jsonl"))
            .map(|e| (e.metadata().unwrap().modified().unwrap(), e.path().file_stem().unwrap().to_string_lossy().into_owned()))
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0));
        let cands: Vec<Candidate> = files
            .iter()
            .take(6)
            .map(|(t, id)| Candidate { id: id.clone(), last_active: t.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64 })
            .collect();
        let engine = Engine::Cli { agent: "claude".into(), model: Some("haiku".into()) };
        let report = run_sync(engine, cands, 6).unwrap();
        println!("{report:?}");
        let store = load_store();
        for (id, t) in &store.threads {
            println!("THREAD {id}: {} — {} {:?}", t.name, t.description, t.tags);
        }
        for (id, e) in &store.sessions {
            println!("  {:8} [{}] {} {:?}\n           left off: {}\n           next: {}", &id[..8], e.thread, e.name, e.tags, e.summary, e.next);
        }
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.done > 0);
    }

    fn cand(id: &str, last: i64) -> Candidate {
        Candidate { id: id.into(), last_active: last }
    }

    #[test]
    fn a_reply_files_sessions_under_existing_and_new_threads() {
        let mut store = Store::default();
        store.threads.insert("esp32-clock".into(), Thread { name: "ESP32 clock".into(), ..Default::default() });
        let reply = parse_reply(r#"Here you go:
[
  {"id":"a","name":"Breadboard LED wiring","tags":["esp32","hardware"],"thread":{"id":"esp32-clock"},"summary":"Blue LED had no ground return.","next":"Rewire blue to the left rail"},
  {"id":"b","name":"Affiliate tracker audit","tags":["affiliate","Audit"],"thread":{"new":{"name":"Affiliate platform","description":"The self-hosted click tracker.","tags":["affiliate"]}},"summary":"Read-only audit done.","next":""},
  {"id":"c","name":"Also affiliate","tags":[],"thread":{"new":{"name":"Affiliate Platform"}},"summary":"","next":""},
  {"id":"zzz","name":"Not asked for","thread":{"id":"esp32-clock"}}
]"#).unwrap();
        let seen = BTreeMap::from([("a".to_string(), 10), ("b".to_string(), 20), ("c".to_string(), 30)]);
        let asked: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(apply(&mut store, &reply, &asked, &seen, "haiku"), 3);
        assert_eq!(store.sessions["a"].thread, "esp32-clock");
        assert_eq!(store.sessions["b"].thread, "affiliate-platform");
        assert_eq!(store.sessions["b"].tags, vec!["affiliate", "audit"]);
        // The second "new" thread with the same name is the same thread.
        assert_eq!(store.sessions["c"].thread, "affiliate-platform");
        assert_eq!(store.threads.len(), 2);
        assert_eq!(store.threads["affiliate-platform"].description, "The self-hosted click tracker.");
        assert!(!store.sessions.contains_key("zzz"));
        assert_eq!(store.sessions["a"].seen, 10);
    }

    /// What the first live run actually sent back: no id field at all, one
    /// object per session in order, and a second session pointing at the
    /// first's *session* id to mean "same thread".
    #[test]
    fn an_id_less_reply_in_order_still_lands_and_a_session_id_names_its_thread() {
        let mut store = Store::default();
        let reply = parse_reply(r#"[
          {"name":"App walkthrough","tags":["ui"],"thread":{"new":{"name":"aiterm desktop app","description":"The app.","tags":["ui"]}},"summary":"s","next":""},
          {"name":"Sidebar hover","tags":["ui"],"thread":{"id":"s1"},"summary":"s","next":""},
          {"name":"Quick check","tags":[],"thread":null,"summary":"hi","next":""},
          {"name":"By name","tags":[],"thread":{"id":"Aiterm Desktop App"},"summary":"","next":""}
        ]"#).unwrap();
        let asked: Vec<String> = ["s1", "s2", "s3", "s4"].iter().map(|s| s.to_string()).collect();
        let seen: BTreeMap<String, i64> = asked.iter().map(|s| (s.clone(), 1)).collect();
        assert_eq!(apply(&mut store, &reply, &asked, &seen, "m"), 4);
        assert_eq!(store.sessions["s1"].thread, "aiterm-desktop-app");
        assert_eq!(store.sessions["s2"].thread, "aiterm-desktop-app");
        assert_eq!(store.sessions["s3"].thread, "");
        assert_eq!(store.sessions["s4"].thread, "aiterm-desktop-app");
        assert_eq!(store.threads.len(), 1);
    }

    /// The tidy pass: four ESP32 threads become one (a new id, the oldest
    /// creation date), a lone thread keeps its id, a loose session is filed,
    /// a thread the reply forgot survives, and an emptied thread is gone.
    #[test]
    fn tidy_merges_files_and_forgets_nothing() {
        let mut store = Store::default();
        for (id, name, created) in [("esp32-wiring", "ESP32 wiring", 5), ("esp32-camera", "ESP32 camera", 9), ("kalshi-bot", "Kalshi bot", 7), ("forgotten", "Forgotten", 3), ("empty", "Empty", 1)] {
            store.threads.insert(id.into(), Thread { name: name.into(), created, tags: vec!["automation".into()], ..Default::default() });
        }
        store.threads.get_mut("esp32-wiring").unwrap().user_tags = vec!["workbench".into()];
        store.threads.get_mut("esp32-camera").unwrap().user_tags = vec!["rig".into(), "workbench".into()];
        for (sid, th) in [("a", "esp32-wiring"), ("b", "esp32-camera"), ("c", "kalshi-bot"), ("d", "forgotten"), ("loose", "")] {
            store.sessions.insert(sid.into(), Entry { thread: th.into(), name: sid.into(), ..Default::default() });
        }
        let reply: serde_json::Value = serde_json::from_str(r#"{"threads":[
          {"name":"ESP32 clock","description":"A clock on an ESP32-S3.","tags":["esp32","hardware"],"merge":["esp32-wiring","esp32-camera"],"add":["loose"]},
          {"name":"Kalshi trading bot","description":"","tags":[],"merge":["kalshi-bot"],"add":[]},
          {"name":"Ghost","description":"","tags":[],"merge":["empty"],"add":[]}
        ]}"#).unwrap();
        let (before, filed) = apply_tidy(&mut store, &reply).unwrap();
        assert_eq!((before, filed), (5, 1));
        assert_eq!(store.sessions["a"].thread, "esp32-clock");
        assert_eq!(store.sessions["b"].thread, "esp32-clock");
        assert_eq!(store.sessions["loose"].thread, "esp32-clock");
        assert_eq!(store.threads["esp32-clock"].created, 5);
        assert_eq!(store.threads["esp32-clock"].tags, vec!["esp32", "hardware"]);
        assert_eq!(store.threads["esp32-clock"].user_tags, vec!["rig", "workbench"], "the person's tags survive a merge, unioned");
        assert_eq!(store.sessions["c"].thread, "kalshi-bot");
        assert_eq!(store.threads["kalshi-bot"].tags, vec!["automation"], "empty tags keep the old ones");
        assert_eq!(store.sessions["d"].thread, "forgotten");
        assert!(store.threads.contains_key("forgotten"));
        assert!(!store.threads.contains_key("empty"));
        assert_eq!(store.threads.len(), 3);
        assert_eq!(store.tidied_sessions, 5);
    }

    /// Against the real store, through claude -p on Haiku. Opt-in:
    /// `cargo test --lib librarian::tests::tidy_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn tidy_live() {
        let r = tidy_sync(Engine::Cli { agent: "claude".into(), model: Some("haiku".into()) }).unwrap();
        println!("{r:?}");
        let store = load_store();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for e in store.sessions.values() { *counts.entry(e.thread.as_str()).or_default() += 1; }
        for (id, t) in &store.threads { println!("  [{:2}] {id}: {} {:?} — {}", counts.get(id.as_str()).unwrap_or(&0), t.name, t.tags, t.description); }
        println!("  loose: {}", counts.get("").unwrap_or(&0));
    }

    #[test]
    fn pending_is_what_is_new_or_has_moved_on() {
        let mut store = Store::default();
        store.sessions.insert("done".into(), Entry { seen: 1_000, ..Default::default() });
        store.sessions.insert("stale".into(), Entry { seen: 1_000, ..Default::default() });
        let cands = vec![cand("new", 5), cand("done", 1_000 + 30_000), cand("stale", 1_000 + 120_000)];
        let ids: Vec<&str> = pending(&store, &cands).iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["new", "stale"]);
    }

    #[test]
    fn a_fenced_or_prosed_reply_still_parses_and_garbage_does_not() {
        assert_eq!(parse_reply("```json\n[{\"id\":\"x\"}]\n```").unwrap().len(), 1);
        assert!(parse_reply("I could not do that.").is_err());
        assert!(parse_reply("[{broken").is_err());
    }

    #[test]
    fn a_hand_set_tag_is_normalised_and_toggles() {
        let mut v = vec!["esp32".to_string()];
        set_tag(&mut v, "  Home Lab ", true);
        assert_eq!(v, vec!["esp32", "home-lab"]);
        set_tag(&mut v, "HOME-LAB", true);
        assert_eq!(v, vec!["esp32", "home-lab"], "no duplicates");
        set_tag(&mut v, "esp32", false);
        assert_eq!(v, vec!["home-lab"]);
        set_tag(&mut v, "   ", true);
        assert_eq!(v, vec!["home-lab"]);
    }

    #[test]
    fn slugs_are_stable_and_never_empty() {
        assert_eq!(slug("ESP32 Clock!"), "esp32-clock");
        assert_eq!(slug("  gTrade / Hyperliquid copy-trade "), "gtrade-hyperliquid-copy-trade");
        assert_eq!(slug("???"), "thread");
    }
}
