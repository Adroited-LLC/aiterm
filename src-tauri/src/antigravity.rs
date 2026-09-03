//! Google's Antigravity CLI (`agy`) as a backend.
//!
//! Everything here was read off agy 1.1.24 on 2026-09-02 against the real
//! store on this machine (`~/.gemini/antigravity-cli`), never guessed.
//!
//! A conversation is spread over four places under that root, keyed by one
//! uuid:
//!
//! * `conversations/<id>.db` — SQLite, every step as a protobuf blob. Not
//!   parsed: the transcript beside it says the same thing in JSON.
//! * `brain/<id>/.system_generated/logs/transcript.jsonl` — one JSON object
//!   per step, appended as the turn progresses. This is the row's file and
//!   the conversation's source.
//! * `annotations/<id>.pbtxt` — `title:"…"`, the title agy generates itself.
//! * `cache/last_conversations.json` — `{"<cwd>": "<id>"}`, the map
//!   `--continue` reads. One id per cwd, overwritten by every run, so older
//!   conversations in the same directory have their cwd only in the DB's
//!   `trajectory_metadata_blob` as a `file://` URI.
//!
//! `conversation_summaries.db` and `cache/conversation_metadata.json` look
//! like an index but hold only the IDE's conversations
//! (`app_data_dir='antigravity'`) — the CLI never wrote any of its eight
//! into them, so they are ignored.
//!
//! agy cannot be told an id at launch (no `--session-id` equivalent; the id
//! is minted server-side and printed only on exit), so a new tab is
//! *adopted*: the row appears under the launch cwd a moment after start and
//! `adopt_agent_session` re-keys the placeholder to it. `--conversation <id>`
//! resumes from any directory — verified from a different cwd, `num_turns:2`.
//!
//! No 🗑: a conversation is four files in four directories plus a `presence/`
//! lock, and `session_delete` renames one file. No `/clear` either — it
//! would just be a fresh `agy`, which is what ＋ already does.

use crate::agents::{
    detect_cli, prompt_of, q, AgentBackend, Caps, Detection, LaunchSpec, ModelOption,
    PermissionMode,
};
use crate::sessions::{Session, SessionProvider};
use std::path::{Path, PathBuf};

pub struct AntigravityBackend;
pub struct AntigravitySessions;

/// The default model agy 1.1.24 picks when none is pinned — `/model` in
/// print mode answered `gemini-3.8-flash-high`.
pub const DEFAULT_MODEL: &str = "gemini-3.8-flash-high";

/// agy's permission presets, from `agy --help` 1.1.24: `--mode accept-edits|plan`
/// and `--dangerously-skip-permissions`.
///
/// The first passes nothing, so `agentMode` in
/// `~/.gemini/antigravity-cli/settings.json` still decides. Note that the
/// account-level `toolPermission: always-proceed` in that same file
/// auto-approves every tool regardless of mode — on such an account nothing
/// ever prompts, and aiterm's needs-you never fires.
pub const ANTIGRAVITY_PERMISSION_MODES: &[PermissionMode] = &[
    PermissionMode {
        id: "default",
        label: "Antigravity's own default",
        note: "Whatever agentMode in ~/.gemini/antigravity-cli/settings.json says; toolPermission there can still auto-approve everything.",
        flags: &[],
    },
    PermissionMode {
        id: "plan",
        label: "Plan mode",
        note: "--mode plan: reads and plans, changes nothing until told to.",
        flags: &["--mode plan"],
    },
    PermissionMode {
        id: "accept-edits",
        label: "Accept edits",
        note: "--mode accept-edits: file edits run without asking; commands still go through toolPermission.",
        flags: &["--mode accept-edits"],
    },
    PermissionMode {
        id: "dangerously-skip-permissions",
        label: "Skip all permissions",
        note: "--dangerously-skip-permissions: every tool call runs without asking.",
        flags: &["--dangerously-skip-permissions"],
    },
];

/// `~/.gemini/antigravity-cli` — the CLI's own data directory (the log says
/// so: `CLI app data directory: …/.gemini/antigravity-cli`). `None` unless it
/// exists; `~/.gemini/antigravity` beside it is the IDE's and is not looked at.
pub(crate) fn store_root() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".gemini").join("antigravity-cli");
    dir.is_dir().then_some(dir)
}

/// Ids are uuids in file names; anything else must not reach a path join.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub(crate) fn transcript_path(root: &Path, id: &str) -> PathBuf {
    root.join("brain")
        .join(id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl")
}

