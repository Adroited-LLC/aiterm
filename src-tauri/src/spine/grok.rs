//! Grok adapter: `~/.grok/sessions/<cwd>/<id>/updates.jsonl` (an on-disk ACP
//! `session/update` stream) plus `events.jsonl` for turns and permissions.
//!
//! Owned by the grok-adapter task. See `docs/spine.md`.
//!
//! `updates.jsonl` is one JSON-RPC notification per line, appended as the
//! turn runs. The two methods that appear are `session/update` (plain ACP)
//! and `_x.ai/session/update` (xAI's extensions), both carrying
//! `params.update.sessionUpdate` as the discriminator:
//!
//! ```text
//! {"timestamp":1787956220,                       // unix SECONDS
//!  "method":"session/update",
//!  "params":{"sessionId":"…",
//!            "update":{"sessionUpdate":"agent_message_chunk",
//!                      "content":{"type":"text","text":"…"}},
//!            "_meta":{"eventId":"<session>-<n>","agentTimestampMs":1787956221679,…}}}
//! ```
//!
//! `events.jsonl` beside it is a different shape entirely — flat objects
//! keyed by `type`, with an ISO-8601 `ts`, no `params` wrapper:
//!
//! ```text
//! {"ts":"2026-08-28T22:30:22.790Z","type":"tool_completed",
//!  "tool_name":"read_file","duration_ms":0,"outcome":"success","tool_call_id":"call-…-0"}
//! ```
//!
//! [observed: grok 1.0.13, 2026-09-02, 22 sessions under ~/.grok/sessions]

use super::{clip, now_ms, Adapter, Kind, Phase, ToolCategory, ToolStatus};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Seek, SeekFrom};
use std::path::PathBuf;

/// How much of a tool's input a card shows, and how much of its output.
const INPUT_MAX: usize = 400;
const OUTPUT_MAX: usize = 2000;

pub struct GrokAdapter {
    dir: PathBuf,
    updates: Tail,
    events: Tail,
    /// 1-based line number of the next `updates.jsonl` line — the ordinal
    /// that ids and turn names are built from. Stable across re-reads
    /// because the file is append-only.
    ordinal: u64,
    /// The prose or reasoning block being accumulated, if one is open.
    run: Option<Run>,
    /// The turn a `turn_completed` closes: the ordinal of the user message
    /// that opened it. "0" before any user message has been seen.
    turn: String,
    /// Tool ids `updates.jsonl` called `completed` in this poll and the one
    /// before it, so `events.jsonl` can correct one to `failed`. Two polls
    /// is plenty — the two lines are written in the same millisecond — and
    /// dropping the rest keeps a long session's bookkeeping bounded.
    completed: [HashSet<String>; 2],
}

/// A run of consecutive same-kind chunks, folded into one block. Grok emits
/// one chunk per completed block rather than per token, and at 1.0.13 every
/// observed run was length 1 (a tool call always separates two blocks) —
/// but the ACP stream permits a run, so we fold.
struct Run {
    thought: bool,
    id: String,
    text: String,
    ts: u64,
}

/// The adapter for a Grok session, or `None` when no session dir exists.
///
/// `updates.jsonl` need not exist yet: a session that has not finished a
/// turn has only `summary.json`, and the file appears under us.
pub fn open(session_id: &str) -> Option<GrokAdapter> {
    let dir = crate::grok::session_dir(session_id)?;
    Some(GrokAdapter {
        updates: Tail::new(dir.join("updates.jsonl")),
        events: Tail::new(dir.join("events.jsonl")),
        dir,
        ordinal: 1,
        run: None,
        turn: "0".to_string(),
        completed: Default::default(),
    })
}

impl Adapter for GrokAdapter {
    fn bootstrap(&mut self) -> Vec<(u64, Kind)> {
        self.poll()
    }

    fn poll(&mut self) -> Vec<(u64, Kind)> {
        let (Some(update_lines), Some(event_lines)) = (self.updates.take(), self.events.take())
        else {
            return self.restart();
        };
        self.merge(update_lines, event_lines)
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        // The directory too: the first turn CREATES updates.jsonl, and a
        // watch on a path that does not exist yet never fires.
        vec![self.updates.path.clone(), self.events.path.clone(), self.dir.clone()]
    }
}

impl GrokAdapter {
    /// A file was truncated or replaced under us: drop everything we hold
    /// and rebuild from both files, behind a `Reset`.
    fn restart(&mut self) -> Vec<(u64, Kind)> {
        self.updates.rewind();
        self.events.rewind();
        self.ordinal = 1;
        self.run = None;
        self.turn = "0".to_string();
        self.completed = Default::default();
        let mut out = vec![(now_ms(), Kind::Reset)];
        let updates = self.updates.take().unwrap_or_default();
        let events = self.events.take().unwrap_or_default();
        out.extend(self.merge(updates, events));
        out
    }

    /// Both files parsed and interleaved by timestamp. `updates.jsonl` is
    /// parsed first so the tool statuses it reports are known before
    /// `events.jsonl` gets a chance to correct one.
    fn merge(&mut self, updates: Vec<String>, events: Vec<String>) -> Vec<(u64, Kind)> {
        let mut from_updates = self.parse_updates(&updates);
        let mut from_events = self.parse_events(&events);
        monotonic(&mut from_updates);
        monotonic(&mut from_events);
        let mut all: Vec<(u64, u8, Kind)> = from_updates
            .into_iter()
            .map(|(ts, k)| (ts, 0, k))
            .chain(from_events.into_iter().map(|(ts, k)| (ts, 1, k)))
            .collect();
        // Stable, so each file keeps its own order; the tag puts updates
        // first on a tie, which is how a `tool_completed` error (same ms as
        // the `completed` it corrects) lands after the status it fixes.
        all.sort_by_key(|(ts, src, _)| (*ts, *src));
        self.completed.swap(0, 1);
        self.completed[0].clear();
        all.into_iter().map(|(ts, _, k)| (ts, k)).collect()
    }

    fn parse_updates(&mut self, lines: &[String]) -> Vec<(u64, Kind)> {
        let mut out = Vec::new();
        for line in lines {
            let ordinal = self.ordinal;
            self.ordinal += 1;
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let params = &v["params"];
            let update = &params["update"];
            let Some(sort) = update["sessionUpdate"].as_str() else { continue };
            let ts = ts_ms(&v, params);
            match sort {
                "user_message_chunk" => {
                    // Grok injects background-task notices as user chunks it
                    // hides from its own scrollback; they are the engine
                    // talking to itself, not a person opening a turn.
                    if update["_meta"]["hideFromScrollback"].is_null() {
                        self.user_chunk(ordinal, ts, chunk_text(&update["content"]), &mut out);
                    }
                }
                "agent_message_chunk" => {
                    self.agent_chunk(false, ordinal, ts, chunk_text(&update["content"]), &mut out)
                }
                "agent_thought_chunk" => {
                    self.agent_chunk(true, ordinal, ts, chunk_text(&update["content"]), &mut out)
                }
                "tool_call" => {
                    if let Some(k) = tool_card(update, ToolStatus::Pending) {
                        self.close_run(&mut out);
                        out.push((ts, k));
                    }
                }
                "tool_call_update" => {
                    self.close_run(&mut out);
                    self.tool_update(update, ts, &mut out);
                }
                // xAI's end-of-turn: the usage block is not ours to carry.
                "turn_completed" => {
                    self.close_run(&mut out);
                    let reason = match update["stop_reason"].as_str() {
                        Some("end_turn") => "completed",
                        Some("cancelled") => "interrupted",
                        Some("error") => "error",
                        _ => "unknown",
                    };
                    out.push((ts, Kind::TurnEnded { turn: self.turn.clone(), reason: reason.into() }));
                }
                // `plan`, `task_backgrounded`, `task_completed`,
                // `subagent_spawned`, `subagent_finished`, `session_recap`,
                // `current_mode_update`, `image_compressed`: state the phone
                // does not render, and none of them means "waiting for you".
                _ => {}
            }
        }
        out
    }

    /// A person spoke: a new turn, then their words. Emitted on the first
    /// chunk rather than held until the run closes, so the phone echoes what
    /// was just typed instead of waiting for the model's first block; a
    /// second consecutive chunk re-emits the same id with the text grown.
    fn user_chunk(&mut self, ordinal: u64, ts: u64, text: String, out: &mut Vec<(u64, Kind)>) {
        if let Some(run) = self.run.as_mut().filter(|r| r.id.starts_with('u')) {
            run.text.push_str(&text);
            out.push((ts, Kind::UserMessage { id: run.id.clone(), text: run.text.clone() }));
            return;
        }
        self.close_run(out);
        self.turn = ordinal.to_string();
        let id = format!("u{ordinal}");
        out.push((ts, Kind::TurnStarted { turn: self.turn.clone() }));
        out.push((ts, Kind::UserMessage { id: id.clone(), text: text.clone() }));
        self.run = Some(Run { thought: false, id, text, ts });
    }

    fn agent_chunk(
        &mut self,
        thought: bool,
        ordinal: u64,
        ts: u64,
        text: String,
        out: &mut Vec<(u64, Kind)>,
    ) {
        match self.run.as_mut() {
            Some(run) if run.thought == thought && !run.id.starts_with('u') => {
                run.text.push_str(&text);
                run.ts = ts;
            }
            _ => {
                self.close_run(out);
                self.run = Some(Run { thought, id: format!("a{ordinal}"), text, ts });
            }
        }
        let Some(run) = self.run.as_ref() else { return };
        out.push((ts, block(run, false)));
    }