fn mtime_ms(p: &Path) -> u64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The title out of `annotations/<id>.pbtxt` — a text proto whose whole
/// content is `title:"Google Ads Performance Review"`. Backslash escapes are
/// undone; `None` when there is no `title:` field.
pub fn parse_title(pbtxt: &str) -> Option<String> {
    let start = pbtxt.find("title:")? + "title:".len();
    let rest = pbtxt[start..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// `cache/last_conversations.json` is `{"<cwd>": "<id>"}`; the scanner wants
/// it the other way round.
pub fn parse_last_conversations(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(text) {
        for (cwd, id) in map {
            if let Some(id) = id.as_str() {
                out.insert(id.to_string(), cwd);
            }
        }
    }
    out
}

/// What the person typed, out of a `USER_INPUT` step's `content`.
///
/// agy wraps the prompt in `<USER_REQUEST>…</USER_REQUEST>` and follows it
/// with `<ADDITIONAL_METADATA>` (the local time) and, on the first turn,
/// `<USER_SETTINGS_CHANGE>` (which model was picked). Only the request is the
/// person. A content with no such tag is taken whole. [observed: agy 1.1.24]
pub fn user_request(content: &str) -> String {
    const OPEN: &str = "<USER_REQUEST>";
    const CLOSE: &str = "</USER_REQUEST>";
    match (content.find(OPEN), content.rfind(CLOSE)) {
        (Some(a), Some(b)) if a + OPEN.len() <= b => content[a + OPEN.len()..b].to_string(),
        _ => content.to_string(),
    }
}

/// A tool-call argument as a string. In `transcript.jsonl` every `args`
/// value is a string, and string-typed arguments are JSON-encoded *inside*
/// it — `"Cwd":"\"/home/john/nanoclaw\""` — while numbers and booleans are
/// bare (`"MaxDepth":"3"`). One decode undoes the inner quoting.
/// [observed: agy 1.1.24; `transcript_full.jsonl` has real types instead]
pub(crate) fn arg_str(call: &serde_json::Value, key: &str) -> Option<String> {
    let raw = call.get("args")?.get(key)?;
    match raw {
        serde_json::Value::String(s) => {
            if s.starts_with('"') {
                serde_json::from_str::<String>(s).ok().or_else(|| Some(s.clone()))
            } else {
                Some(s.clone())
            }
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The one line a person reads for a tool call: agy writes its own
/// `toolSummary` on every call ("Search memory for Google Ads tools"); the
/// tool's name stands in when it is missing.
pub(crate) fn tool_summary(call: &serde_json::Value) -> String {
    arg_str(call, "toolSummary")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            call.get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("tool")
                .to_string()
        })
}

/// The tools that write a file and the argument naming it, as read off a
/// real session: `write_to_file` and `replace_file_content` both carry
/// `TargetFile`. `multi_replace_file_content` and `sed_file` are in agy's
/// tool list too but no session here used them, so their argument shape is
/// unknown and they are not read. [observed: agy 1.1.24, session 1e6213a2]
fn write_tool_of(name: &str) -> Option<&'static str> {
    match name {
        "write_to_file" => Some("Write"),
        "replace_file_content" => Some("Edit"),
        _ => None,
    }
}

/// The conversation from `transcript.jsonl`, as `(role, text)`.
///
/// `USER_INPUT` is the person (request only, see [`user_request`]);
/// `PLANNER_RESPONSE` with `content` is the assistant's prose. A response that
/// is only `tool_calls` becomes one line per call from its `toolSummary`, so
/// the view says what happened between two answers without the arguments.
/// `thinking` is the engine talking to itself and is not surfaced. `GENERIC`
/// (tool results) and `SYSTEM_MESSAGE` (the "server restart" notice every
/// resume adds) are skipped, matching what grok's view does with tool output.
/// [observed: agy 1.1.24]
pub fn parse_messages(text: &str) -> Vec<(String, String)> {
    parse_messages_with(text, &|_, content| content.to_string())
}

/// [`parse_messages`] with a hand that can put back what agy cut: `recover`
/// gets each `PLANNER_RESPONSE`'s `step_index` and its `content` as logged,
/// and returns the content to show. See [`recover_truncated`].
pub fn parse_messages_with(text: &str, recover: &dyn Fn(u64, &str) -> String) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            match v.get("type")?.as_str()? {
                "USER_INPUT" => {
                    let body = user_request(v.get("content")?.as_str()?);
                    if crate::sessions::is_only_system_block(&body) {
                        return None;
                    }
                    let body = body.trim();
                    (!body.is_empty()).then(|| ("user".to_string(), body.to_string()))
                }
                "PLANNER_RESPONSE" => {
                    let logged = v.get("content").and_then(|c| c.as_str()).unwrap_or("").trim();
                    let index = v.get("step_index").and_then(|i| i.as_u64()).unwrap_or(0);
                    let content = if logged.is_empty() { String::new() } else { recover(index, logged) };
                    let content = content.trim();
                    if !content.is_empty() {
                        return Some(("assistant".to_string(), content.to_string()));
                    }
                    let lines: Vec<String> = v
                        .get("tool_calls")
                        .and_then(|t| t.as_array())
                        .map(|calls| calls.iter().map(tool_summary).collect())
                        .unwrap_or_default();
                    (!lines.is_empty()).then(|| ("assistant".to_string(), lines.join("\n")))
                }
                _ => None,
            }
        })
        .collect()
}

/// The first prompt, as one line — the title when agy has not written one.
pub fn first_request_line(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if v.get("type")?.as_str()? != "USER_INPUT" {
            return None;
        }
        let body = user_request(v.get("content")?.as_str()?);
        let line = body.split_whitespace().collect::<Vec<_>>().join(" ");
        (!line.is_empty()).then_some(line)
    })
}

/// The `Cwd` of the first `run_command` call — where the session ran when
/// `last_conversations.json` no longer says.
pub fn first_run_command_cwd(text: &str) -> Option<String> {
    text.lines()
        .filter(|l| l.contains("run_command"))
        .find_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            v.get("tool_calls")?.as_array()?.iter().find_map(|call| {
                (call.get("name")?.as_str()? == "run_command")
                    .then(|| arg_str(call, "Cwd"))
                    .flatten()
                    .filter(|c| c.starts_with('/'))
            })
        })
}

/// agy logs a step's `content` in `transcript.jsonl` capped near 4 KiB: the
/// head and the tail are kept and the middle is replaced by a line reading
/// `<truncated N bytes>`, with `"truncated_fields":["content"]` on the
/// record. The whole text still sits in `conversations/<id>.db`, table
/// `steps`, column `step_payload`, row `idx` = `step_index`, as one protobuf
/// blob — and a protobuf string is its bytes, contiguous. So the middle is
/// the N bytes between the logged head and the logged tail in that blob.
/// [observed: agy 1.1.25, 2026-09-03 — a 5.7 KB answer logged as 4116 chars]
const TRUNCATED_MARK: &str = "<truncated ";

/// `content` with its `<truncated N bytes>` gap filled from `payload`, or
/// `None` when there is no gap, the head or tail cannot be found in the
/// blob, or the bytes between them are not the N the marker promised.
pub fn splice_truncated(content: &str, payload: &[u8]) -> Option<String> {
    let at = content.find(TRUNCATED_MARK)?;
    let rest = &content[at + TRUNCATED_MARK.len()..];
    let close = rest.find(" bytes>")?;
    let n: usize = rest[..close].parse().ok()?;
    let mark_end = at + TRUNCATED_MARK.len() + close + " bytes>".len();
    // agy puts the marker on a line of its own; those two newlines are the
    // marker's, not the text's.
    let head = content[..at].strip_suffix('\n').unwrap_or(&content[..at]);
    let tail = content[mark_end..].strip_prefix('\n').unwrap_or(&content[mark_end..]);
    if head.is_empty() && tail.is_empty() {
        return None;
    }
    let h = find_bytes(payload, head.as_bytes(), 0)?;
    let middle_start = h + head.len();
    let t = find_bytes(payload, tail.as_bytes(), middle_start)?;
    if t - middle_start != n {
        return None;
    }
    let middle = std::str::from_utf8(&payload[middle_start..t]).ok()?;
    Some(format!("{head}{middle}{tail}"))
}