    /// Close the open block, if any: one last snapshot with `done`. Stamped
    /// with the block's own last timestamp, not the line that ended it, so
    /// it sorts before whatever comes next.
    fn close_run(&mut self, out: &mut Vec<(u64, Kind)>) {
        let Some(run) = self.run.take() else { return };
        if run.id.starts_with('u') {
            // A user message is complete when it is emitted; there is no
            // `done` on the kind and re-sending it would only repeat text.
            return;
        }
        out.push((run.ts, block(&run, true)));
    }

    /// A `tool_call_update` is two different lines wearing one name. With no
    /// `status` it is the call being filled in as it starts — the pretty
    /// title, the ACP kind, the real input — which the spine has no kind for,
    /// so it re-issues the card (upsert by id) with everything known. With a
    /// `status` it is the terminal result. [observed: grok 1.0.13]
    fn tool_update(&mut self, update: &Value, ts: u64, out: &mut Vec<(u64, Kind)>) {
        let Some(id) = update["toolCallId"].as_str() else { return };
        let status = update["status"].as_str().map(tool_status);
        if status.is_none() {
            if let Some(k) = tool_card(update, ToolStatus::Running) {
                out.push((ts, k));
            }
        }
        let status = status.unwrap_or(ToolStatus::Running);
        let output = tool_output(&update["content"]);
        // The start-of-run line only earns a second event when it brought
        // something to show (a diff, or a command's output so far).
        if status != ToolStatus::Running || output.is_some() {
            out.push((ts, Kind::ToolCallUpdate { id: id.to_string(), status, output }));
        }
        if status == ToolStatus::Completed {
            self.completed[0].insert(id.to_string());
        }
    }

    fn parse_events(&mut self, lines: &[String]) -> Vec<(u64, Kind)> {
        // Permissions come in pairs. Under yolo mode grok still asks and
        // answers itself: 1010 of 1013 observed requests resolved in under
        // 5 ms. A pair that is already closed by the time we read the file
        // never needed anyone, so both halves are dropped; a request still
        // open at the end of the batch is the real thing.
        let mut rows: Vec<Option<(u64, Kind)>> = Vec::new();
        let mut waiting: HashMap<String, usize> = HashMap::new();
        for line in lines {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(kind) = v["type"].as_str() else { continue };
            let ts = v["ts"].as_str().and_then(iso_ms).unwrap_or_else(now_ms);
            let tool = v["tool_name"].as_str().unwrap_or("tool").to_string();
            match kind {
                "permission_requested" => {
                    waiting.insert(tool.clone(), rows.len());
                    rows.push(Some((
                        ts,
                        Kind::Phase { phase: Phase::NeedsYou, detail: format!("permission: {tool}") },
                    )));
                }
                "permission_resolved" => match waiting.remove(&tool) {
                    Some(at) => {
                        rows[at] = None;
                    }
                    None => rows.push(Some((
                        ts,
                        Kind::Phase { phase: Phase::Working, detail: String::new() },
                    ))),
                },
                // The turn is already announced by updates.jsonl's user
                // chunk; only the phase is worth repeating.
                "turn_started" => {
                    rows.push(Some((ts, Kind::Phase { phase: Phase::Working, detail: String::new() })))
                }
                "turn_ended" => {
                    rows.push(Some((ts, Kind::Phase { phase: Phase::Idle, detail: String::new() })))
                }
                // The one thing events.jsonl knows that updates.jsonl does
                // not: 17 of 36 failing tools were written to updates.jsonl
                // as `completed`. Correct only a status we saw as completed,
                // so a card whose result has not arrived is left alone.
                "tool_completed" if v["outcome"].as_str() != Some("success") => {
                    let Some(id) = v["tool_call_id"].as_str() else { continue };
                    if self.completed[0].remove(id) || self.completed[1].remove(id) {
                        rows.push(Some((
                            ts,
                            Kind::ToolCallUpdate {
                                id: id.to_string(),
                                status: ToolStatus::Failed,
                                output: None,
                            },
                        )));
                    }
                }
                // `first_token`, `loop_started`, `tool_started`, a successful
                // `tool_completed`, `phase_changed` (37 714 of them across 22
                // sessions — a per-token status the spine has no use for),
                // `mcp_*`, `yolo_toggled`.
                _ => {}
            }
        }
        rows.into_iter().flatten().collect()
    }
}

/// Drag each timestamp up to the one before it. A file's own order is the
/// truth — the stamps only exist so the two files can be interleaved — and
/// grok's are not always sorted: a background task's `tool_call_update` is
/// stamped when the task started, up to 43 s before the line above it
/// (55 of 4122 lines observed). Sorting on the raw stamp would deal a
/// tool's result out ahead of its own card. [observed: grok 1.0.13]
fn monotonic(evs: &mut [(u64, Kind)]) {
    let mut floor = 0;
    for (ts, _) in evs.iter_mut() {
        floor = floor.max(*ts);
        *ts = floor;
    }
}

fn block(run: &Run, done: bool) -> Kind {
    let (id, text) = (run.id.clone(), run.text.clone());
    if run.thought {
        Kind::AgentThought { id, text, done }
    } else {
        Kind::AgentText { id, text, done }
    }
}

/// A chunk's words. `content` is a single ACP content block, text in every
/// observed case but one — a person can paste an image into a prompt.
fn chunk_text(content: &Value) -> String {
    match content["type"].as_str() {
        Some("text") => content["text"].as_str().unwrap_or_default().to_string(),
        Some("image") => {
            format!("[image {}]", content["mimeType"].as_str().unwrap_or("attached"))
        }
        _ => String::new(),
    }
}

/// A tool card from either the `tool_call` line or the `tool_call_update`
/// that fills it in.
fn tool_card(update: &Value, status: ToolStatus) -> Option<Kind> {
    let id = update["toolCallId"].as_str()?;
    let xai = &update["_meta"]["x.ai/tool"];
    let title = update["title"].as_str().unwrap_or_default();
    let tool = xai["name"]
        .as_str()
        .or_else(|| (!title.is_empty()).then_some(title))
        .unwrap_or("tool");
    Some(Kind::ToolCall {
        id: id.to_string(),
        tool: tool.to_string(),
        title: if title.is_empty() { tool.to_string() } else { title.to_string() },
        category: category(update["kind"].as_str(), xai["kind"].as_str()),
        input: summarize(&update["rawInput"]),
        status,
    })
}

/// The card's mark. The ACP `kind` is only on the fill-in line; the first
/// line carries grok's own richer vocabulary under `_meta["x.ai/tool"]`, so
/// that is mapped the way grok itself maps it (measured by pairing the two
/// lines of all 1013 calls). [observed: grok 1.0.13]
fn category(acp: Option<&str>, xai: Option<&str>) -> ToolCategory {
    match acp.or(xai) {
        Some("read") => ToolCategory::Read,
        Some("edit" | "write") => ToolCategory::Edit,
        Some("execute") => ToolCategory::Execute,
        Some("search") => ToolCategory::Search,
        Some("fetch" | "web_fetch") => ToolCategory::Fetch,
        Some("think" | "plan") => ToolCategory::Think,
        // `list`, `image_gen`, `task`, `search_tool`,
        // `background_task_action`, `kill_task_action` — grok calls these
        // `other` on the fill-in line too.
        _ => ToolCategory::Other,
    }
}

fn tool_status(s: &str) -> ToolStatus {
    match s {
        "pending" => ToolStatus::Pending,
        "in_progress" => ToolStatus::Running,
        "completed" => ToolStatus::Completed,
        "failed" => ToolStatus::Failed,
        "cancelled" => ToolStatus::Cancelled,
        _ => ToolStatus::Running,
    }
}

/// `rawInput` on one line: `k=v` pairs, minus the `variant` tag grok uses to
/// name the input's own shape.
fn summarize(raw: &Value) -> String {
    let text = match raw.as_object() {
        Some(map) => map
            .iter()
            .filter(|(k, _)| k.as_str() != "variant")
            .map(|(k, v)| match v.as_str() {
                Some(s) => format!("{k}={s}"),
                None => format!("{k}={v}"),
            })
            .collect::<Vec<_>>()
            .join(" "),
        None if raw.is_null() => String::new(),
        None => raw.to_string(),
    };
    clip(&text.split_whitespace().collect::<Vec<_>>().join(" "), INPUT_MAX)
}