fn find_bytes(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// [`splice_truncated`] against the conversation's own db: the content as
/// agy wrote it when the marker is there and the db row has the whole, the
/// logged content untouched otherwise. Read-only; a missing or locked db is
/// the untouched case, never an error.
pub(crate) fn recover_truncated(root: &Path, id: &str, step_index: u64, content: &str) -> String {
    if !content.contains(TRUNCATED_MARK) {
        return content.to_string();
    }
    let db = root.join("conversations").join(format!("{id}.db"));
    let recovered = (|| -> Option<String> {
        let conn = rusqlite::Connection::open_with_flags(
            &db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .ok()?;
        let payload: Vec<u8> = conn
            .query_row("SELECT step_payload FROM steps WHERE idx = ?1", [step_index as i64], |r| r.get(0))
            .ok()?;
        splice_truncated(content, &payload)
    })();
    recovered.unwrap_or_else(|| content.to_string())
}

/// Every `file://` URI in a blob of bytes, in order, without parsing the
/// protobuf around them. A URI ends at the first byte that cannot be in a
/// path: a control byte, a quote, whitespace, `#`, `)`, `]`. The DB's
/// `trajectory_metadata_blob` carries the workspace as `file:///home/x/proj`
/// — but so does the system prompt carry `file:///path/to/bar.py`, and a
/// protobuf tag byte after the real one can be printable (`…/nanoclawj`), so
/// callers filter for a directory that exists.
pub fn file_uris_in(bytes: &[u8]) -> Vec<String> {
    const NEEDLE: &[u8] = b"file:///";
    let mut out = Vec::new();
    let mut i = 0;
    while i + NEEDLE.len() <= bytes.len() {
        if &bytes[i..i + NEEDLE.len()] == NEEDLE {
            let start = i + NEEDLE.len() - 1;
            let mut end = start;
            while end < bytes.len() {
                let b = bytes[end];
                if b < 0x21 || b >= 0x7f || matches!(b, b'"' | b'#' | b')' | b']' | b'\'' | b'`') {
                    break;
                }
                end += 1;
            }
            if end > start + 1 {
                out.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// The workspace out of `conversations/<id>.db` (and its WAL, where a live
/// conversation's latest pages still sit): the first `file://` path that is a
/// directory on this machine and not the conversation's own `brain/` scratch.
fn cwd_from_db(root: &Path, id: &str) -> Option<String> {
    const CAP: u64 = 16 * 1024 * 1024;
    let base = root.join("conversations").join(format!("{id}.db"));
    for path in [base.clone(), PathBuf::from(format!("{}-wal", base.display()))] {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.len() > CAP {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let brain = root.join("brain");
        // A protobuf tag byte that happens to be printable rides on the
        // end of the real path (`…/harness-auditj`, seen live 09-02), so a
        // candidate is also tried with its last one or two bytes cut off.
        if let Some(cwd) = file_uris_in(&bytes).into_iter().find_map(|p| {
            (0..=2)
                .filter_map(|cut| p.get(..p.len().checked_sub(cut)?))
                .find(|c| c.len() > 1 && !Path::new(c).starts_with(&brain) && Path::new(c).is_dir())
                .map(str::to_owned)
        }) {
            return Some(cwd);
        }
    }
    None
}

/// Whether a transcript has at least one step. Print runs that failed before
/// speaking (no TTY, bad stdin) still create the conversation on disk with
/// an empty or absent transcript; those are not sessions.
fn has_a_step(transcript: &Path) -> bool {
    use std::io::{BufRead, BufReader};
    let Ok(f) = std::fs::File::open(transcript) else { return false };
    BufReader::new(f)
        .lines()
        .next()
        .and_then(|l| l.ok())
        .is_some_and(|l| !l.trim().is_empty())
}

/// One conversation id → its row. `None` when it has no transcript with a
/// step in it.
fn read_row(
    root: &Path,
    id: &str,
    last: &std::collections::HashMap<String, String>,
) -> Option<(Session, PathBuf)> {
    let transcript = transcript_path(root, id);
    if !transcript.is_file() || !has_a_step(&transcript) {
        return None;
    }
    let text = std::fs::read_to_string(&transcript).unwrap_or_default();
    let cwd = last
        .get(id)
        .cloned()
        .or_else(|| first_run_command_cwd(&text))
        .or_else(|| cwd_from_db(root, id))
        .unwrap_or_default();
    let title = std::fs::read_to_string(root.join("annotations").join(format!("{id}.pbtxt")))
        .ok()
        .and_then(|t| parse_title(&t))
        .or_else(|| first_request_line(&text))
        .or_else(|| {
            Path::new(&cwd)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| id.to_string());
    Some((
        Session {
            id: id.to_string(),
            // Stamped by the registry; see `scan_backends`.
            agent: String::new(),
            title,
            project_path: cwd.clone(),
            group_path: cwd,
            branch: None,
            forked: false,
            background: false,
            fork_parent: None,
            last_active: mtime_ms(&transcript),
        },
        transcript,
    ))
}

/// The body of [`AntigravitySessions::scan_with_paths`], over an explicit
/// root so it can be tested against a directory built for the purpose.
pub fn scan_dir(root: &Path) -> Vec<(Session, PathBuf)> {
    let mut budget = crate::sessions::DiscoveryBudget::new();
    scan_dir_bounded(root, &mut budget)
}

pub(crate) fn scan_dir_bounded(
    root: &Path,
    budget: &mut crate::sessions::DiscoveryBudget,
) -> Vec<(Session, PathBuf)> {
    let Ok(dbs) = std::fs::read_dir(root.join("conversations")) else {
        return vec![];
    };
    let last = std::fs::read_to_string(root.join("cache").join("last_conversations.json"))
        .map(|t| parse_last_conversations(&t))
        .unwrap_or_default();
    let mut out = Vec::new();
    for e in dbs.flatten() {
        if budget.remaining() == 0 {
            break;
        }
        let Ok(file_type) = e.file_type() else { continue };
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        let name = e.file_name();
        let name = name.to_string_lossy();
        // `<id>.db` only — the `-wal`/`-shm` siblings belong to the same id.
        let Some(id) = name.strip_suffix(".db") else { continue };
        if !valid_id(id) || !budget.claim_file() {
            continue;
        }
        if let Some(row) = read_row(root, id, &last) {
            out.push(row);
        }
    }
    out
}

/// The transcript for `session_id`, if this is an antigravity conversation
/// with something in it.
fn transcript_of(session_id: &str) -> Option<PathBuf> {
    if !valid_id(session_id) {
        return None;
    }
    let p = transcript_path(&store_root()?, session_id);
    (p.is_file() && has_a_step(&p)).then_some(p)
}

/// Files the session wrote, from `write_to_file` and `replace_file_content`
/// calls, newest touch first. Each step carries `created_at`, so `at` is the
/// step's time.
pub fn parse_artifacts(text: &str) -> Vec<crate::sessions::Artifact> {
    let mut order: Vec<String> = Vec::new();
    let mut info: std::collections::HashMap<String, (&'static str, String)> = Default::default();
    for line in text.lines() {
        if !line.contains("TargetFile") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let at = v.get("created_at").and_then(|c| c.as_str()).unwrap_or("").to_string();
        let Some(calls) = v.get("tool_calls").and_then(|t| t.as_array()) else { continue };
        for call in calls {
            let Some(tool) = call.get("name").and_then(|n| n.as_str()).and_then(write_tool_of) else {
                continue;
            };
            let Some(fp) = arg_str(call, "TargetFile").filter(|f| !f.is_empty()) else { continue };
            if let Some(pos) = order.iter().position(|p| p == &fp) {
                order.remove(pos);
            }
            info.insert(fp.clone(), (tool, at.clone()));
            order.push(fp);
        }
    }
    order
        .into_iter()
        .rev()
        .map(|path| {
            let (tool, at) = info.remove(&path).unwrap_or(("Write", String::new()));
            crate::sessions::Artifact { tool: tool.to_string(), path, at }
        })
        .collect()
}

/// The model's label out of the first turn's `<USER_SETTINGS_CHANGE>`:
/// "The user changed setting `Model Selection` from None to Gemini 3.8 Flash
/// (High). No need to comment…" — the only place a conversation records
/// which model it ran on. [observed: agy 1.1.24]
fn model_from_settings_change(content: &str) -> Option<String> {
    let i = content.find("`Model Selection` from ")?;
    let rest = &content[i..];
    let j = rest.find(" to ")? + " to ".len();
    let label = &rest[j..];
    let end = label.find(". ").or_else(|| label.find('\n')).unwrap_or(label.len());
    let label = label[..end].trim();
    (!label.is_empty()).then(|| label.to_string())
}

/// What the flyout shows for an antigravity conversation, from the
/// transcript and the annotation title. Every step carries `created_at`, so
/// started/last-active are real; the model is the label the first turn's
/// settings-change names. Token counts are only in print-mode output, never
/// on disk, so context stays unknown. [observed: agy 1.1.24]
pub fn parse_detail(id: &str, transcript: &str, title: Option<&str>, cwd: Option<&str>) -> crate::detail::SessionDetail {
    use crate::detail::{note_message, push_unique, top_tools, touch_file, SessionDetail};
    let mut d = SessionDetail { id: id.to_string(), ..Default::default() };
    d.title = title.map(String::from);
    d.cwd = cwd.filter(|c| !c.is_empty()).map(String::from);
    let mut tools: std::collections::HashMap<String, u32> = Default::default();
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if let Some(at) = v.get("created_at").and_then(|c| c.as_str()) {
            if d.started.is_none() {
                d.started = Some(at.to_string());
            }
            d.last_active = Some(at.to_string());
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("USER_INPUT") => {
                let Some(content) = v.get("content").and_then(|c| c.as_str()) else { continue };
                if let Some(m) = model_from_settings_change(content) {
                    push_unique(&mut d.models, &m);
                }
                let body = user_request(content);
                if !crate::sessions::is_only_system_block(&body) && !body.trim().is_empty() {
                    note_message(&mut d, "user", body.trim());
                }
            }
            Some("PLANNER_RESPONSE") => {
                for call in v.get("tool_calls").and_then(|t| t.as_array()).into_iter().flatten() {
                    d.tool_calls += 1;
                    let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                    *tools.entry(name.to_string()).or_insert(0) += 1;
                    if write_tool_of(name).is_some() {
                        if let Some(fp) = arg_str(call, "TargetFile") {
                            touch_file(&mut d.files, &fp);
                        }
                    }
                }
                if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
                    if !content.trim().is_empty() {
                        note_message(&mut d, "assistant", content.trim());
                    }
                }
            }
            _ => {}
        }
    }
    d.tools = top_tools(tools);
    d
}

impl SessionProvider for AntigravitySessions {
    fn scan_with_paths(&self) -> Vec<(Session, PathBuf)> {
        store_root().map(|r| scan_dir(&r)).unwrap_or_default()
    }

    fn scan_with_paths_bounded(
        &self,
        budget: &mut crate::sessions::DiscoveryBudget,
    ) -> Vec<(Session, PathBuf)> {
        store_root()
            .map(|root| scan_dir_bounded(&root, budget))
            .unwrap_or_default()
    }

    fn find_session_file(&self, session_id: &str) -> Option<PathBuf> {
        transcript_of(session_id)
    }

    /// Answered here: the file is jsonl, but nothing like claude's, and the
    /// default reader would make nothing of it.
    fn messages(&self, session_id: &str) -> Option<Vec<(String, String)>> {
        let text = std::fs::read_to_string(transcript_of(session_id)?).ok()?;
        let root = store_root()?;
        Some(parse_messages_with(&text, &|index, content| recover_truncated(&root, session_id, index, content)))
    }

    fn detail(&self, session_id: &str) -> Option<crate::detail::SessionDetail> {
        let root = store_root()?;
        let text = std::fs::read_to_string(transcript_of(session_id)?).ok()?;
        let title = std::fs::read_to_string(root.join("annotations").join(format!("{session_id}.pbtxt")))
            .ok()
            .and_then(|t| parse_title(&t));
        let last = std::fs::read_to_string(root.join("cache").join("last_conversations.json"))
            .map(|t| parse_last_conversations(&t))
            .unwrap_or_default();
        let cwd = last
            .get(session_id)
            .cloned()
            .or_else(|| first_run_command_cwd(&text))
            .or_else(|| cwd_from_db(&root, session_id));
        Some(parse_detail(session_id, &text, title.as_deref(), cwd.as_deref()))
    }

    fn artifacts(&self, session_id: &str) -> Option<Vec<crate::sessions::Artifact>> {
        let text = std::fs::read_to_string(transcript_of(session_id)?).ok()?;
        Some(parse_artifacts(&text))
    }
}

// ---------------------------------------------------------------------------
// running agy for answers
// ---------------------------------------------------------------------------

/// Run a command with its stdin and stderr closed and give back
/// `(exit code, stdout)`, or `Err` when it did not finish in `timeout`. Every
/// agy process starts a language-server subprocess and takes a second or
/// two even for `models` or `/usage`, so nothing that calls it may wait
/// without a cap. On timeout the process is killed; the reader thread is
/// left to end when the pipe closes.
pub(crate) fn run_capped(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> Result<(i32, String), String> {
    use std::io::Read;
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().unwrap_or_default();
                return Ok((status.code().unwrap_or(-1), String::from_utf8_lossy(&out).into_owned()));
            }
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The `agy` binary: PATH first, then the login shell's PATH — a GUI app's
/// environment rarely has `~/.local/bin`, which is where the installer puts it.
pub(crate) fn agy_bin() -> Option<PathBuf> {
    crate::agents::which("agy").or_else(|| crate::agents::which_via_login_shell("agy"))
}

/// Parse `agy models` — TSV `slug<TAB>label`, after a "Fetching available
/// models..." line — with the default first. No efforts: the slugs carry
/// the effort themselves (`gemini-3.8-flash-high`) and `--effort` is a
/// session-wide flag agy documents without a per-model list, so none is
/// invented. [observed: agy 1.1.24; `--output-format json` is refused]
pub fn parse_models(text: &str) -> Vec<ModelOption> {
    let mut out: Vec<ModelOption> = text
        .lines()
        .filter_map(|line| {
            let (slug, label) = line.split_once('\t')?;
            let slug = slug.trim();
            if slug.is_empty() || slug.contains(' ') {
                return None;
            }
            Some(ModelOption {
                id: slug.to_string(),
                display_name: label.trim().to_string(),
                efforts: vec![],
                default_effort: None,
            })
        })
        .collect();
    if let Some(pos) = out.iter().position(|m| m.id == DEFAULT_MODEL) {
        let d = out.remove(pos);
        out.insert(0, d);
    }
    out
}

static MODELS_CACHE: std::sync::Mutex<Option<(std::time::Instant, Vec<ModelOption>)>> =
    std::sync::Mutex::new(None);

/// `agy models`, cached for an hour — it is a network call behind a full
/// process start. A failure is cached for five minutes so a missing or
/// signed-out agy does not cost three seconds per open of the ＋ menu.
fn models_cached() -> Vec<ModelOption> {
    let mut cache = MODELS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, list)) = cache.as_ref() {
        let ttl = if list.is_empty() { 300 } else { 3600 };
        if at.elapsed().as_secs() < ttl {
            return list.clone();
        }
    }
    let list = agy_bin()
        .and_then(|bin| {
            let mut cmd = std::process::Command::new(bin);
            cmd.arg("models");
            if let Some(home) = dirs::home_dir() {
                cmd.current_dir(home);
            }
            run_capped(cmd, std::time::Duration::from_secs(3)).ok()
        })
        .filter(|(code, _)| *code == 0)
        .map(|(_, out)| parse_models(&out))
        .unwrap_or_default();
    *cache = Some((std::time::Instant::now(), list.clone()));
    list
}

impl AgentBackend for AntigravityBackend {
    fn id(&self) -> &'static str {
        "antigravity"
    }
    fn display_name(&self) -> &'static str {
        "Antigravity"
    }
    fn detect(&self) -> Detection {
        Detection { caps: self.caps(), ..detect_cli(self.id(), self.display_name(), "agy") }
    }
    fn sessions(&self) -> &dyn SessionProvider {
        &AntigravitySessions
    }

    /// agy mints its own conversation id and prints it only on exit — see
    /// the module doc. Adoption re-keys the tab.
    fn mints_session_id(&self) -> bool {
        false
    }

    /// Resume only. No clear (it would be a plain `agy`), no delete (a
    /// conversation is four files), no tasks: `manage_task` is in agy's tool
    /// list but no session here ever called it, so its shape is unread and
    /// the Tasks tab would be a guess. No TUI driving, no transcript panels:
    /// both read claude's shapes.
    fn caps(&self) -> Caps {
        Caps { resume: true, ..Default::default() }
    }

    fn permission_modes(&self) -> &'static [PermissionMode] {
        ANTIGRAVITY_PERMISSION_MODES
    }

    /// `agy --conversation <id>` — works from any directory (verified: a
    /// resume from a different cwd continued the same conversation).
    fn resume(&self, session_id: &str) -> Option<String> {
        Some(format!("agy --conversation {}", q(session_id)))
    }

    fn models(&self) -> Vec<ModelOption> {
        models_cached()
    }

    /// `-i` starts the TUI with a first prompt and stays open; `--model` and
    /// `--effort` straight from `agy --help` 1.1.24. Permissions are the
    /// resolver's, and a session id is not something agy will take.
    fn launch(&self, spec: &LaunchSpec) -> String {
        // agy records no workspace for a plain launch — `WorkspaceURIs: null`,
        // and a conversation that never runs a command has no `Cwd` in its
        // transcript either — so its row had no folder, and adoption (which
        // matches on the folder) never bound the tab: every session-bound
        // action from the phone — bring-in first of all — then failed with
        // "open the session on the desktop first". `--add-dir` puts the
        // launch directory in the conversation's trajectory metadata as a
        // `file://` URI, which `cwd_from_db` reads. The shell expands `$PWD`:
        // the command runs through `$SHELL -i -c` in the tab's directory.
        // [observed: agy 1.1.24, 2026-09-02]
        let mut cmd = String::from("agy --add-dir \"$PWD\"");
        if let Some(p) = prompt_of(spec) {
            cmd.push_str(&format!(" -i {}", q(p)));
        }
        if let Some(m) = spec.model.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --model {}", q(m)));
        }
        if let Some(e) = spec.effort.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --effort {}", q(e)));
        }
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_truncated_answer_is_made_whole_from_its_blob() {
        let full = "Spider monkeys keep mental maps of hundreds of individual canopy trees across their range, and bark at predators (jaguars, humans) or rival males.";
        let head = "Spider monkeys keep mental maps of hundreds of ";
        let tail = "ors (jaguars, humans) or rival males.";
        let cut = full.len() - head.len() - tail.len();
        let logged = format!("{head}\n<truncated {cut} bytes>\n{tail}");
        // Protobuf around the string: any bytes, the string contiguous.
        let mut payload = vec![0x0a, 0x91, 0x01];
        payload.extend_from_slice(full.as_bytes());
        payload.extend_from_slice(&[0x12, 0x02, 0x08, 0x01]);
        assert_eq!(splice_truncated(&logged, &payload).as_deref(), Some(full));
        // The marker promises a size; a blob whose gap is another size is
        // not this step, and the logged text stands.
        let wrong = format!("{head}\n<truncated {} bytes>\n{tail}", cut + 1);
        assert_eq!(splice_truncated(&wrong, &payload), None);
        // No marker, nothing to do.
        assert_eq!(splice_truncated(full, &payload), None);
        // parse_messages_with hands the step to the recoverer.
        let line = format!(
            "{{\"step_index\":3,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"content\":{},\"truncated_fields\":[\"content\"]}}",
            serde_json::to_string(&logged).unwrap()
        );
        let msgs = parse_messages_with(&line, &|index, c| {
            assert_eq!(index, 3);
            splice_truncated(c, &payload).unwrap_or_else(|| c.to_string())
        });
        assert_eq!(msgs, vec![("assistant".to_string(), full.to_string())]);
    }

    // Every record below is verbatim from the real store, agy 1.1.24,
    // 2026-09-02 (tool outputs shortened, nothing reshaped).

    const FIRST_INPUT: &str = r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-09-02T15:55:55Z","content":"<USER_REQUEST>\ntake a look at the acc medlink google ads account so far this week. i wnt to make sure everything is running smoothly\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-09-02T11:55:55-04:00.\n</ADDITIONAL_METADATA>\n<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection` from None to Gemini 3.8 Flash (High). No need to comment on this change if the user doesn't ask about it. If reporting what model you are, please use a human readable name instead of the exact string.\n</USER_SETTINGS_CHANGE>"}"#;
    const TOOL_STEP: &str = r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-09-02T15:55:55Z","thinking":"**Assessing Google Ads Performance**\n\nDetermining the appropriate date range is essential.\n\n\n","tool_calls":[{"name":"run_command","args":{"CommandLine":"\"~/nanoclaw/projects/memory-index/mem-search \\\"google ads account report status\\\"\"","Cwd":"\"/home/john/nanoclaw\"","RequestedTerminalID":"\"\"","RunPersistent":"false","WaitMsBeforeAsync":"5000","toolAction":"\"Searching memory index\"","toolSummary":"\"Search memory for Google Ads tools\""}}]}"#;
    const RESULT_STEP: &str = r#"{"step_index":2,"source":"MODEL","type":"GENERIC","status":"DONE","created_at":"2026-09-02T15:55:58Z","content":"Created At: 2026-09-02T11:55:58-04:00\nCompleted At: 2026-09-02T11:56:15-04:00\n\nThe command exited with code 0.\nOutput:\n\r\n0.515  reference_microsoft_ads\r\n","truncated_fields":["content"]}"#;
    const SYSTEM_STEP: &str = r#"{"step_index":3,"source":"SYSTEM","type":"SYSTEM_MESSAGE","status":"DONE","created_at":"2026-09-02T16:06:37Z","content":"The following is a <SYSTEM_MESSAGE> not actually sent by the user. It is provided by the system as important information to pay attention to.\n\n<SYSTEM_MESSAGE>\n[Message] timestamp=2026-09-02T16:06:37Z sender=system priority=MESSAGE_PRIORITY_LOW content=[Notice] All your subagents and background tasks have been stopped due to server restart. If you want a subagent to continue working, it needs to be revived by sending it a new message. If resuming work, please check on status and restart as needed.\n</SYSTEM_MESSAGE>"}"#;
    const ANSWER_STEP: &str = r#"{"step_index":4,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-09-02T16:06:37Z","content":"pong3"}"#;
    const SECOND_INPUT: &str = r#"{"step_index":2,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-09-02T16:06:37Z","content":"<USER_REQUEST>\nReply with exactly: pong3\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-09-02T12:06:37-04:00.\n</ADDITIONAL_METADATA>"}"#;
    const WRITE_STEP: &str = r##"{"step_index": 45, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-09-02T16:02:31Z", "tool_calls": [{"name": "write_to_file", "args": {"ArtifactMetadata": "{\"RequestFeedback\":false,\"Summary\":\"Python script to pull comprehensive Google Ads performance data.\",\"UserFacing\":false}", "CodeContent": "\"#!/usr/bin/env python3\\nimport os, sys,", "Description": "\"Script to pull Google Ads metrics for Mon/Tue last week vs this week\"", "Overwrite": "true", "TargetFile": "\"/home/john/nanoclaw/projects/google_ads/montue_compare_20260902/pull_data.py\"", "toolAction": "\"Writing pull script\"", "toolSummary": "\"Create pull_data.py\""}}], "truncated_fields": ["tool_calls"]}"##;
    const EDIT_STEP: &str = r#"{"step_index": 51, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-09-02T16:02:57Z", "tool_calls": [{"name": "replace_file_content", "args": {"AllowMultiple": "false", "Description": "\"Fix GAQL date operator to BETWEEN\"", "EndLine": "25", "Instruction": "\"Change date filter to BETWEEN '2026-08-24' AND '2026-09-01'\"", "ReplacementContent": "\"DATES = [\\\"2026-08-24\\\", \\\"20", "StartLine": "18", "TargetContent": "\"DATES = [\\\"2026-08-24\\\", \\\"20", "TargetFile": "\"/home/john/.gemini/antigravity-cli/brain/1e6213a2-6da8-46d3-a2e6-624bc72ff061/scratch/pull_data.py\"", "toolAction": "\"Fixing date filter\"", "toolSummary": "\"Change date filter in scratch/pull_data.py\""}}]}"#;

    #[test]
    fn the_title_is_read_off_the_annotation() {
        assert_eq!(parse_title(r#"title:"Google Ads Performance Review""#).as_deref(), Some("Google Ads Performance Review"));
        assert_eq!(parse_title(r#"title:"Say \"hi\" \\ twice""#).as_deref(), Some(r#"Say "hi" \ twice"#));
        assert_eq!(parse_title(r#"title:"""#), None);
        assert_eq!(parse_title("other:1"), None);
    }

    #[test]
    fn last_conversations_is_inverted() {
        let m = parse_last_conversations(r#"{
  "/home/john/nanoclaw": "4e27641c-e5e2-47c7-9086-2c1f75ccd490",
  "/tmp/probe": "5ad73b75-8047-4a40-8f58-0a770f114c1a"
}"#);
        assert_eq!(m["4e27641c-e5e2-47c7-9086-2c1f75ccd490"], "/home/john/nanoclaw");
        assert_eq!(m.len(), 2);
        assert!(parse_last_conversations("nope").is_empty());
    }

    #[test]
    fn the_preview_is_the_person_and_the_prose() {
        let log = [FIRST_INPUT, TOOL_STEP, RESULT_STEP, SECOND_INPUT, SYSTEM_STEP, ANSWER_STEP].join("\n");
        let msgs = parse_messages(&log);
        assert_eq!(
            msgs,
            vec![
                ("user".to_string(), "take a look at the acc medlink google ads account so far this week. i wnt to make sure everything is running smoothly".to_string()),
                ("assistant".to_string(), "Search memory for Google Ads tools".to_string()),
                ("user".to_string(), "Reply with exactly: pong3".to_string()),
                ("assistant".to_string(), "pong3".to_string()),
            ],
            "no metadata, no settings-change, no thinking, no tool output, no server-restart notice"
        );
    }

    #[test]
    fn a_tool_only_response_falls_back_to_the_tool_name() {
        let line = r#"{"step_index":9,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-09-02T16:01:40Z","tool_calls":[{"name":"list_dir","args":{"DirectoryPath":"\"/tmp\""}},{"name":"view_file","args":{"AbsolutePath":"\"/tmp/a\"","toolSummary":"\"View a\""}}]}"#;
        assert_eq!(parse_messages(line), vec![("assistant".to_string(), "list_dir\nView a".to_string())]);
    }

    #[test]
    fn a_settings_only_input_is_not_a_message() {
        let line = r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-09-02T16:00:00Z","content":"<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection` from None to Gemini 3.8 Flash (High).\n</USER_SETTINGS_CHANGE>"}"#;
        assert!(parse_messages(line).is_empty());
    }

    #[test]
    fn the_first_request_and_the_first_cwd_are_found() {
        let log = [FIRST_INPUT, TOOL_STEP].join("\n");
        assert_eq!(first_request_line(&log).as_deref(), Some("take a look at the acc medlink google ads account so far this week. i wnt to make sure everything is running smoothly"));
        assert_eq!(first_run_command_cwd(&log).as_deref(), Some("/home/john/nanoclaw"));
        assert_eq!(first_run_command_cwd(FIRST_INPUT), None);
    }

    #[test]
    fn file_uris_come_out_of_protobuf_bytes_in_order() {
        // The DB's metadata blob followed by a printable tag byte, then a
        // prompt-text example — as `grep -a -o 'file://…'` over a real .db.
        let bytes = b"\x0a\x1cfile:///home/john/nanoclaw\x12\x05x file:///home/john/nanoclawj\"\x00file:///path/to/bar.py#L10 (file:///absolute/path/to/file)";
        assert_eq!(
            file_uris_in(bytes),
            vec!["/home/john/nanoclaw", "/home/john/nanoclawj", "/path/to/bar.py", "/absolute/path/to/file"]
        );
        assert!(file_uris_in(b"file:///").is_empty());
    }

    #[test]
    fn a_store_lists_its_spoken_conversations_only() {
        let root = std::env::temp_dir().join(format!("aiterm-agy-{}", uuid::Uuid::new_v4()));
        let cwd = root.join("work").join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        for d in ["conversations", "annotations", "cache"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let write = |id: &str, transcript: Option<&str>, title: Option<&str>| {
            std::fs::write(root.join("conversations").join(format!("{id}.db")), b"SQLite format 3\0").unwrap();
            std::fs::write(root.join("conversations").join(format!("{id}.db-wal")), b"").unwrap();
            if let Some(t) = transcript {
                let p = transcript_path(&root, id);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, t).unwrap();
            }
            if let Some(t) = title {
                std::fs::write(root.join("annotations").join(format!("{id}.pbtxt")), format!("title:\"{t}\"")).unwrap();
            }
        };
        // Titled, cwd from the cache.
        write("4e27641c-e5e2-47c7-9086-2c1f75ccd490", Some(&[FIRST_INPUT, TOOL_STEP].join("\n")), Some("Google Ads Performance Review"));
        // Untitled, cwd from its first run_command.
        write("8733080f-ff82-4f52-a73a-094777650e1c", Some(&[FIRST_INPUT, TOOL_STEP].join("\n")), None);
        // Untitled, no tool call: cwd from the DB's file:// URI, title from the prompt.
        write("58cd57d4-e8e3-4c76-9eed-f2359e2d018d", Some(&[SECOND_INPUT, ANSWER_STEP].join("\n")), None);
        std::fs::write(
            root.join("conversations").join("58cd57d4-e8e3-4c76-9eed-f2359e2d018d.db"),
            format!("\x0a\x20file:///path/to/bar.py\x00\x0a\x20file://{}\x12", cwd.display()).as_bytes(),
        )
        .unwrap();
        // A failed probe: conversation on disk, transcript empty.
        write("2108b7b5-c769-4b54-be7a-4db3f78a95f2", Some(""), None);
        // Another: no transcript at all.
        write("36fd0982-4254-4045-81c2-32124c0ae32e", None, None);
        std::fs::write(
            root.join("cache").join("last_conversations.json"),
            format!(r#"{{"{}": "4e27641c-e5e2-47c7-9086-2c1f75ccd490"}}"#, cwd.display()),
        )
        .unwrap();

        let mut rows = scan_dir(&root);
        rows.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        assert_eq!(rows.len(), 3, "zero-step conversations are not sessions");
        assert_eq!(rows[0].0.id, "4e27641c-e5e2-47c7-9086-2c1f75ccd490");
        assert_eq!(rows[0].0.title, "Google Ads Performance Review");
        assert_eq!(rows[0].0.project_path, cwd.to_string_lossy());
        assert!(rows[0].1.ends_with(".system_generated/logs/transcript.jsonl"));
        assert_eq!(rows[1].0.id, "58cd57d4-e8e3-4c76-9eed-f2359e2d018d");
        assert_eq!(rows[1].0.title, "Reply with exactly: pong3");
        assert_eq!(rows[1].0.project_path, cwd.to_string_lossy(), "the first file:// that is a real directory");
        assert_eq!(rows[2].0.title, "take a look at the acc medlink google ads account so far this week. i wnt to make sure everything is running smoothly");
        assert_eq!(rows[2].0.project_path, "/home/john/nanoclaw");
        assert!(rows[2].0.last_active > 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn artifacts_read_newest_first_with_the_step_time() {
        let log = [WRITE_STEP, EDIT_STEP, TOOL_STEP].join("\n");
        let arts = parse_artifacts(&log);
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].path, "/home/john/.gemini/antigravity-cli/brain/1e6213a2-6da8-46d3-a2e6-624bc72ff061/scratch/pull_data.py");
        assert_eq!(arts[0].tool, "Edit");
        assert_eq!(arts[0].at, "2026-09-02T16:02:57Z");
        assert_eq!(arts[1].path, "/home/john/nanoclaw/projects/google_ads/montue_compare_20260902/pull_data.py");
        assert_eq!(arts[1].tool, "Write");
    }

    #[test]
    fn the_flyout_gets_the_lot() {
        let log = [FIRST_INPUT, TOOL_STEP, RESULT_STEP, WRITE_STEP, SECOND_INPUT, SYSTEM_STEP, ANSWER_STEP].join("\n");
        let d = parse_detail("4e27", &log, Some("Google Ads Performance Review"), Some("/home/john/nanoclaw"));
        assert_eq!(d.started.as_deref(), Some("2026-09-02T15:55:55Z"));
        assert_eq!(d.last_active.as_deref(), Some("2026-09-02T16:06:37Z"));
        assert_eq!(d.title.as_deref(), Some("Google Ads Performance Review"));
        assert_eq!(d.cwd.as_deref(), Some("/home/john/nanoclaw"));
        assert_eq!(d.models, vec!["Gemini 3.8 Flash (High)"], "the label the settings-change names");
        assert_eq!((d.user_messages, d.assistant_messages), (2, 1));
        assert_eq!(d.tool_calls, 2);
        assert_eq!(d.tools[0].name, "run_command");
        assert_eq!(d.files, vec!["/home/john/nanoclaw/projects/google_ads/montue_compare_20260902/pull_data.py"]);
        assert!(d.first_prompt.as_deref().unwrap().starts_with("take a look at the acc medlink"));
        assert_eq!(d.last_assistant.as_deref(), Some("pong3"));
        assert_eq!(d.context_tokens, None, "tokens are only in print-mode output; nothing is invented");
    }

    /// `agy models` 1.1.24, verbatim.
    #[test]
    fn models_are_the_tsv_with_the_default_first() {
        let out = "Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\ngemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)\ngemini-3.1-pro-high\tGemini 3.1 Pro (High)\nclaude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\ngpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n";
        let m = parse_models(out);
        assert_eq!(m.len(), 5);
        assert_eq!(m[0].id, DEFAULT_MODEL);
        assert_eq!(m[0].display_name, "Gemini 3.8 Flash (High)");
        assert!(m[0].efforts.is_empty());
        assert_eq!(m[3].id, "claude-sonnet-4-6");
        assert!(parse_models("Fetching available models...\n").is_empty());
    }

    #[test]
    fn launch_spells_the_flags_agy_documents() {
        assert_eq!(AntigravityBackend.launch(&LaunchSpec::default()), "agy --add-dir \"$PWD\"");
        let cmd = AntigravityBackend.launch(&LaunchSpec {
            model: Some("gemini-3.1-pro-high".into()),
            effort: Some("low".into()),
            session_id: Some("ignored".into()),
            provider: None,
            prompt: Some("  say it's done  ".into()),
        });
        assert_eq!(cmd, "agy --add-dir \"$PWD\" -i 'say it'\\''s done' --model 'gemini-3.1-pro-high' --effort 'low'");
        assert_eq!(
            AntigravityBackend.resume("8733080f-ff82-4f52-a73a-094777650e1c").unwrap(),
            "agy --conversation '8733080f-ff82-4f52-a73a-094777650e1c'"
        );
        assert_eq!(AntigravityBackend.clear("x"), None);
        assert!(!AntigravityBackend.mints_session_id());
        assert!(AntigravityBackend.caps().resume);
        assert!(!AntigravityBackend.caps().tasks);
        assert!(!AntigravityBackend.caps().delete);
    }

    #[test]
    fn permission_ids_are_agys_own_words() {
        let ids: Vec<&str> = ANTIGRAVITY_PERMISSION_MODES.iter().map(|m| m.id).collect();
        assert_eq!(ids, ["default", "plan", "accept-edits", "dangerously-skip-permissions"]);
        assert_eq!(ANTIGRAVITY_PERMISSION_MODES[0].flags, &[] as &[&str]);
        assert_eq!(ANTIGRAVITY_PERMISSION_MODES[3].flags, &["--dangerously-skip-permissions"]);
    }

    /// Reads the real store on this machine and prints what it finds; it
    /// only asserts when a store exists.
    /// `cargo test --lib antigravity::tests::live_cwd_of_every_row -- --ignored --nocapture`
    /// prints id → cwd for the real store; empty cwds are the ones to chase.
    #[test]
    #[ignore]
    fn live_cwd_of_every_row() {
        let Some(root) = store_root() else { println!("agy store absent"); return };
        for (s, _) in AntigravitySessions.scan_with_paths() {
            println!("{}  {:40}  {}", &s.id[..8], s.title, if s.project_path.is_empty() { "<none>" } else { &s.project_path });
        }
        let _ = root;
    }

    #[test]
    fn live_store_scan() {
        let Some(root) = store_root() else {
            println!("no antigravity store here");
            return;
        };
        let rows = scan_dir(&root);
        let newest = rows.iter().max_by_key(|r| r.0.last_active);
        println!(
            "antigravity live scan: {} rows; newest = {:?} ({:?}, cwd {:?})",
            rows.len(),
            newest.map(|r| r.0.title.as_str()),
            newest.map(|r| r.0.id.as_str()),
            newest.map(|r| r.0.project_path.as_str()),
        );
        for r in &rows {
            assert!(!r.0.title.is_empty(), "{} has no title", r.0.id);
            assert!(r.1.is_file());
        }
    }
}