/// What a tool showed. `content` is an array of ACP tool-call content:
/// `{"type":"content","content":{"type":"text"|"image",…}}` or a
/// `{"type":"diff","path","oldText","newText"}`. [observed: grok 1.0.13]
fn tool_output(content: &Value) -> Option<String> {
    let parts = content.as_array()?;
    let text = parts
        .iter()
        .filter_map(|p| match p["type"].as_str() {
            Some("diff") => Some(format!("[diff {}]", p["path"].as_str().unwrap_or("?"))),
            Some("content") => match p["content"]["type"].as_str() {
                Some("text") => Some(p["content"]["text"].as_str()?.to_string()),
                // Base64 image payloads run to megabytes; the phone gets the
                // fact, not the bytes.
                Some("image") => Some(format!(
                    "[image {}]",
                    p["content"]["mimeType"].as_str().unwrap_or("attached")
                )),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then(|| clip(&text, OUTPUT_MAX))
}

/// When a line happened: xAI's own millisecond stamp, else the JSON-RPC
/// envelope's unix seconds, else now.
fn ts_ms(v: &Value, params: &Value) -> u64 {
    params["_meta"]["agentTimestampMs"]
        .as_u64()
        .or_else(|| v["timestamp"].as_u64().map(|s| s * 1000))
        .unwrap_or_else(now_ms)
}

/// "2026-08-28T22:30:22.790Z" → ms. events.jsonl stamps ISO where
/// updates.jsonl stamps numbers; this is the same civil-days arithmetic as
/// `remote_api::parse_iso_secs`, kept to the millisecond.
fn iso_ms(s: &str) -> Option<u64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = rest.trim_end_matches('Z');
    let time = time.split(['+', '-']).next().unwrap_or(time);
    let mut t = time.split(':');
    let (h, mi) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let secs_part = t.next()?;
    let (sec, frac) = match secs_part.split_once('.') {
        Some((s, f)) => (s.parse::<i64>().ok()?, format!("{f:0<3}")[..3].parse::<i64>().ok()?),
        None => (secs_part.parse::<i64>().ok()?, 0),
    };
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from((days * 86400 + h * 3600 + mi * 60 + sec) * 1000 + frac).ok()
}

/// One append-only file read from a byte offset. Grok appends whole lines,
/// but a poll can land between a line's bytes and its newline, so the
/// trailing fragment is held until the newline arrives.
struct Tail {
    path: PathBuf,
    offset: u64,
    partial: String,
    /// Which file we have been reading, so a replacement is noticed even
    /// when the new one is already longer than the old.
    id: Option<u64>,
}

impl Tail {
    fn new(path: PathBuf) -> Self {
        Self { path, offset: 0, partial: String::new(), id: None }
    }

    fn rewind(&mut self) {
        self.offset = 0;
        self.partial.clear();
        self.id = None;
    }

    /// Whole lines since the last call. `None` means the file was truncated
    /// or replaced and the caller must rebuild from zero.
    fn take(&mut self) -> Option<Vec<String>> {
        // A session that has not completed a turn has no updates.jsonl yet;
        // it appears under us, and until it does there is nothing to say.
        let Ok(meta) = std::fs::metadata(&self.path) else { return Some(Vec::new()) };
        let id = file_id(&meta);
        if meta.len() < self.offset || self.id.is_some_and(|was| was != id) {
            return None;
        }
        self.id = Some(id);
        if meta.len() == self.offset {
            return Some(Vec::new());
        }
        let mut buf = Vec::new();
        let read = std::fs::File::open(&self.path)
            .and_then(|mut f| {
                f.seek(SeekFrom::Start(self.offset))?;
                f.read_to_end(&mut buf)
            })
            .unwrap_or(0);
        self.offset += read as u64;
        let mut text = std::mem::take(&mut self.partial);
        text.push_str(&String::from_utf8_lossy(&buf));
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        // Whatever follows the last newline is not a line yet.
        self.partial = lines.pop().unwrap_or_default();
        Some(lines.into_iter().filter(|l| !l.trim().is_empty()).collect())
    }
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(meta)
}

#[cfg(not(unix))]
fn file_id(meta: &std::fs::Metadata) -> u64 {
    // No inode to ask for; a shorter file is the only rotation we can see.
    let _ = meta;
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare adapter over two paths, so the parsing can be driven from
    /// files a test writes rather than from ~/.grok.
    fn adapter(dir: &std::path::Path) -> GrokAdapter {
        GrokAdapter {
            updates: Tail::new(dir.join("updates.jsonl")),
            events: Tail::new(dir.join("events.jsonl")),
            dir: dir.to_path_buf(),
            ordinal: 1,
            run: None,
            turn: "0".to_string(),
            completed: Default::default(),
        }
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("spine-grok-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(dir: &std::path::Path, file: &str, body: &str) {
        std::fs::write(dir.join(file), body).unwrap();
    }

    fn append(dir: &std::path::Path, file: &str, body: &str) {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(file)).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    // Lines below are copied from ~/.grok/sessions/…/5d992ea4-… and
    // …/01a02bd1-…, with long prose and rawOutput shortened. Shapes are
    // verbatim. [observed: grok 1.0.13, 2026-09-02]
    const USER: &str = r#"{"timestamp":1787956220,"method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"build a weather page"},"_meta":{"modelId":"grok-4.6","promptIndex":0}},"_meta":{"eventId":"S-2","agentTimestampMs":1787956219612}}}"#;
    const THOUGHT: &str = r#"{"timestamp":1787956221,"method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"The user wants a page. "}},"_meta":{"totalTokens":6090,"eventId":"S-43","agentTimestampMs":1787956221037,"updateType":"AgentThoughtChunk","chunkId":41}}}"#;
    const SAY: &str = r#"{"timestamp":1787956222,"method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I'll start "}},"_meta":{"totalTokens":6090,"eventId":"S-61","agentTimestampMs":1787956221679,"updateType":"AgentMessageChunk","chunkId":59}}}"#;
    const CALL: &str = r#"{"timestamp":1787956222,"method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"tool_call","toolCallId":"call-A-0","title":"read_file","rawInput":{"target_file":"/home/admin/AI-OS/CLAUDE.md"},"_meta":{"x.ai/tool":{"version":1,"name":"read_file","kind":"read","namespace":"grok_build","label":"Read","read_only":true}}},"_meta":{"totalTokens":16475,"eventId":"S-63","agentTimestampMs":1787956222786,"updateType":"ToolCall","updateParams":{"toolCallId":"call-A-0","title":"read_file","kind":"Other","status":"Pending"}}}}"#;
    const CALL_FILLED: &str = r#"{"timestamp":1787956222,"method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-A-0","kind":"read","title":"Read `/home/admin/AI-OS/CLAUDE.md`","locations":[{"path":"/home/admin/AI-OS/CLAUDE.md"}],"rawInput":{"variant":"ReadFile","target_file":"/home/admin/AI-OS/CLAUDE.md"},"_meta":{"x.ai/tool":{"version":1,"name":"read_file","kind":"read","namespace":"grok_build","label":"Read","read_only":true}}},"_meta":{"eventId":"S-64","agentTimestampMs":1787956222787,"updateType":"ToolCallUpdate","updateParams":{"toolCallId":"call-A-0","status":null}}}}"#;
    const CALL_DONE: &str = r#"{"timestamp":1787956222,"method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-A-0","status":"completed","content":[{"type":"content","content":{"type":"text","text":"1→# aiterm"}}],"rawOutput":{"ok":true}},"_meta":{"eventId":"S-65","agentTimestampMs":1787956222790}}}"#;
    const TURN_DONE: &str = r#"{"timestamp":1787956230,"method":"_x.ai/session/update","params":{"sessionId":"S","update":{"sessionUpdate":"turn_completed","prompt_id":"P","stop_reason":"end_turn","usage":{"inputTokens":48927,"outputTokens":1504,"totalTokens":50431,"numTurns":3}},"_meta":{"eventId":"S-1345","agentTimestampMs":1787956230527}}}"#;
    const PLAN: &str = r#"{"timestamp":1787956223,"method":"session/update","params":{"sessionId":"S","update":{"sessionUpdate":"plan","entries":[{"content":"Load skills","priority":"medium","status":"in_progress"}]},"_meta":{"eventId":"S-70","agentTimestampMs":1787956223000}}}"#;

    const EV_TURN_STARTED: &str = r#"{"ts":"2026-08-28T22:30:19.612Z","type":"turn_started","session_id":"S","turn_number":0,"model_id":"grok-4.6","yolo_mode":true,"conversation_message_count":3,"session_relationship":"primary","schema_version":"1.0"}"#;
    const EV_PERM_REQ: &str = r#"{"ts":"2026-08-28T22:30:22.787Z","type":"permission_requested","tool_name":"read_file"}"#;
    const EV_PERM_OK: &str = r#"{"ts":"2026-08-28T22:30:22.788Z","type":"permission_resolved","tool_name":"read_file","decision":"allow","wait_ms":0}"#;
    const EV_TOOL_ERR: &str = r#"{"ts":"2026-08-28T22:30:22.790Z","type":"tool_completed","tool_name":"read_file","duration_ms":0,"outcome":"error","tool_call_id":"call-A-0"}"#;
    const EV_PHASE: &str = r#"{"ts":"2026-08-28T22:30:20.714Z","type":"phase_changed","phase":"streaming_text"}"#;

    fn kinds(evs: &[(u64, Kind)]) -> Vec<&Kind> {
        evs.iter().map(|(_, k)| k).collect()
    }

    #[test]
    fn a_user_chunk_opens_a_turn_and_speaks() {
        let d = tmpdir("user");
        write(&d, "updates.jsonl", &format!("{USER}\n"));
        let got = adapter(&d).bootstrap();
        assert_eq!(
            kinds(&got),
            vec![
                &Kind::TurnStarted { turn: "1".into() },
                &Kind::UserMessage { id: "u1".into(), text: "build a weather page".into() },
            ]
        );
        // agentTimestampMs wins over the envelope's unix seconds.
        assert_eq!(got[0].0, 1787956219612);
    }

    #[test]
    fn a_run_of_agent_chunks_is_one_growing_block_closed_by_the_next_line() {
        let d = tmpdir("run");
        write(&d, "updates.jsonl", &format!("{SAY}\n{SAY}\n{SAY}\n{CALL}\n"));
        let got = adapter(&d).bootstrap();
        let texts: Vec<_> = got
            .iter()
            .filter_map(|(_, k)| match k {
                Kind::AgentText { id, text, done } => Some((id.as_str(), text.as_str(), *done)),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                ("a1", "I'll start ", false),
                ("a1", "I'll start I'll start ", false),
                ("a1", "I'll start I'll start I'll start ", false),
                ("a1", "I'll start I'll start I'll start ", true),
            ]
        );
        assert!(matches!(got.last(), Some((_, Kind::ToolCall { .. }))));
    }

    #[test]
    fn a_thought_run_is_its_own_block() {
        let d = tmpdir("thought");
        write(&d, "updates.jsonl", &format!("{THOUGHT}\n{THOUGHT}\n{SAY}\n"));
        let got = adapter(&d).bootstrap();
        assert_eq!(
            kinds(&got),
            vec![
                &Kind::AgentThought { id: "a1".into(), text: "The user wants a page. ".into(), done: false },
                &Kind::AgentThought {
                    id: "a1".into(),
                    text: "The user wants a page. The user wants a page. ".into(),
                    done: false
                },
                &Kind::AgentThought {
                    id: "a1".into(),
                    text: "The user wants a page. The user wants a page. ".into(),
                    done: true
                },
                &Kind::AgentText { id: "a3".into(), text: "I'll start ".into(), done: false },
            ]
        );
    }

    #[test]
    fn a_tool_call_is_issued_filled_in_then_finished() {
        let d = tmpdir("tool");
        write(&d, "updates.jsonl", &format!("{CALL}\n{CALL_FILLED}\n{CALL_DONE}\n"));
        let got = adapter(&d).bootstrap();
        assert_eq!(
            kinds(&got),
            vec![
                &Kind::ToolCall {
                    id: "call-A-0".into(),
                    tool: "read_file".into(),
                    title: "read_file".into(),
                    category: ToolCategory::Read,
                    input: "target_file=/home/admin/AI-OS/CLAUDE.md".into(),
                    status: ToolStatus::Pending,
                },
                // The fill-in line brings the human title and the ACP kind,
                // so the card is re-issued under the same id.
                &Kind::ToolCall {
                    id: "call-A-0".into(),
                    tool: "read_file".into(),
                    title: "Read `/home/admin/AI-OS/CLAUDE.md`".into(),
                    category: ToolCategory::Read,
                    input: "target_file=/home/admin/AI-OS/CLAUDE.md".into(),
                    status: ToolStatus::Running,
                },
                &Kind::ToolCallUpdate {
                    id: "call-A-0".into(),
                    status: ToolStatus::Completed,
                    output: Some("1→# aiterm".into()),
                },
            ]
        );
    }

    #[test]
    fn turn_completed_closes_an_open_block_and_ends_the_turn() {
        let d = tmpdir("turnend");
        write(&d, "updates.jsonl", &format!("{USER}\n{SAY}\n{PLAN}\n{TURN_DONE}\n"));
        let got = adapter(&d).bootstrap();
        // `plan` says nothing, but it does end the prose block.
        assert_eq!(
            kinds(&got)[2..],
            [
                &Kind::AgentText { id: "a2".into(), text: "I'll start ".into(), done: false },
                &Kind::AgentText { id: "a2".into(), text: "I'll start ".into(), done: true },
                &Kind::TurnEnded { turn: "1".into(), reason: "completed".into() },
            ]
        );
    }

    #[test]
    fn a_permission_answered_before_we_looked_never_needed_anyone() {
        let d = tmpdir("perm-fast");
        write(&d, "events.jsonl", &format!("{EV_TURN_STARTED}\n{EV_PERM_REQ}\n{EV_PERM_OK}\n{EV_PHASE}\n"));
        let got = adapter(&d).bootstrap();
        assert_eq!(
            kinds(&got),
            vec![&Kind::Phase { phase: Phase::Working, detail: String::new() }]
        );
    }

    #[test]
    fn a_permission_still_open_asks_for_you_and_is_released_next_poll() {
        let d = tmpdir("perm-slow");
        write(&d, "events.jsonl", &format!("{EV_PERM_REQ}\n"));
        let mut a = adapter(&d);
        assert_eq!(
            kinds(&a.bootstrap()),
            vec![&Kind::Phase { phase: Phase::NeedsYou, detail: "permission: read_file".into() }]
        );
        append(&d, "events.jsonl", &format!("{EV_PERM_OK}\n"));
        assert_eq!(
            kinds(&a.poll()),
            vec![&Kind::Phase { phase: Phase::Working, detail: String::new() }]
        );
    }

    #[test]
    fn events_correct_a_tool_that_updates_called_completed() {
        let d = tmpdir("toolerr");
        write(&d, "updates.jsonl", &format!("{CALL}\n{CALL_DONE}\n"));
        write(&d, "events.jsonl", &format!("{EV_TOOL_ERR}\n"));
        let got = adapter(&d).bootstrap();
        assert_eq!(
            got.last().map(|(_, k)| k),
            Some(&Kind::ToolCallUpdate {
                id: "call-A-0".into(),
                status: ToolStatus::Failed,
                output: None
            })
        );
        // Same millisecond as the `completed` it corrects — the tie-break
        // is what puts it after.
        assert_eq!(got[got.len() - 2].0, got[got.len() - 1].0);
    }

    #[test]
    fn the_two_files_interleave_by_timestamp() {
        let d = tmpdir("merge");
        // The user line is stamped 1787956219612; the events line is
        // 2026-08-28T22:30:19.612Z, which is the same instant.
        write(&d, "updates.jsonl", &format!("{USER}\n{SAY}\n"));
        write(&d, "events.jsonl", &format!("{EV_TURN_STARTED}\n{EV_PHASE}\n"));
        let got = adapter(&d).bootstrap();
        assert_eq!(iso_ms("2026-08-28T22:30:19.612Z"), Some(1787956219612));
        let stamps: Vec<u64> = got.iter().map(|(ts, _)| *ts).collect();
        assert!(stamps.windows(2).all(|w| w[0] <= w[1]), "{stamps:?}");
        // updates first on a tie, then the events phase, then the prose.
        assert!(matches!(got[0].1, Kind::TurnStarted { .. }));
        assert!(matches!(got[1].1, Kind::UserMessage { .. }));
        assert_eq!(got[2].1, Kind::Phase { phase: Phase::Working, detail: String::new() });
        assert!(matches!(got[3].1, Kind::AgentText { .. }));
    }

    #[test]
    fn half_a_line_waits_for_its_newline() {
        let d = tmpdir("partial");
        let (head, tail) = USER.split_at(120);
        write(&d, "updates.jsonl", head);
        let mut a = adapter(&d);
        assert!(a.bootstrap().is_empty());
        append(&d, "updates.jsonl", &format!("{tail}\n"));
        assert_eq!(
            kinds(&a.poll()),
            vec![
                &Kind::TurnStarted { turn: "1".into() },
                &Kind::UserMessage { id: "u1".into(), text: "build a weather page".into() },
            ]
        );
    }

    #[test]
    fn a_truncated_file_rebuilds_behind_a_reset() {
        let d = tmpdir("reset");
        write(&d, "updates.jsonl", &format!("{USER}\n{SAY}\n"));
        let mut a = adapter(&d);
        assert_eq!(a.bootstrap().len(), 3);
        write(&d, "updates.jsonl", &format!("{USER}\n"));
        let got = a.poll();
        assert_eq!(
            kinds(&got),
            vec![
                &Kind::Reset,
                &Kind::TurnStarted { turn: "1".into() },
                &Kind::UserMessage { id: "u1".into(), text: "build a weather page".into() },
            ]
        );
    }

    #[test]
    fn a_session_with_no_updates_file_yet_is_simply_quiet() {
        let d = tmpdir("empty");
        let mut a = adapter(&d);
        assert!(a.bootstrap().is_empty());
        append(&d, "updates.jsonl", &format!("{USER}\n"));
        assert_eq!(a.poll().len(), 2);
    }

    /// Run against the real thing: `cargo test --lib spine::grok -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads ~/.grok, which only exists on a machine that runs grok"]
    fn bootstrap_a_real_session() {
        let root = dirs::home_dir().unwrap().join(".grok/sessions");
        let mut sessions: Vec<PathBuf> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|c| c.path().is_dir())
            .flat_map(|c| std::fs::read_dir(c.path()).unwrap().flatten().map(|s| s.path()))
            .filter(|s| s.join("updates.jsonl").is_file())
            .collect();
        sessions.sort();
        let mut histogram: std::collections::BTreeMap<String, usize> = Default::default();
        for dir in sessions {
            let lines = |f: &str| {
                std::fs::read_to_string(dir.join(f)).map(|s| s.lines().count()).unwrap_or(0)
            };
            let (u, e) = (lines("updates.jsonl"), lines("events.jsonl"));
            let start = std::time::Instant::now();
            let out = adapter(&dir).bootstrap();
            println!(
                "{:<38} {u:>5} updates + {e:>6} events → {:>5} events in {:>6.1} ms",
                dir.file_name().unwrap().to_string_lossy(),
                out.len(),
                start.elapsed().as_secs_f64() * 1000.0
            );
            for (_, k) in &out {
                let tag = serde_json::to_value(k).unwrap()["kind"].as_str().unwrap().to_string();
                *histogram.entry(tag).or_default() += 1;
            }
            // At most one block may still be open — the one the file ends
            // on, which a later chunk may still grow. Any earlier block left
            // at `done:false` would be a fold that never closed.
            let open: std::collections::BTreeSet<&String> = out
                .iter()
                .filter_map(|(_, k)| match k {
                    Kind::AgentText { id, done: false, .. }
                    | Kind::AgentThought { id, done: false, .. } => Some(id),
                    _ => None,
                })
                .filter(|id| {
                    !out.iter().any(|(_, k)| {
                        matches!(k, Kind::AgentText { id: i, done: true, .. }
                            | Kind::AgentThought { id: i, done: true, .. } if i == *id)
                    })
                })
                .collect();
            assert!(open.len() <= 1, "{dir:?} left blocks open: {open:?}");
        }
        println!("{histogram:#?}");
    }
}
