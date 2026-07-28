use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub agent: String,
    pub title: String,
    pub project_path: String,
    /// Project the row groups under. Same as `project_path` except for
    /// sessions living in a Claude Code worktree (`<repo>/.claude/worktrees/…`),
    /// which group under `<repo>` so a `/fork` lands beside its parent instead
    /// of in a group of its own. `project_path` stays the true cwd — it's what
    /// a resumed tab is spawned in.
    pub group_path: String,
    pub branch: Option<String>,
    /// This transcript continues a conversation whose earlier messages live in
    /// another file — a `/fork` child or a compact continuation. Its parent is
    /// still on disk, frozen at the fork point, and must stay listed.
    pub forked: bool,
    /// Ran under the daemon as a background agent (`sessionKind":"bg"`), i.e.
    /// `/fork` or `--bg`. `forked && background` is a `/fork` child: the
    /// terminal that spawned it stayed on the parent, so no tab moves.
    pub background: bool,
    /// Session this one was forked from, per Claude Code's job state. Known
    /// from the instant `/fork` runs — unlike the transcript chain, which only
    /// reveals lineage once messages land.
    pub fork_parent: Option<String>,
    /// Unix millis of last activity (file mtime).
    pub last_active: u64,
    /// **Which source started this session**, when aiterm was the one that
    /// started it: the registry id the picker offered — `"claude"`, `"codex"`,
    /// `"api:openrouter"`. `None` for everything else, which is most rows on a
    /// machine that has been used for a while.
    ///
    /// Deliberately *not* the same question as `agent`, and the whole reason
    /// this field exists. `agent` is the engine that owns the transcript, and
    /// it is stamped by the registry from the backend that found the file. A
    /// session started against an API provider **is** Claude Code — aiterm sets
    /// `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` and runs `claude` — so its
    /// transcript lands in `~/.claude/projects` and `ClaudeProvider` yields it
    /// tagged `agent: "claude"`, correctly. Nothing anywhere on disk records
    /// that OpenRouter was ever involved.
    ///
    /// So it cannot be derived; it has to be *recorded*, by the code that knew
    /// it at the time. See `record_session_source` — same shape, and for the
    /// same reason, as the fork lineage in `forks.json` below.
    pub source: Option<String>,
    /// Human-facing name for `source`, resolved from the registry at scan time
    /// rather than stored: a provider renamed in settings should rename its
    /// rows, and a stored copy would go stale the moment it was written.
    pub source_label: Option<String>,
    /// The model this session runs on, as far as aiterm can tell. Recorded at
    /// launch for sessions aiterm started; read out of the rollout for Codex,
    /// which writes it into every `turn_context`. `None` when nobody said.
    ///
    /// Not the same as the `session_model` command, which reads what a *Claude*
    /// transcript last switched to. This one never parses a transcript for it.
    pub source_model: Option<String>,
}

/// Where one agent keeps its sessions.
///
/// Wide enough to be the only thing the rest of the app needs in order to list,
/// index and open a session — that is the bar, and the previous single-method
/// version did not meet it: the indexer reached past the trait to
/// `ClaudeProvider` directly, so a second agent would have appeared in the
/// sidebar and been missing from search with nothing to explain why.
///
/// See `agents.rs` for what is still hard-wired to Claude Code beyond this.
pub trait SessionProvider: Send + Sync {
    /// Sessions along with their transcript paths. The one method a backend
    /// must write; the rest are derived from it.
    fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)>;

    /// The transcript for `session_id`, or `None` if this provider does not
    /// own that id. Returning `None` for another agent's session is what lets
    /// the registry route by asking rather than by parsing ids.
    fn find_session_file(&self, session_id: &str) -> Option<std::path::PathBuf>;

    fn scan(&self) -> Vec<Session> {
        self.scan_with_paths().into_iter().map(|(s, _)| s).collect()
    }
}

pub struct ClaudeProvider;

impl SessionProvider for ClaudeProvider {
    fn find_session_file(&self, session_id: &str) -> Option<std::path::PathBuf> {
        claude_session_file(session_id)
    }

    /// Scan sessions along with their jsonl paths (needed by the indexer).
    fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)> {
        let Some(home) = dirs::home_dir() else {
            return vec![];
        };
        let root = home.join(".claude/projects");
        let Ok(projects) = std::fs::read_dir(&root) else {
            return vec![];
        };

        let mut sessions = Vec::new();
        for project in projects.flatten() {
            let Ok(files) = std::fs::read_dir(project.path()) else {
                continue;
            };
            let paths: Vec<std::path::PathBuf> = files
                .flatten()
                .map(|f| f.path())
                .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                // Claude Code renames superseded transcripts to
                // <id>.orphaned-<ts>-<hash>.jsonl — not resumable sessions.
                .filter(|p| {
                    !p.file_name()
                        .is_some_and(|n| n.to_string_lossy().contains(".orphaned-"))
                })
                .collect();
            // Every real session in a project dir records the same cwd; find it
            // once so /fork stub files (title-only, no cwd) can borrow it.
            let dir_cwd = paths.iter().find_map(|p| read_first_cwd(p));
            for path in paths {
                if let Some(s) = parse_session(&path, dir_cwd.as_deref()) {
                    sessions.push((s, path));
                }
            }
        }
        // Attach fork lineage from Claude Code's job state. This has to come
        // from outside the transcript: a fresh `/fork` stub is two lines
        // (`ai-title` + `agent-name`) with no message chain at all, so the
        // in-file heuristic reads `forked = false` at exactly the moment the
        // UI decides whether to hide the parent — which hid it. The job state
        // records the pair the instant the fork is created.
        // Two sources, same shape: aiterm's own record for branches made with
        // ⑂, and Claude Code's job state for `/fork`. Ours is checked first —
        // it was written by the code that created the file, so it is the more
        // direct evidence. In practice the key sets are disjoint.
        let parents = fork_parent_map(&home.join(".claude/jobs"));
        let ours = read_aiterm_fork_map();
        for (s, _) in &mut sessions {
            if let Some(parent) = ours.get(&s.id).or_else(|| parents.get(&s.id)) {
                s.fork_parent = Some(parent.clone());
                s.forked = true;
            }
        }

        sessions.sort_by(|a, b| b.0.last_active.cmp(&a.0.last_active));

        // One live `<id>.jsonl` = one row, even when several share a
        // `bridgeSessionId`. An explicit `--fork-session` leaves the original
        // transcript intact and independently resumable — the fork and its
        // parent are BOTH real sessions, each frozen at its own point, so
        // collapsing the family to the newest row (as we used to) hid the
        // parent and made its context unreachable. The duplicate-row problem
        // that collapse solved came from resume minting a fork on every
        // select; resume now forks only when the session is actually running,
        // and /clear/compact retire the old file via the `.orphaned-` rename
        // filtered above — so no collapse is needed.
        sessions
    }
}

/// Where aiterm records the branches *it* creates: branch id → parent id.
/// Claude Code's job state (below) only knows about its own `/fork`, so a
/// branch made by the ⑂ button would otherwise wear no lineage and show up as
/// an unexplained twin of its parent.
fn aiterm_fork_map_path() -> Option<std::path::PathBuf> {
    Some(dirs::data_dir()?.join("aiterm/forks.json"))
}

fn read_aiterm_fork_map() -> std::collections::HashMap<String, String> {
    let Some(path) = aiterm_fork_map_path() else {
        return Default::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Record a branch's parent. Best-effort: a fork whose lineage fails to save
/// is still a perfectly good session, so this never fails the fork itself.
fn record_aiterm_fork(branch: &str, parent: &str) {
    let Some(path) = aiterm_fork_map_path() else {
        return;
    };
    let mut map = read_aiterm_fork_map();
    map.insert(branch.to_string(), parent.to_string());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(path, text);
    }
}

/* ---- which source started a session ------------------------------------- */

/// What aiterm knows about how a session was started.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSource {
    /// Registry id of the backend the user picked: `"claude"`, `"codex"`,
    /// `"api:<provider-slug>"`. Stored raw rather than reduced to a category,
    /// because the category is a display decision and this file outlives it.
    pub agent: String,
    /// Model chosen in the picker, if one was. Blank means "whatever the agent
    /// would do on its own", which is a real answer and not a missing one — it
    /// is stored as absent rather than as an empty string so the two do not
    /// have to be told apart later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Where aiterm records which source started a session: session id → source.
///
/// The second thing in this app that has to be written down because it cannot
/// be read back off disk — `forks.json`, directly above, was the first, and
/// this follows it deliberately rather than inventing a new mechanism.
///
/// The fact being recorded is narrow and load-bearing: **a session started
/// against an API provider is Claude Code**, pointed at another endpoint by two
/// environment variables. It writes an ordinary Claude Code transcript into
/// `~/.claude/projects`, with nothing in it naming the provider — not the base
/// URL, not the model (the model line records whatever Claude Code thinks it
/// is asking for), not a marker of any kind. Grep the store afterwards and
/// there is genuinely no way to tell it from a session on your own plan.
///
/// Which is why the sidebar could not tell them apart, and why adding icons did
/// not help: every row was `agent: "claude"` by construction, so every row drew
/// the same mark, correctly.
///
/// The one moment the answer exists is the moment before launch, in
/// `newSession`, which has just minted the session id *and* holds the picked
/// source. Write it there or lose it.
///
/// ### What this does not do
///
/// - It does not grow a row. Only sessions aiterm itself started are in here;
///   an entry is looked up by id and its absence means "aiterm did not start
///   this", which is the honest answer and the common one.
/// - It is never pruned. An entry is ~60 bytes and outlives the session it
///   names, exactly like `forks.json`. Pruning would mean a write on every
///   scan to save a few kilobytes a year, and a scan that writes is a scan that
///   can corrupt.
/// - It is not consulted for anything but display. Nothing resumes, forks or
///   deletes differently because of what is in here.
fn aiterm_source_map_path() -> Option<std::path::PathBuf> {
    Some(dirs::data_dir()?.join("aiterm/sources.json"))
}

/// Parse the file's contents. Split from the read so the tolerance below can be
/// tested without a home directory: a half-written or hand-edited file must
/// degrade to "aiterm started none of these", never to a failed scan.
pub fn parse_source_map(text: &str) -> HashMap<String, SessionSource> {
    serde_json::from_str(text).unwrap_or_default()
}

pub fn read_aiterm_source_map() -> HashMap<String, SessionSource> {
    let Some(path) = aiterm_source_map_path() else {
        return Default::default();
    };
    std::fs::read_to_string(path)
        .map(|s| parse_source_map(&s))
        .unwrap_or_default()
}

/// Note that `session_id` was started as `agent`. Best-effort, like the fork
/// record: a session whose provenance fails to save is still a perfectly good
/// session, and failing the launch over a display detail would be absurd.
#[tauri::command]
pub fn record_session_source(session_id: String, agent: String, model: Option<String>) {
    write_session_source(
        &session_id,
        SessionSource {
            agent,
            model: model.filter(|m| !m.is_empty()),
        },
    );
}

fn write_session_source(session_id: &str, rec: SessionSource) {
    let Some(path) = aiterm_source_map_path() else {
        return;
    };
    let mut map = read_aiterm_source_map();
    map.insert(session_id.to_string(), rec);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(path, text);
    }
}

/// A branch inherits its parent's source. Nothing in the copy says where the
/// conversation came from, and a branch of an OpenRouter session is still an
/// OpenRouter session — without this the ⑂ copy would appear beside its parent
/// wearing a different mark, which is precisely the confusion this whole change
/// is about.
fn inherit_session_source(child: &str, parent: &str) {
    if let Some(rec) = read_aiterm_source_map().get(parent).cloned() {
        write_session_source(child, rec);
    }
}

/// Fill in `source`/`source_label`/`source_model` for one row.
///
/// Pure, and given its inputs rather than reading them, so the resolution rules
/// below can be tested without a data directory or a configured provider.
///
/// `labels` is registry id → display name. A source whose backend is gone —
/// the provider was deleted from settings after the session ran — still gets a
/// readable name from its own id rather than falling back to nothing: the
/// session did run against something, and saying "openrouter" is better than
/// silently demoting the row to look like an ordinary local one.
pub fn attach_source(
    s: &mut Session,
    map: &HashMap<String, SessionSource>,
    labels: &HashMap<String, String>,
) {
    let Some(rec) = map.get(&s.id) else {
        return;
    };
    s.source = Some(rec.agent.clone());
    s.source_label = Some(source_label(&rec.agent, labels));
    if s.source_model.is_none() {
        s.source_model = rec.model.clone();
    }
}

fn source_label(id: &str, labels: &HashMap<String, String>) -> String {
    if let Some(name) = labels.get(id) {
        return name.clone();
    }
    id.strip_prefix("api:").unwrap_or(id).to_string()
}

/// Fork lineage from Claude Code's job state files: forked session UUID →
/// parent session UUID. Each `~/.claude/jobs/<short>/state.json` of a `/fork`
/// job carries `forkSessionId` + `forkParentSessionId`; jobs without the pair
/// (plain bg jobs, interactive sessions) are skipped. State files are small
/// and few, so reading them per scan is cheap.
///
/// (Ported from the worktree-fix-agents-sync branch, which found this source.)
fn fork_parent_map(jobs_dir: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(jobs) = std::fs::read_dir(jobs_dir) else {
        return map;
    };
    for job in jobs.flatten() {
        let Ok(raw) = std::fs::read_to_string(job.path().join("state.json")) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        // `forkParentSessionId` alone identifies a fork job. Key it by
        // `sessionId` — `forkSessionId` is written only sometimes (a job forked
        // from another fork omits it), and requiring both silently skipped
        // exactly those, leaving their parent hidden after all.
        let parent = v.get("forkParentSessionId").and_then(|s| s.as_str());
        let fork = v
            .get("forkSessionId")
            .and_then(|s| s.as_str())
            .or_else(|| v.get("sessionId").and_then(|s| s.as_str()));
        if let (Some(fork), Some(parent)) = (fork, parent) {
            if fork != parent {
                map.insert(fork.to_string(), parent.to_string());
            }
        }
    }
    map
}

/// Read the first `cwd` a transcript records. Used to backfill the project for
/// Claude Code /fork stub files, which are title-only and omit `cwd`. Only the
/// head of the file is scanned — cwd appears on the earliest real records.
fn read_first_cwd(path: &Path) -> Option<String> {
    let reader = BufReader::new(File::open(path).ok()?);
    for line in reader.lines().take(80).flatten() {
        if !line.contains("\"cwd\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                if !c.is_empty() {
                    return Some(c.to_string());
                }
            }
        }
    }
    None
}

/// Read a transcript's `bridgeSessionId` — the stable id shared across every
/// fork of one logical conversation. The `bridge-session` record is appended
/// periodically (its `lastSequenceNum` grows), so it lives near the end of
/// the file; read only a tail window instead of the whole (multi-MB) file.
fn read_bridge_id(path: &Path) -> Option<String> {
    const TAIL: u64 = 128 * 1024;
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    // Arbitrary byte offset may split a UTF-8 char / a line — lossy-decode,
    // then drop the first (partial) line when we didn't start at byte 0.
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        lines.next();
    }
    // Last bridge-session record wins (id is constant across them anyway).
    let mut found = None;
    for line in lines {
        if !line.contains("\"bridge-session\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("bridge-session") {
                if let Some(id) = v.get("bridgeSessionId").and_then(|b| b.as_str()) {
                    found = Some(id.to_string());
                }
            }
        }
    }
    found
}


/// System XML tags Claude Code injects into user messages (ported from
/// claudeman's parser — keep the lists in sync).
fn is_system_tag_name(tag: &str) -> bool {
    matches!(
        tag,
        "ide_selection"
            | "ide_opened_file"
            | "ide_closed_file"
            | "command-message"
            | "command-name"
            | "command-args"
            | "local-command-stdout"
            | "local-command-stderr"
            | "local-command-caveat"
            | "system-reminder"
            | "user-prompt-submit-hook"
    )
}

/// Remove known system tags (with their content) from a prompt. Unbalanced
/// known tags drop the rest of the text; unknown tags pass through.
fn strip_system_tags(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open_idx) = rest.find('<') {
        out.push_str(&rest[..open_idx]);
        let after_lt = &rest[open_idx + 1..];
        let Some(gt_idx) = after_lt.find('>') else {
            out.push_str(&rest[open_idx..]);
            rest = "";
            break;
        };
        let tag_name = &after_lt[..gt_idx];
        let after_open = &after_lt[gt_idx + 1..];
        let close_pat = format!("</{tag_name}>");
        match after_open.find(&close_pat) {
            Some(close_idx) => rest = &after_open[close_idx + close_pat.len()..],
            None if is_system_tag_name(tag_name) => {
                // Truncated/unbalanced known system tag — drop the rest.
                rest = "";
                break;
            }
            None => {
                out.push('<');
                out.push_str(tag_name);
                out.push('>');
                rest = after_open;
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// System-injected meta prompts (memory summarizers, compression runs) that
/// should never be shown as a session title.
fn is_system_meta_prompt(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("You are summarizing a Claude Code session")
        || trimmed.starts_with("Caveat: The messages below were generated by the user")
        || trimmed.starts_with("Apply maximum non-destructive compression")
        || trimmed.starts_with("This session is being continued from a previous conversation")
}

/// Pull title/cwd/branch out of the first lines of a session jsonl without
/// parsing the whole transcript.
fn parse_session(path: &Path, dir_cwd: Option<&str>) -> Option<Session> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() == 0 {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;

    let id = path.file_stem()?.to_string_lossy().to_string();
    let reader = BufReader::new(File::open(path).ok()?);

    let mut title: Option<String> = None;
    // Claude Code's auto-generated title (record type `ai-title`). Used as a
    // last-resort title so /fork stub files — which contain ONLY an ai-title
    // ("<project> ⑂") and an agent-name, no prompt — still appear in the list.
    let mut ai_title: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut first_prompt: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut branch: Option<String> = None;
    // Fork files (orchestrator/agents view, compact continuations) start
    // with a compact boundary or bridge-session record and never contain a
    // prompt the user actually typed — they'd show up as duplicate entries.
    let mut fork_marker = false;
    let mut human_prompt = false;
    // Fork detection. Records are written parent-before-child, so a message
    // whose `parentUuid` was never defined earlier in this same file can only
    // be continuing another transcript — that's a `/fork` child (or a compact
    // continuation). A `/clear` writes a self-contained file whose first chain
    // link resolves in-file, which is how the two are told apart. Only the
    // first linked record decides it; later dangling refs are noise.
    let mut seen_uuids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut forked: Option<bool> = None;
    let mut background = false;

    for line in reader.lines().take(400).flatten() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if forked.is_none() {
            if let Some(parent) = v.get("parentUuid").and_then(|p| p.as_str()) {
                forked = Some(!seen_uuids.contains(parent));
            }
        }
        if let Some(u) = v.get("uuid").and_then(|u| u.as_str()) {
            seen_uuids.insert(u.to_string());
        }
        if !background {
            background = v.get("sessionKind").and_then(|k| k.as_str()) == Some("bg");
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("custom-title") => {
                title = v.get("customTitle").and_then(|t| t.as_str()).map(String::from)
            }
            Some("ai-title") if ai_title.is_none() => {
                ai_title = v
                    .get("aiTitle")
                    .and_then(|t| t.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            }
            Some("summary") => {
                summary = v.get("summary").and_then(|t| t.as_str()).map(String::from)
            }
            Some("bridge-session") => fork_marker = true,
            Some("system") => {
                if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") {
                    fork_marker = true;
                }
            }
            // A `/fork` child records what the user typed in a `last-prompt`
            // record, not as `user` message content — its own `user` records
            // are all replayed tool results. Without counting this, every fork
            // looked like promptless bookkeeping and got dropped by the
            // `fork_marker && !human_prompt` filter below, which is why a
            // `/fork` never earned a row in the list.
            Some("last-prompt") if !human_prompt => {
                let prompt = v
                    .get("lastPrompt")
                    .and_then(|p| p.as_str())
                    .map(strip_system_tags)
                    .filter(|s| !s.is_empty() && !is_system_meta_prompt(s));
                if let Some(p) = prompt {
                    human_prompt = true;
                    if first_prompt.is_none() {
                        first_prompt = Some(p.chars().take(80).collect());
                    }
                }
            }
            Some("user") if !human_prompt => {
                // Only accept a real human prompt: strip injected system tags
                // and skip auto-generated meta prompts entirely.
                let prompt = v
                    .pointer("/message/content")
                    .and_then(|c| c.as_str())
                    .map(strip_system_tags)
                    .filter(|s| !s.is_empty() && !is_system_meta_prompt(s));
                if let Some(p) = prompt {
                    human_prompt = true;
                    if first_prompt.is_none() {
                        first_prompt = Some(p.chars().take(80).collect());
                    }
                }
            }
            _ => {}
        }
        if cwd.is_none() {
            cwd = v.get("cwd").and_then(|c| c.as_str()).map(String::from);
        }
        if branch.is_none() {
            branch = v
                .get("gitBranch")
                .and_then(|b| b.as_str())
                .filter(|b| !b.is_empty() && *b != "HEAD")
                .map(String::from);
        }
        if title.is_some() && cwd.is_some() && branch.is_some() && human_prompt && forked.is_some()
        {
            break;
        }
    }

    // A fork/continuation file with nothing the user typed is upstream
    // bookkeeping, not a session anyone wants to see or resume.
    if fork_marker && !human_prompt {
        return None;
    }

    // Fork stubs omit cwd; backfill from a sibling session's cwd in the same
    // project dir so they still group under the right project.
    let project_path = cwd.or_else(|| dir_cwd.map(String::from))?;
    // Noise filters: scratch sessions in /tmp, and sessions with no human
    // content at all (memory summarizer runs, local-command-only sessions).
    if project_path == "/tmp" || project_path.starts_with("/tmp/") {
        return None;
    }
    let title = title.or(summary).or(first_prompt).or(ai_title)?;

    Some(Session {
        id,
        agent: "claude".into(),
        title,
        group_path: worktree_repo_root(&project_path).unwrap_or_else(|| project_path.clone()),
        project_path,
        branch,
        forked: forked.unwrap_or(false),
        background,
        fork_parent: None, // filled in from job state by the caller
        last_active: mtime,
        // All three filled in by `attach_source` — the transcript cannot know
        // them, which is the point of recording them elsewhere.
        source: None,
        source_label: None,
        source_model: None,
    })
}

/// Collapse a Claude Code worktree path to the repo it was cut from:
/// `<repo>/.claude/worktrees/<name>[/<sub>]` → `<repo>`. `/fork` can run its
/// child agent in a fresh worktree, which lands the transcript in a project
/// dir of its own — without this the fork row shows up under a stray
/// `<name>`/`src-tauri` group instead of next to the session it forked from.
fn worktree_repo_root(cwd: &str) -> Option<String> {
    let root = cwd.split("/.claude/worktrees/").next()?;
    if root == cwd || root.is_empty() {
        return None;
    }
    Some(root.to_string())
}

/* ---- Codex ---------------------------------------------------------------
 *
 * This replaces a deliberate placeholder. Until now `CodexSessions` in
 * `agents.rs` returned nothing, with a comment saying the on-disk format had
 * not been examined and that inventing a plausible path would fail in the one
 * way nobody can debug — indistinguishably from "you have no Codex sessions".
 *
 * It has now been examined, both ways in, and this is written from the two
 * files rather than from any documentation:
 *
 * > **2026-07-27, codex-cli 0.145.0.** `codex exec` wrote
 * > `~/.codex/sessions/2026/07/27/rollout-2026-07-27T20-22-44-019fa61a-39f4-7923-a717-215dd3b0aa58.jsonl`,
 * > opening with one `session_meta` record whose payload carried
 * > `session_id`, `cwd: "/home/admin/AI-OS"`, `originator: "codex_exec"`,
 * > `source: "exec"`, `cli_version` and `git: {commit_hash, branch: "master"}`.
 * > What the user typed appeared as
 * > `{"type":"event_msg","payload":{"type":"user_message","message":"…"}}`,
 * > and the model as `{"type":"turn_context","payload":{…,"model":"gpt-5.6-sol"}}`.
 * >
 * > The *interactive* TUI was then driven on a real pty (there is no headless
 * > way to ask it) and wrote
 * > `…/rollout-2026-07-27T21-32-09-019fa659-c884-7620-94fa-606596862c11.jsonl`
 * > in the same directory, with the same record types, differing only in
 * > `originator: "codex-tui"`, `source: "cli"` — and with no `git` key at all,
 * > because that session's cwd was `/tmp`. So `git` is optional and `cwd` is
 * > not.
 *
 * That second file is the one that mattered. `codex exec` alone would have
 * proved nothing about the mode aiterm actually launches, and assuming the two
 * shared a writer is exactly the sort of plausible-but-unchecked step
 * SESSION-MODEL.md opens by warning about. Corroborating it: `codex resume
 * --help` documents `--include-non-interactive`, "include non-interactive
 * sessions in the resume picker" — a flag that only needs to exist if both
 * kinds live in one store.
 *
 * What is deliberately **not** claimed here:
 *
 * - **Lineage.** `codex fork` exists, but nothing was found that records the
 *   pair, so every Codex row reports `forked: false` rather than a guess.
 * - **Liveness.** The roster is `claude agents --json`; there is no equivalent
 *   here, so a running Codex session wears no dot. That is a missing fact, not
 *   a wrong one.
 * - **The directory layout.** `<year>/<month>/<day>` is what this machine has,
 *   and the walk below does not depend on it — it descends a bounded few levels
 *   and takes whatever `.jsonl` files it finds, so a future version that flattens
 *   or re-nests the store keeps working.
 */
pub struct CodexProvider;

impl SessionProvider for CodexProvider {
    fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)> {
        let Some(root) = codex_sessions_root() else {
            return vec![];
        };
        let mut files = Vec::new();
        collect_jsonl(&root, 4, &mut files);
        files
            .into_iter()
            .filter_map(|p| parse_codex_rollout(&p).map(|s| (s, p)))
            .collect()
    }

    /// Codex names the file `rollout-<timestamp>-<session-id>.jsonl`, so the id
    /// is a suffix match rather than the stem. Matched against the whole
    /// `-<id>.jsonl` tail, not merely `contains`, so a session id that happens
    /// to be a substring of another cannot resolve to the wrong transcript.
    fn find_session_file(&self, session_id: &str) -> Option<std::path::PathBuf> {
        let root = codex_sessions_root()?;
        let mut files = Vec::new();
        collect_jsonl(&root, 4, &mut files);
        let tail = format!("-{session_id}.jsonl");
        files
            .into_iter()
            .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&tail)))
    }
}

fn codex_sessions_root() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".codex/sessions"))
}

/// Every `.jsonl` under `dir`, descending at most `depth` levels.
///
/// Bounded rather than unbounded so a symlink loop or someone's backup folder
/// dropped into the store cannot turn a sidebar refresh into an unbounded walk.
/// The real store is three levels deep; four leaves room for a re-nesting
/// without leaving the app unable to see sessions until it is rebuilt.
fn collect_jsonl(dir: &Path, depth: u32, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                collect_jsonl(&p, depth - 1, out);
            }
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

/// The facts a Codex rollout's head carries about its session.
#[derive(Default, Debug, PartialEq)]
pub struct CodexHead {
    pub id: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    /// First thing the user actually typed. `event_msg`/`user_message` is used
    /// rather than the `response_item` records with `role: "user"`, because
    /// those also carry everything Codex injects — the permissions block, the
    /// apps and skills catalogues, `<environment_context>` — and the first of
    /// them is never a human sentence.
    pub title: Option<String>,
    /// `"cli"` for the interactive TUI, `"exec"` for `codex exec`.
    pub source: Option<String>,
    pub model: Option<String>,
}

/// Read a rollout's head. Pure, over lines, so the two captured files can be
/// asserted against without a `~/.codex` on the machine running the tests.
pub fn parse_codex_head<'a>(lines: impl IntoIterator<Item = &'a str>) -> CodexHead {
    let mut head = CodexHead::default();
    for line in lines {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("session_meta") => {
                let p = v.get("payload").unwrap_or(&serde_json::Value::Null);
                head.id = p.get("session_id").and_then(|x| x.as_str()).map(String::from);
                head.cwd = p
                    .get("cwd")
                    .and_then(|x| x.as_str())
                    .filter(|c| !c.is_empty())
                    .map(String::from);
                head.source = p.get("source").and_then(|x| x.as_str()).map(String::from);
                head.branch = p
                    .pointer("/git/branch")
                    .and_then(|x| x.as_str())
                    .filter(|b| !b.is_empty() && *b != "HEAD")
                    .map(String::from);
            }
            Some("turn_context") if head.model.is_none() => {
                head.model = v
                    .pointer("/payload/model")
                    .and_then(|x| x.as_str())
                    .filter(|m| !m.is_empty())
                    .map(String::from);
            }
            Some("event_msg") if head.title.is_none() => {
                if v.pointer("/payload/type").and_then(|t| t.as_str()) != Some("user_message") {
                    continue;
                }
                head.title = v
                    .pointer("/payload/message")
                    .and_then(|m| m.as_str())
                    .map(|m| m.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|m| !m.is_empty())
                    .map(|m| m.chars().take(80).collect());
            }
            _ => {}
        }
    }
    head
}

/// One rollout file as a row, or `None` if it is not one worth showing.
///
/// The two rejections match `parse_session`'s, on purpose — two backends whose
/// lists obey different rules is a worse sidebar than either rule alone:
///
/// - **No prompt, no row.** A rollout with a `session_meta` and nothing the
///   user typed is a session that was opened and abandoned. There is nothing to
///   title it with and nothing in it to resume to.
/// - **Nothing under `/tmp`.** Scratch runs, same as for Claude Code.
fn parse_codex_rollout(path: &Path) -> Option<Session> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() == 0 {
        return None;
    }
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
    let reader = BufReader::new(File::open(path).ok()?);
    // 400 lines, matching parse_session. Everything wanted here is in the first
    // dozen; the bound is what stops a long conversation being read in full on
    // every sidebar refresh.
    let lines: Vec<String> = reader.lines().take(400).map_while(Result::ok).collect();
    let head = parse_codex_head(lines.iter().map(String::as_str));

    let id = head.id?;
    let title = head.title?;
    let project_path = head.cwd?;
    if project_path == "/tmp" || project_path.starts_with("/tmp/") {
        return None;
    }
    Some(Session {
        id,
        // Overwritten by the registry with the id of the backend that produced
        // this row — stated there, not here, so the two cannot disagree.
        agent: "codex".into(),
        title,
        group_path: worktree_repo_root(&project_path).unwrap_or_else(|| project_path.clone()),
        project_path,
        branch: head.branch,
        // Not guesses — unknowns. See the module note above: Codex lineage is
        // not recorded anywhere that has been found, and reporting `forked`
        // from a hunch would put a ⑂ on rows at random.
        forked: false,
        background: false,
        fork_parent: None,
        last_active: mtime,
        // Unlike a Claude row, a Codex row knows its own source: only Codex
        // writes these files. Set here so a Codex session started outside
        // aiterm still wears the right mark — `sources.json` only ever covers
        // sessions aiterm launched, and Codex will not take a minted id, so it
        // has no entry there even when aiterm did launch it.
        source: Some("codex".into()),
        source_label: Some("Codex".into()),
        source_model: head.model,
    })
}

#[derive(Serialize, Default)]
pub struct SessionStatus {
    pub exists: bool,
    /// e.g. "bypassPermissions", "acceptEdits", "plan", "default"
    pub permission_mode: Option<String>,
    pub mode: Option<String>,
}

/// Read the current mode lines from a Claude session jsonl. Mode changes are
/// appended over time, so the last occurrence in the file wins.
#[tauri::command]
pub fn session_status(session_id: String) -> SessionStatus {
    let Some(path) = find_session_file(&session_id) else {
        return SessionStatus::default();
    };
    let Ok(file) = File::open(&path) else {
        return SessionStatus::default();
    };
    let mut status = SessionStatus {
        exists: true,
        ..Default::default()
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // Cheap substring filter before JSON parsing.
        if !line.contains("\"permission-mode\"") && !line.contains("\"type\":\"mode\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("permission-mode") => {
                status.permission_mode = v
                    .get("permissionMode")
                    .and_then(|m| m.as_str())
                    .map(String::from)
            }
            Some("mode") => {
                status.mode = v.get("mode").and_then(|m| m.as_str()).map(String::from)
            }
            _ => {}
        }
    }
    status
}

/// How long trashed sessions are kept before the next delete purges them.
const TRASH_KEEP_DAYS: u64 = 7;

/// Delete a session: its transcript jsonl and task store move to
/// ~/.claude/trash (kept for TRASH_KEEP_DAYS as an undo safety net,
/// purged lazily on later deletes).
#[tauri::command]
pub fn session_delete(session_id: String) -> Result<(), String> {
    if session_id.contains('/') || session_id.contains("..") {
        return Err("invalid session id".into());
    }
    let path = find_session_file(&session_id).ok_or("session not found")?;
    let home = dirs::home_dir().ok_or("no home dir")?;
    let trash = home.join(".claude/trash");
    std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;

    // Lazy purge of old trash entries.
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(TRASH_KEEP_DAYS * 86400);
    if let Ok(entries) = std::fs::read_dir(&trash) {
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|m| m < cutoff)
                .unwrap_or(false);
            if old {
                let p = e.path();
                let _ = if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
        }
    }

    // Same filesystem (~/.claude), so rename is atomic and cheap. Rename
    // keeps the old mtime, which the purge above reads as age — reset it so
    // the entry gets its full keep window.
    let touch = |p: &std::path::Path| {
        if let Ok(f) = File::open(p) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    };
    let dest = trash.join(format!("{session_id}.jsonl"));
    std::fs::rename(&path, &dest).map_err(|e| e.to_string())?;
    touch(&dest);
    let tasks = home.join(".claude/tasks").join(&session_id);
    if tasks.is_dir() {
        let tasks_dest = trash.join(format!("{session_id}.tasks"));
        if std::fs::rename(&tasks, &tasks_dest).is_ok() {
            touch(&tasks_dest);
        }
    }
    // Claude Code keeps a second, independent record of a session under
    // `~/.claude/jobs/<short>/`, and nothing there follows the transcript. Left
    // behind, it is a ghost: `claude agents` goes on listing a session you
    // deleted, with its old state and last output, forever. A delete that
    // leaves the session listed elsewhere has not really deleted it.
    //
    // It goes to the trash like everything else rather than being removed, so
    // a restore puts it back and nothing here is one-way.
    if let Some(job) = find_job_dir(&home.join(".claude/jobs"), &session_id) {
        let job_dest = trash.join(format!("{session_id}.job"));
        if std::fs::rename(&job, &job_dest).is_ok() {
            touch(&job_dest);
        }
    }
    Ok(())
}

/// The job directory belonging to exactly this session, found by reading each
/// `state.json` — never by matching the directory's name.
///
/// The names happen to be the session's first uuid segment today, but matching
/// on that would mean deleting Claude Code's records on a coincidence. Reading
/// the `sessionId` inside is the only claim we can actually stand behind.
fn find_job_dir(jobs_root: &Path, session_id: &str) -> Option<std::path::PathBuf> {
    for job in std::fs::read_dir(jobs_root).ok()?.flatten() {
        let Ok(raw) = std::fs::read_to_string(job.path().join("state.json")) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if v.get("sessionId").and_then(|s| s.as_str()) == Some(session_id) {
            return Some(job.path());
        }
    }
    None
}

/// Where a trashed job directory belongs. Prefers the `daemonShort` recorded
/// inside it over re-deriving the name from the id.
fn job_dir_name(trashed: &Path, session_id: &str) -> String {
    std::fs::read_to_string(trashed.join("state.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.get("daemonShort")
                .and_then(|s| s.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| session_id.split('-').next().unwrap_or(session_id).to_string())
}

#[derive(Serialize)]
pub struct TrashedSession {
    pub id: String,
    pub title: String,
    pub project_path: String,
    pub deleted_at: u64,
}

fn trash_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude/trash"))
}

fn valid_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty() || session_id.contains('/') || session_id.contains("..") {
        return Err("invalid session id".into());
    }
    Ok(())
}

#[tauri::command]
pub fn trash_list() -> Vec<TrashedSession> {
    let Some(trash) = trash_dir() else {
        return vec![];
    };
    let Ok(rd) = std::fs::read_dir(&trash) else {
        return vec![];
    };
    let mut out: Vec<TrashedSession> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                return None;
            }
            let id = p.file_stem()?.to_string_lossy().to_string();
            let deleted_at = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            // parse_session rejects some transcripts (noise filters); trash
            // still lists those with a fallback label so nothing is invisible.
            let (title, project_path) = match parse_session(&p, None) {
                Some(s) => (s.title, s.project_path),
                None => (format!("session {}", &id[..8.min(id.len())]), String::new()),
            };
            Some(TrashedSession { id, title, project_path, deleted_at })
        })
        .collect();
    out.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
    out
}

/// Claude Code's project-dir flattening: path separators and dots become '-'.
fn flatten_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

#[tauri::command]
pub fn trash_restore(session_id: String) -> Result<(), String> {
    valid_id(&session_id)?;
    let trash = trash_dir().ok_or("no home dir")?;
    let src = trash.join(format!("{session_id}.jsonl"));
    if !src.exists() {
        return Err("session not in trash".into());
    }
    // The transcript's cwd decides which project dir it goes back to.
    let mut cwd: Option<String> = None;
    if let Ok(f) = File::open(&src) {
        for line in BufReader::new(f).lines().take(400).map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                    cwd = Some(c.to_string());
                    break;
                }
            }
        }
    }
    let cwd = cwd.ok_or("transcript has no cwd; can't pick a project dir")?;
    let home = dirs::home_dir().ok_or("no home dir")?;
    // Prefer the dir live sessions of this project already use; fall back to
    // the flattening convention for projects with no other sessions.
    let proj_dir = ClaudeProvider
        .scan_with_paths()
        .into_iter()
        .find(|(s, _)| s.project_path == cwd)
        .and_then(|(_, p)| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| home.join(".claude/projects").join(flatten_project_dir(&cwd)));
    std::fs::create_dir_all(&proj_dir).map_err(|e| e.to_string())?;
    std::fs::rename(&src, proj_dir.join(format!("{session_id}.jsonl")))
        .map_err(|e| e.to_string())?;
    let tasks_src = trash.join(format!("{session_id}.tasks"));
    if tasks_src.is_dir() {
        let _ = std::fs::rename(&tasks_src, home.join(".claude/tasks").join(&session_id));
    }
    let job_src = trash.join(format!("{session_id}.job"));
    if job_src.is_dir() {
        let jobs = home.join(".claude/jobs");
        let _ = std::fs::create_dir_all(&jobs);
        let _ = std::fs::rename(&job_src, jobs.join(job_dir_name(&job_src, &session_id)));
    }
    Ok(())
}

#[tauri::command]
pub fn trash_delete(session_id: String) -> Result<(), String> {
    valid_id(&session_id)?;
    let trash = trash_dir().ok_or("no home dir")?;
    std::fs::remove_file(trash.join(format!("{session_id}.jsonl"))).map_err(|e| e.to_string())?;
    let tasks = trash.join(format!("{session_id}.tasks"));
    if tasks.is_dir() {
        let _ = std::fs::remove_dir_all(tasks);
    }
    let job = trash.join(format!("{session_id}.job"));
    if job.is_dir() {
        let _ = std::fs::remove_dir_all(job);
    }
    Ok(())
}

#[tauri::command]
pub fn trash_empty() -> Result<(), String> {
    let trash = trash_dir().ok_or("no home dir")?;
    if let Ok(rd) = std::fs::read_dir(&trash) {
        for e in rd.flatten() {
            let p = e.path();
            let _ = if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_file(&p)
            };
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct PreviewMsg {
    pub role: String,
    pub text: String,
    pub at: Option<String>,
}

/// Tail of the human-visible conversation for a session — used by the
/// pre-resume preview pane. Returns the last few user/assistant text
/// messages, oldest first.
#[tauri::command]
pub fn session_preview(session_id: String) -> Vec<PreviewMsg> {
    const KEEP: usize = 12;
    const MAX_CHARS: usize = 700;
    let Some(path) = find_session_file(&session_id) else {
        return vec![];
    };
    let Ok(file) = File::open(&path) else {
        return vec![];
    };
    let mut out: std::collections::VecDeque<PreviewMsg> = std::collections::VecDeque::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // Cheap substring filter before JSON parsing. The Codex pair is listed
        // separately rather than loosened to `"type":"user`, which would start
        // matching `"type":"user_message"` in a Claude file too — the two
        // formats now share this function and the filter has to stay exact.
        let claude_shape =
            line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"");
        let codex_shape =
            line.contains("\"user_message\"") || line.contains("\"agent_message\"");
        if !claude_shape && !codex_shape {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        // A Codex rollout says what a turn was in `payload.type`, and puts the
        // text in `payload.message` as a plain string — no content blocks, no
        // sidechains, no injected tags. Handled here rather than in a second
        // function so the trimming, truncation and KEEP window below stay one
        // implementation: a preview pane that behaved differently per agent
        // would be a second thing to keep in step for no gain.
        if let Some(role) = codex_role(&v) {
            let text = v
                .pointer("/payload/message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            push_preview(&mut out, role, text, &v, KEEP, MAX_CHARS);
            continue;
        }
        let role = match v.get("type").and_then(|t| t.as_str()) {
            Some(r @ ("user" | "assistant")) => r.to_string(),
            _ => continue,
        };
        // Subagent traffic isn't part of the main conversation.
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        let mut text = String::new();
        match v.pointer("/message/content") {
            Some(serde_json::Value::String(s)) => text = s.clone(),
            Some(serde_json::Value::Array(blocks)) => {
                for b in blocks {
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                    }
                }
            }
            _ => {}
        }
        push_preview(&mut out, role, text, &v, KEEP, MAX_CHARS);
    }
    out.into()
}

/// `user` / `assistant` for a Codex `event_msg`, or `None` for anything else.
///
/// Codex names the assistant's turn `agent_message`, which is mapped to
/// `assistant` here rather than passed through: the role string reaches the
/// preview pane's CSS and its "you"/"claude" labelling, and a third value would
/// silently render unstyled.
fn codex_role(v: &serde_json::Value) -> Option<String> {
    if v.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
        return None;
    }
    match v.pointer("/payload/type").and_then(|t| t.as_str())? {
        "user_message" => Some("user".into()),
        "agent_message" => Some("assistant".into()),
        _ => None,
    }
}

/// Trim, drop, truncate and window one preview message. Shared by both formats
/// so the pane behaves identically whichever agent wrote the transcript.
fn push_preview(
    out: &mut std::collections::VecDeque<PreviewMsg>,
    role: String,
    text: String,
    v: &serde_json::Value,
    keep: usize,
    max_chars: usize,
) {
    let text = strip_system_tags(&text);
    if text.trim().is_empty() || (role == "user" && is_system_meta_prompt(&text)) {
        return;
    }
    let truncated = text.chars().count() > max_chars;
    let mut text: String = text.chars().take(max_chars).collect();
    if truncated {
        text.push('…');
    }
    let at = v.get("timestamp").and_then(|t| t.as_str()).map(String::from);
    out.push_back(PreviewMsg { role, text, at });
    if out.len() > keep {
        out.pop_front();
    }
}

#[derive(Serialize)]
pub struct SessionTask {
    pub id: String,
    pub subject: String,
    pub status: String,
    pub active_form: Option<String>,
    pub blocked_by: Vec<String>,
}

/// Task list for a session, parsed from the transcript. Claude Code has two
/// task systems: the newer TaskCreate/TaskUpdate tools (sequential ids) and
/// the older TodoWrite snapshots (last write wins). Whichever wrote later in
/// the file is the live one; the legacy per-task json dir is a last resort.
#[tauri::command]
pub fn session_tasks(session_id: String) -> Vec<SessionTask> {
    if let Some(path) = resolve_live_session_file(&session_id) {
        if let Ok(file) = File::open(&path) {
            let mut todo: Option<Vec<SessionTask>> = None;
            let mut todo_at = 0usize;
            let mut created: Vec<SessionTask> = Vec::new();
            let mut task_at = 0usize;
            for (n, line) in BufReader::new(file).lines().map_while(Result::ok).enumerate() {
                if !line.contains("\"TodoWrite\"")
                    && !line.contains("\"TaskCreate\"")
                    && !line.contains("\"TaskUpdate\"")
                {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array())
                else {
                    continue;
                };
                for b in blocks {
                    if b.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    let input = b.get("input");
                    let get = |k: &str| {
                        input
                            .and_then(|i| i.get(k))
                            .and_then(|x| x.as_str())
                            .map(String::from)
                    };
                    match b.get("name").and_then(|n| n.as_str()) {
                        Some("TodoWrite") => {
                            if let Some(todos) =
                                b.pointer("/input/todos").and_then(|t| t.as_array())
                            {
                                todo = Some(
                                    todos
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(i, t)| {
                                            Some(SessionTask {
                                                id: i.to_string(),
                                                subject: t
                                                    .get("content")?
                                                    .as_str()?
                                                    .to_string(),
                                                status: t
                                                    .get("status")
                                                    .and_then(|s| s.as_str())
                                                    .unwrap_or("pending")
                                                    .to_string(),
                                                active_form: t
                                                    .get("activeForm")
                                                    .and_then(|s| s.as_str())
                                                    .map(String::from),
                                                blocked_by: vec![],
                                            })
                                        })
                                        .collect(),
                                );
                                todo_at = n;
                            }
                        }
                        Some("TaskCreate") => {
                            if let Some(subject) = get("subject") {
                                created.push(SessionTask {
                                    id: (created.len() + 1).to_string(),
                                    subject,
                                    status: "pending".into(),
                                    active_form: get("activeForm"),
                                    blocked_by: vec![],
                                });
                                task_at = n;
                            }
                        }
                        Some("TaskUpdate") => {
                            if let Some(tid) = get("taskId") {
                                if let Some(t) = created.iter_mut().find(|t| t.id == tid) {
                                    if let Some(s) = get("status") {
                                        t.status = s;
                                    }
                                    if let Some(s) = get("subject") {
                                        t.subject = s;
                                    }
                                    if let Some(a) = get("activeForm") {
                                        t.active_form = Some(a);
                                    }
                                    task_at = n;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if !created.is_empty() && (todo.is_none() || task_at > todo_at) {
                return created.into_iter().filter(|t| t.status != "deleted").collect();
            }
            if let Some(tasks) = todo {
                return tasks;
            }
        }
    }
    session_tasks_dir(&session_id)
}

/// Legacy fallback: per-task json files in ~/.claude/tasks/<session-id>/.
fn session_tasks_dir(session_id: &str) -> Vec<SessionTask> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let dir = home.join(".claude/tasks").join(session_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut tasks: Vec<SessionTask> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some(SessionTask {
                id: v.get("id")?.as_str()?.to_string(),
                subject: v.get("subject")?.as_str()?.to_string(),
                status: v
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("pending")
                    .to_string(),
                active_form: v
                    .get("activeForm")
                    .and_then(|s| s.as_str())
                    .map(String::from),
                blocked_by: v
                    .get("blockedBy")
                    .and_then(|b| b.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .filter(|t| t.status != "deleted")
        .collect();
    tasks.sort_by_key(|t| t.id.parse::<u64>().unwrap_or(u64::MAX));
    tasks
}

#[derive(Serialize)]
pub struct AgentRun {
    /// tool_use id of the spawning call.
    pub id: String,
    pub agent_type: String,
    pub description: String,
    /// "running" | "done"
    pub status: String,
    pub started_at: Option<String>,
    /// Final report snippet (or completion summary for background agents).
    pub result: Option<String>,
}

/// Text of a tool_result block (plain string or text-block array).
fn tool_result_text(block: &serde_json::Value) -> String {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn xml_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim())
}

fn snippet(text: &str, max: usize) -> String {
    let s: String = text.trim().chars().take(max).collect();
    if text.trim().chars().count() > max {
        s + "…"
    } else {
        s
    }
}

/// Subagents this session spawned (Agent/Task tool calls), with liveness:
/// a sync agent is done when its tool_result lands; a background agent's
/// tool_result is just "Async agent launched…" and completion arrives later
/// as a <task-notification> carrying the original tool-use-id.
#[tauri::command]
pub fn session_agents(session_id: String) -> Vec<AgentRun> {
    let Some(path) = resolve_live_session_file(&session_id) else {
        return vec![];
    };
    let Ok(file) = File::open(&path) else {
        return vec![];
    };
    let mut runs: Vec<AgentRun> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("tool_use") && !line.contains("task-notification") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        match v.pointer("/message/content") {
            Some(serde_json::Value::Array(blocks)) => {
                for b in blocks {
                    match b.get("type").and_then(|t| t.as_str()) {
                        Some("tool_use")
                            if matches!(
                                b.get("name").and_then(|n| n.as_str()),
                                Some("Agent") | Some("Task")
                            ) =>
                        {
                            let Some(id) = b.get("id").and_then(|i| i.as_str()) else {
                                continue;
                            };
                            let input = b.get("input");
                            let get = |k: &str| {
                                input
                                    .and_then(|i| i.get(k))
                                    .and_then(|x| x.as_str())
                                    .map(String::from)
                            };
                            let description = get("description")
                                .or_else(|| get("prompt").map(|p| snippet(&p, 60)))
                                .unwrap_or_else(|| "agent".into());
                            index.insert(id.to_string(), runs.len());
                            runs.push(AgentRun {
                                id: id.to_string(),
                                agent_type: get("subagent_type")
                                    .unwrap_or_else(|| "general".into()),
                                description,
                                status: "running".into(),
                                started_at: v
                                    .get("timestamp")
                                    .and_then(|t| t.as_str())
                                    .map(String::from),
                                result: None,
                            });
                        }
                        Some("tool_result") => {
                            let Some(&i) = b
                                .get("tool_use_id")
                                .and_then(|i| i.as_str())
                                .and_then(|i| index.get(i))
                            else {
                                continue;
                            };
                            let text = tool_result_text(b);
                            if text.starts_with("Async agent launched") {
                                continue; // still running in the background
                            }
                            runs[i].status = "done".into();
                            runs[i].result = Some(snippet(&text, 240));
                        }
                        _ => {}
                    }
                }
            }
            // Background completion notification (plain-string user record).
            Some(serde_json::Value::String(text)) if text.contains("<task-notification>") => {
                let Some(&i) = xml_tag(text, "tool-use-id").and_then(|id| index.get(id)) else {
                    continue;
                };
                runs[i].status = "done".into();
                // The notification carries the agent's full report in
                // <result>; fall back to the one-line <summary>.
                if let Some(r) = xml_tag(text, "result").or_else(|| xml_tag(text, "summary")) {
                    runs[i].result = Some(snippet(r, 600));
                }
            }
            _ => {}
        }
    }
    runs
}

#[derive(Serialize)]
pub struct Artifact {
    pub path: String,
    pub tool: String,
    /// ISO timestamp of the last touch.
    pub at: String,
}

fn mtime_of(path: &Path) -> Option<u64> {
    Some(
        std::fs::metadata(path)
            .ok()?
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis() as u64,
    )
}

/// Locate the transcript for a bare session id. Fast path is the exact
/// `<id>.jsonl`; if that's gone, Claude Code may have renamed it to
/// `<id>.orphaned-<ts>-<hash>.jsonl` (compaction/supersede), so fall back to
/// the newest such variant rather than returning nothing (which would blank
/// the tasks/agents/artifacts panels).
/// The transcript for `session_id`, whichever agent owns it.
///
/// Asks each backend in turn rather than deciding from the id itself: a session
/// id is opaque, and a rule for telling one agent's ids from another's would be
/// a guess that breaks the first time an id format changes. The owner is the
/// backend that can find the file.
fn find_session_file(session_id: &str) -> Option<std::path::PathBuf> {
    crate::agents::find_session_file_in(&crate::agents::backends(), session_id)
}

/// Claude Code's own lookup: `~/.claude/projects/<project>/<id>.jsonl`, falling
/// back to the newest `<id>.orphaned-…` when the live file has been retired.
fn claude_session_file(session_id: &str) -> Option<std::path::PathBuf> {
    let root = dirs::home_dir()?.join(".claude/projects");
    let exact = format!("{session_id}.jsonl");
    if let Ok(projects) = std::fs::read_dir(&root) {
        for project in projects.flatten() {
            let p = project.path().join(&exact);
            if p.exists() {
                return Some(p);
            }
        }
    }
    let orphaned_prefix = format!("{session_id}.orphaned-");
    let mut best: Option<(std::path::PathBuf, u64)> = None;
    for project in std::fs::read_dir(&root).ok()?.flatten() {
        let Ok(files) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            if name.starts_with(&orphaned_prefix) && name.ends_with(".jsonl") {
                let m = mtime_of(&p).unwrap_or(0);
                if best.as_ref().is_none_or(|(_, bm)| m > *bm) {
                    best = Some((p, m));
                }
            }
        }
    }
    best.map(|(p, _)| p)
}

/// Resolve the transcript to actually read for a UI-pinned session id. A live
/// `<id>.jsonl` IS the session: an explicit `--fork-session` leaves the
/// original intact and independently resumable, so a healthy pinned id must
/// never be redirected to a fork sibling — that silently swapped the
/// original's frozen context for the fork's. Only when the pinned file was
/// retired (/clear or compact renames it to `<id>.orphaned-…`) does the
/// conversation truly continue elsewhere: then follow the `bridgeSessionId`
/// to the newest live sibling in the family (continuations keep the same
/// cwd → same project dir), which keeps the agents/tasks panels in sync.
fn resolve_live_session_file(session_id: &str) -> Option<std::path::PathBuf> {
    let start = find_session_file(session_id)?;
    if !start
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains(".orphaned-"))
    {
        return Some(start);
    }
    let Some(bridge) = read_bridge_id(&start) else {
        return Some(start);
    };
    let project_dir = start.parent()?;
    let mut best = start.clone();
    let mut best_mtime = mtime_of(&start).unwrap_or(0);
    if let Ok(files) = std::fs::read_dir(project_dir) {
        for f in files.flatten() {
            let p = f.path();
            if p == start || p.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            // A live fork is a normal <id>.jsonl; never pick an orphaned file
            // as the "newest" winner.
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().contains(".orphaned-"))
            {
                continue;
            }
            if read_bridge_id(&p).as_deref() == Some(bridge.as_str()) {
                let m = mtime_of(&p).unwrap_or(0);
                if m > best_mtime {
                    best = p;
                    best_mtime = m;
                }
            }
        }
    }
    Some(best)
}

/// Resolve a UI-pinned session id to an id that `claude --resume` can actually
/// open *right now*. Follows the same logic as the panels
/// (`resolve_live_session_file`) but returns the id (filename stem) and refuses
/// to hand back an orphaned/superseded transcript. When a session is `/clear`ed
/// or compacted, Claude Code retires the original `<id>.jsonl` (deletes it, or
/// renames it to `<id>.orphaned-…`); `claude --resume <that-id>` then dies with
/// "no conversation found", leaving a black pane. This returns `Some(live_id)`
/// pointing at the surviving continuation in the family, or `None` when
/// nothing resumable is left — so the UI can say so instead of launching a
/// doomed resume. A live `<id>.jsonl` resolves to itself — forking never
/// retires the original, so a forked parent stays resumable at its own point.
#[tauri::command]
pub fn resolve_resumable_id(session_id: String) -> Option<String> {
    let path = resolve_live_session_file(&session_id)?;
    // A resumable transcript is a plain `<id>.jsonl`. If resolution could only
    // land on an orphaned remnant, there is nothing `claude` can resume.
    if path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains(".orphaned-"))
    {
        return None;
    }
    // Existing on disk is not the same as resumable. A `/fork` stub is a real
    // file with a title and no conversation, and `claude --resume` rejects it
    // with the same "No conversation found" it gives for a missing file. Saying
    // so here — before anything is stopped — is what keeps a failed resume from
    // costing a running session.
    if !has_conversation(&path) {
        return None;
    }
    Some(path.file_stem()?.to_string_lossy().into_owned())
}

/// The session that took over `session_id`'s conversation by migrating to the
/// daemon, if one has. `None` is the normal answer.
///
/// Opening Claude Code's agents view — left arrow, on an empty prompt — moves
/// the running conversation to the daemon. What lands on disk is a *new*
/// transcript under a new session id: the original stops at that instant and
/// never moves again, while the pty in the tab goes on rendering the child. A
/// tab pinned to the parent then shows live text over dead panels — its clock
/// stops and Agents/Tasks/Artifacts read a file nothing is writing.
///
/// Nothing in the job state links the two. A migrated job records
/// `interactiveLineage` but no `forkParentSessionId`, so `fork_parent_map`
/// cannot see it. The link is in the transcript, at message level: the child
/// carries copied history whose `parentUuid`/`logicalParentUuid` values are
/// `uuid`s of records in the parent. Measured against a specimen captured
/// 2026-07-26 — 67 of the child's 77 `parentUuid`s resolved into the parent,
/// and `sessionKind: "bg"` appeared throughout the child and nowhere in the
/// parent.
///
/// Both halves are required, and neither is sufficient. `sessionKind: "bg"`
/// alone matches every background agent in the project. UUID overlap alone
/// matches an ordinary `--fork-session` branch — which must never re-key a
/// tab, because forking leaves the parent independently resumable and still
/// running, and that parent is what the tab actually holds.
/// Frontend-to-journal logging. The webview's console goes nowhere in a
/// release build; errors the UI swallows (a failed invoke, a rejected promise)
/// were invisible, which is how a dead code path survives testing. Low volume:
/// callers log outcomes and errors, not chatter.
#[tauri::command]
pub fn ui_log(msg: String) {
    eprintln!("[aiterm-ui] {msg}");
}

#[tauri::command]
pub fn session_migrated_to(session_id: String) -> Option<String> {
    let out = session_migrated_to_inner(&session_id);
    // TEMP diagnostics for the missed re-key (2026-07-27): journal-visible
    // trace of every poll. Remove once the frontend path is proven.
    eprintln!("[aiterm] session_migrated_to({session_id}) -> {out:?}");
    out
}

fn session_migrated_to_inner(session_id: &str) -> Option<String> {
    let parent = find_session_file(session_id)?;
    if parent
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains(".orphaned-"))
    {
        return None; // retired transcripts are resolve_live_session_file's job
    }
    let parent_mtime = mtime_of(&parent).unwrap_or(0);
    let dir = parent.parent()?;

    let mut best: Option<(u64, String)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path == parent || path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let name = path.file_name()?.to_string_lossy().into_owned();
        if name.contains(".orphaned-") {
            continue;
        }
        // A child cannot predate the instant its parent stopped.
        let m = mtime_of(&path).unwrap_or(0);
        if m < parent_mtime {
            continue;
        }
        if best.as_ref().is_some_and(|(bm, _)| m <= *bm) {
            continue; // already holding a newer candidate
        }
        let Some((links, is_bg)) = read_lineage_links(&path) else {
            continue;
        };
        if !is_bg || links.is_empty() || !file_has_any_uuid(&parent, &links) {
            continue;
        }
        best = Some((m, path.file_stem()?.to_string_lossy().into_owned()));
    }
    best.map(|(_, id)| id)
}

/// The uuids a transcript claims as its ancestry, plus whether it runs under
/// the daemon. Reads only the head of the file — copied history sits at the
/// front, so scanning a multi-megabyte tail to re-learn the same answer is
/// waste.
fn read_lineage_links(path: &Path) -> Option<(std::collections::HashSet<String>, bool)> {
    const MAX_RECORDS: usize = 500;
    const MAX_LINKS: usize = 64;
    let file = File::open(path).ok()?;
    let mut links = std::collections::HashSet::new();
    let mut is_bg = false;
    for line in BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .take(MAX_RECORDS)
    {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("sessionKind").and_then(|k| k.as_str()) == Some("bg") {
            is_bg = true;
        }
        for field in ["parentUuid", "logicalParentUuid"] {
            if let Some(u) = v.get(field).and_then(|u| u.as_str()) {
                if links.len() < MAX_LINKS {
                    links.insert(u.to_string());
                }
            }
        }
    }
    Some((links, is_bg))
}

/// Whether any record in `path` carries one of `uuids` as its own `uuid`.
/// Streams and stops at the first hit — the parent can be tens of megabytes.
fn file_has_any_uuid(path: &Path, uuids: &std::collections::HashSet<String>) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    BufReader::new(file).lines().map_while(Result::ok).any(|l| {
        serde_json::from_str::<serde_json::Value>(&l)
            .ok()
            .and_then(|v| v.get("uuid")?.as_str().map(|u| uuids.contains(u)))
            .unwrap_or(false)
    })
}

/// Whether a transcript holds an actual exchange, as opposed to metadata only.
/// Message records are what `claude --resume` looks for; titles and agent names
/// are not enough.
fn has_conversation(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    BufReader::new(file).lines().map_while(Result::ok).any(|l| {
        serde_json::from_str::<serde_json::Value>(&l)
            .ok()
            .and_then(|v| v.get("type")?.as_str().map(|t| t == "user" || t == "assistant"))
            .unwrap_or(false)
    })
}

/// Session ids that currently have a live Claude Code process, read from
/// `/proc`. A session counts as running if some process names it via
/// `--session-id <id>` or `--resume <id|/path/<id>.jsonl>`. The UI uses this to
/// decide fork-vs-resume: a plain `claude --resume` fails on a session that's
/// still running (leaving a black pane), so those must fork; everything else
/// resumes in place, which avoids minting a duplicate forked transcript.
/// Short session ids (first UUID segment) of sessions the Claude Code daemon
/// currently has live, read from its per-session sockets under
/// /tmp/cc-daemon-*/<hash>/{rv,pty}/<shortid>.sock. Covers background agents,
/// which don't appear in /proc cmdlines.
fn daemon_live_session_shortids() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(tmp) = std::fs::read_dir("/tmp") else {
        return out;
    };
    for entry in tmp.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("cc-daemon-")
        {
            continue;
        }
        let Ok(hashes) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        for hash in hashes.flatten() {
            for sub in ["rv", "pty"] {
                let Ok(socks) = std::fs::read_dir(hash.path().join(sub)) else {
                    continue;
                };
                for s in socks.flatten() {
                    let name = s.file_name();
                    if let Some(stem) = name.to_string_lossy().strip_suffix(".sock") {
                        if !stem.is_empty() {
                            out.push(stem.to_string());
                        }
                    }
                }
            }
        }
    }
    out
}

#[tauri::command]
pub fn running_session_ids() -> Vec<String> {
    let mut ids = std::collections::HashSet::new();
    // Background agents (Claude Code's `/fork`, `--bg`) run under a daemon and
    // never name their session in /proc — but the daemon opens a socket per
    // live session at /tmp/cc-daemon-*/<hash>/{rv,pty}/<shortid>.sock, where
    // shortid is the FIRST segment of the session UUID. Collect those short
    // ids; the resume path matches them by prefix. Without this, resuming a
    // bg-agent fork falls through to plain `claude --resume`, which errors
    // ("currently running as a background agent … add --fork-session").
    ids.extend(daemon_live_session_shortids());
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return ids.into_iter().collect();
    };
    for entry in procs.flatten() {
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        for (i, a) in args.iter().enumerate() {
            if a == "--session-id" || a == "--resume" {
                if let Some(id) = args.get(i + 1).and_then(|v| extract_session_id(v)) {
                    ids.insert(id);
                }
            }
        }
    }
    ids.into_iter().collect()
}

/// Pull a session UUID out of a `--session-id`/`--resume` value, which is
/// either a bare id or a path like `/…/<id>.jsonl` (possibly `.orphaned-…`).
fn extract_session_id(val: &str) -> Option<String> {
    let stem = Path::new(val).file_stem()?.to_string_lossy().into_owned();
    let candidate = stem.split(".orphaned-").next().unwrap_or(&stem);
    if candidate.len() == 36 && candidate.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// Every session the Claude Code daemon currently holds — background agents
/// AND interactive ones. This is what "this session is alive right now" means,
/// and it is not the same question as "aiterm has a tab open for it".
///
/// Those two were conflated, and after a background-mode resume they point at
/// different rows: the tab runs `--resume <parent>` while the conversation
/// moves to a new bg session id. Keying the live dot on tab ownership put the
/// badge on a frozen snapshot and offered Delete on the session that was
/// actually running.
///
/// One more distinction, learned the same way: an *interactive* roster entry
/// whose conversation has migrated to the daemon is a renderer, not a
/// conversation. The client process stays alive under the old id for as long
/// as the tab is open, so the roster reports it forever — and its row wore a
/// green dot over a transcript nothing will ever write again. If the same
/// linkage the tab re-key trusts says the conversation moved, the old id is
/// not "alive" in any sense the sidebar should report. Background entries are
/// never filtered: the migrated-to session IS the live one.
///
/// Cost note: the migration scan is mtime-gated, so for a healthy interactive
/// session (its own transcript newest in the dir) it rejects every candidate
/// without reading them.
#[tauri::command]
pub fn live_session_ids() -> Vec<String> {
    read_roster()
        .into_iter()
        .filter(|e| e.background || session_migrated_to_inner(&e.session_id).is_none())
        .map(|e| e.session_id)
        .collect()
}

/// One live entry from `claude agents --json`.
#[derive(Clone)]
pub struct RosterEntry {
    pub session_id: String,
    /// Absent for sessions the daemon holds without a client process of their
    /// own — those can't be signalled, only stopped from the agents view.
    pub pid: Option<u32>,
    pub background: bool,
}

/// How long a roster reading may be reused.
///
/// Asking costs a whole `claude` process — measured at ~0.26s wall and a peak
/// around 300 MB RSS on 2026-07-27 — and several commands ask in quick
/// succession. Resuming a session alone calls `unstoppable_session_ids` and
/// then `stop_session`, with the sidebar's own poll landing in between.
///
/// Two seconds is chosen to collapse those bursts and nothing more. It is far
/// below any interval a human would notice a dot being stale over, and the one
/// caller that must not tolerate staleness — the stop loop, where a cached
/// "still running" is indistinguishable from a stop that failed — reads
/// through `read_roster_fresh` instead.
const ROSTER_TTL: std::time::Duration = std::time::Duration::from_secs(2);

static ROSTER: crate::cache::TtlCache<Vec<RosterEntry>> =
    crate::cache::TtlCache::new(ROSTER_TTL);

/// The roster, minus finished sessions. `claude agents --json` keeps reporting
/// a session with `state: "done"`, so "appears in the roster" is not the same
/// question as "is running" — counting those made dead sessions look alive and
/// suppressed Resume on rows that were perfectly resumable.
///
/// May be up to `ROSTER_TTL` old. Use `read_roster_fresh` where the answer is
/// being used to decide whether something you just did worked.
pub fn read_roster() -> Vec<RosterEntry> {
    ROSTER.get(read_roster_uncached)
}

/// Ask Claude Code again, whatever is cached, and reseed the cache with the
/// answer so readers right behind this one get the new state rather than the
/// old one.
pub fn read_roster_fresh() -> Vec<RosterEntry> {
    ROSTER.refresh(read_roster_uncached)
}

/// Forget the cached roster. Called when something is known to have changed it,
/// so the sidebar does not spend up to `ROSTER_TTL` showing a session as
/// running after it has been stopped.
pub fn invalidate_roster() {
    ROSTER.invalidate();
}

fn read_roster_uncached() -> Vec<RosterEntry> {
    let Ok(out) = std::process::Command::new("claude")
        .args(["agents", "--json"])
        .output()
    else {
        return Vec::new();
    };
    let Ok(list) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else {
        return Vec::new();
    };
    list.iter()
        .filter(|a| a.get("state").and_then(|s| s.as_str()) != Some("done"))
        .filter_map(|a| {
            let session_id = a.get("sessionId")?.as_str()?.to_owned();
            let pid = a.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32);
            // A pid the roster still lists but /proc doesn't know is a stale
            // entry; treat it as gone rather than as an unstoppable session.
            let pid = pid.filter(|&p| crate::pty::pid_alive(p));
            if a.get("pid").is_some() && pid.is_none() {
                return None;
            }
            Some(RosterEntry {
                session_id,
                pid,
                background: a.get("kind").and_then(|k| k.as_str()) == Some("background"),
            })
        })
        .collect()
}

/// Stop a running session so it can be resumed in place.
///
/// This is the shell workflow: you don't branch a copy to get back into a
/// conversation, you close the one that's running and `claude --resume` it.
/// `--resume` refuses a session that's currently live, so the stop has to
/// actually complete before the resume is spawned — hence the verified kill
/// and the error return rather than a fire-and-forget signal.
/// Off the main thread: this signals, then polls the roster for up to five
/// seconds. Run inline it would freeze the window for that whole time.
#[tauri::command]
pub async fn stop_session(session_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || stop_session_blocking(session_id))
        .await
        .map_err(|e| e.to_string())?
}

fn stop_session_blocking(session_id: String) -> Result<(), String> {
    // Fresh throughout. Every read on this path is load-bearing: a stale entry
    // would have us signal a pid that is already gone, and a stale *absence*
    // would report a stop that never happened and launch a `--resume` doomed
    // to be refused.
    let Some(entry) = read_roster_fresh()
        .into_iter()
        .find(|e| e.session_id == session_id)
    else {
        return Ok(()); // already gone — resuming is safe
    };
    // Signal the pid the roster gave us, when it gave us one. For an
    // interactive session that pid is the real `claude` process. For a
    // *background* agent it is not: the roster reports a `bg-spare` helper
    // parented to the daemon, while the conversation runs under a different
    // process carrying a different session id. Killing it there is a no-op.
    if let Some(pid) = entry.pid {
        crate::pty::kill_tree(pid, std::time::Duration::from_millis(1500));
    }
    // So success is defined by the roster, never by the pid dying: the session
    // is stopped when Claude Code stops listing it. Anything else would report
    // a stop that didn't happen and then launch a `--resume` doomed to refuse.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !read_roster_fresh().iter().any(|e| e.session_id == session_id) {
            return Ok(());
        }
        // Each pass already costs a process spawn of its own (~0.26s), so the
        // sleep is what the loop period is *on top of* that, not the period
        // itself. 250ms made this the heaviest thing the app ever did: up to
        // ten `claude` processes inside the five-second window, for a question
        // whose answer changes once.
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
    Err(
        "Claude Code still lists this session as running — it's held by the daemon, \
         so stop it from `claude agents` and try again."
            .into(),
    )
}

/// Full session UUIDs of live *background* agents only. A session on this list
/// is held by the daemon with no tab of ours, so `--resume` on it exits with
/// "…add --fork-session to branch off a copy" — resume says so rather than
/// launching a doomed one.
#[tauri::command]
pub fn bg_agent_session_ids() -> Vec<String> {
    read_roster()
        .into_iter()
        .filter(|e| e.background)
        .map(|e| e.session_id)
        .collect()
}

/// Sessions aiterm cannot reliably stop, so resume must not act as if it can.
/// `stop_session` already reports this, but only after polling for five
/// seconds — and by then the resume path has closed the tab it was going to
/// reuse, so the failure costs a tab and gains nothing. Asking first costs one
/// roster read.
///
/// Two ways a session lands here, and it takes both to cover the ground:
///
/// - **No pid.** The daemon holds it with no client process of its own, so
///   there is nothing to signal.
/// - **`background`.** It may well report a live pid — a `--fork-session`
///   process really is running behind it — but per `stop_session`, the pid the
///   roster gives for a background agent can be a `bg-spare` helper parented to
///   the daemon rather than the conversation, and killing that is a no-op.
///
/// Filtering on either one alone was tried and is wrong: a roster observed on
/// 2026-07-26 reported *every* entry with a live pid, including a background
/// one, so a `pid.is_none()` test matched nothing at all.
#[tauri::command]
pub fn unstoppable_session_ids() -> Vec<String> {
    read_roster()
        .into_iter()
        .filter(|e| e.background || e.pid.is_none())
        .map(|e| e.session_id)
        .collect()
}

/// Files this session created or modified, newest first — parsed from
/// Write/Edit/NotebookEdit tool calls in the transcript.
#[tauri::command]
pub fn session_artifacts(session_id: String) -> Vec<Artifact> {
    let Some(path) = resolve_live_session_file(&session_id) else {
        return vec![];
    };
    let Ok(file) = File::open(&path) else {
        return vec![];
    };
    const TOOLS: [&str; 4] = ["Write", "Edit", "NotebookEdit", "MultiEdit"];
    let mut latest: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("file_path") || !line.contains("tool_use") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            let Some(tool) = block.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            if !TOOLS.contains(&tool) {
                continue;
            }
            if let Some(fp) = block.pointer("/input/file_path").and_then(|f| f.as_str()) {
                latest.insert(fp.to_string(), (tool.to_string(), ts.clone()));
            }
        }
    }
    let mut artifacts: Vec<Artifact> = latest
        .into_iter()
        .map(|(path, (tool, at))| Artifact { path, tool, at })
        .collect();
    artifacts.sort_by(|a, b| b.at.cmp(&a.at));
    artifacts
}

#[derive(Serialize, Default)]
pub struct ModelChoice {
    /// Full id as recorded, e.g. "claude-opus-5". None before the first reply.
    pub model: Option<String>,
    /// "low" | "medium" | "high" | "xhigh" | "max" | … as recorded.
    pub effort: Option<String>,
    /// Timestamp of the record these came from. Lets the UI tell "no turn has
    /// run since you clicked" from "a turn ran and this is what it used" —
    /// the difference between a pending request and a settled fact.
    pub at: Option<String>,
    /// How full the context window is, from the last main-chain reply's
    /// `usage`: input + cache reads + cache writes + output. Sidechain records
    /// are skipped — a subagent has its own window, and its numbers would
    /// randomly overwrite the conversation's. None before the first reply.
    pub context_tokens: Option<u64>,
}

/// What model and effort the session last actually ran with.
///
/// Read from the transcript rather than tracked in the app: claude records the
/// model and effort on every assistant record, so the file is the authority and
/// stays right no matter who changed it — the pill, a `/model` typed into the
/// terminal, or a flag on launch. Nothing to keep in sync.
///
/// Only the tail is read; these files reach several MB.
#[tauri::command]
pub fn session_model(session_id: String) -> ModelChoice {
    let Some(path) = find_session_file(&session_id) else {
        return ModelChoice::default();
    };
    const TAIL: u64 = 256 * 1024;
    let Ok(mut file) = File::open(&path) else {
        return ModelChoice::default();
    };
    let Ok(meta) = file.metadata() else {
        return ModelChoice::default();
    };
    let start = meta.len().saturating_sub(TAIL);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return ModelChoice::default();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return ModelChoice::default();
    }
    // An arbitrary offset can split a line or a UTF-8 char — decode lossily and
    // drop the first partial line, as the bridge-id reader does.
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        lines.next();
    }
    parse_model_choice(lines)
}

/// The scan behind [`session_model`], separated so the record-shape rules can
/// be tested without a real transcript on disk.
fn parse_model_choice<'a>(lines: impl Iterator<Item = &'a str>) -> ModelChoice {
    let mut out = ModelChoice::default();
    for line in lines {
        if !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        // Last one wins: a model change mid-session leaves both in the file.
        // `<synthetic>` marks a record the CLI wrote itself rather than a real
        // reply — it names no model, so taking it would blank the pill.
        if let Some(m) = v
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
            .filter(|m| !m.starts_with('<'))
        {
            out.model = Some(m.to_string());
        }
        if let Some(e) = v.get("effort").and_then(|e| e.as_str()) {
            out.effort = Some(e.to_string());
        }
        if let Some(t) = v.get("timestamp").and_then(|t| t.as_str()) {
            out.at = Some(t.to_string());
        }
        if v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true) {
            if let Some(usage) = v.pointer("/message/usage") {
                let tok = |key: &str| usage.get(key).and_then(|n| n.as_u64()).unwrap_or(0);
                let total = tok("input_tokens")
                    + tok("cache_read_input_tokens")
                    + tok("cache_creation_input_tokens")
                    + tok("output_tokens");
                if total > 0 {
                    out.context_tokens = Some(total);
                }
            }
        }
    }
    out
}

/// The permission mode Claude Code would start a *new* session in here, read
/// from its own config chain — project-local first, then the user's.
///
/// This exists because permission mode is session state, not config state: a
/// resumed session replays whatever mode it last recorded, and `defaultMode`
/// only ever applies to new ones. Resuming through aiterm passed no flag, so a
/// session that had drifted to `acceptEdits` could never be lifted back to the
/// mode the config asks for — it stayed drifted forever. Passing the config's
/// answer on resume is what makes an aiterm resume behave like a fresh start.
///
/// Returns `None` when nothing is configured, or when the configured value
/// isn't one the CLI accepts — passing an unknown mode makes `claude` exit, and
/// a terminal that dies on open is worse than a permission prompt.
#[tauri::command]
pub fn claude_permission_mode(project_path: String) -> Option<String> {
    const ACCEPTED: [&str; 6] = [
        "acceptEdits",
        "auto",
        "bypassPermissions",
        "manual",
        "dontAsk",
        "plan",
    ];
    let home = dirs::home_dir()?;
    let project = std::path::Path::new(&project_path);
    // Most specific wins, matching how Claude Code layers its own settings.
    let candidates = [
        project.join(".claude/settings.local.json"),
        project.join(".claude/settings.json"),
        home.join(".claude/settings.json"),
    ];
    for path in candidates {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let Some(mode) = v
            .get("permissions")
            .and_then(|p| p.get("defaultMode"))
            .and_then(|m| m.as_str())
        else {
            continue;
        };
        return ACCEPTED.contains(&mode).then(|| mode.to_string());
    }
    None
}

fn user_settings_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude/settings.json"))
}

/// The `model` key in ~/.claude/settings.json — Claude Code's global default
/// for new sessions. None when unset.
#[tauri::command]
pub fn claude_model_default() -> Option<String> {
    let raw = std::fs::read_to_string(user_settings_path()?).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    v.get("model")?.as_str().map(|s| s.to_string())
}

/// Put the global `model` default back to `previous` after a `/model` command
/// has changed it.
///
/// Typing `/model <name>` at claude's prompt does not just retarget the running
/// session — it writes that name into ~/.claude/settings.json as the default
/// for every new session, in every project. Verified by driving a real PTY:
/// the key went from `opus` to `haiku` and stayed there. `/effort` does not do
/// this; only model.
///
/// aiterm's pill is a per-session control, so it undoes that half. Waits for
/// the CLI to actually write (it does so a moment after the command lands)
/// before restoring, otherwise we would race it and lose. Returns whether a
/// restore was needed. Only ever touches the one key, re-reading the file first
/// so anything else written meanwhile survives.
/// Async, and the waiting happens on a blocking-pool thread. Tauri runs a
/// plain `fn` command on the main thread, so the first version of this froze
/// the whole window while it polled — worst case the full timeout, which is
/// exactly what happens when you pick the model that is already your default
/// and the file therefore never changes.
#[tauri::command]
pub async fn restore_claude_model_default(previous: Option<String>) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || restore_model_default_blocking(previous))
        .await
        .map_err(|e| format!("restoring the model default: {e}"))?
}

fn restore_model_default_blocking(previous: Option<String>) -> Result<bool, String> {
    let path = user_settings_path().ok_or_else(|| "no home directory".to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if claude_model_default() != previous {
            break;
        }
        if std::time::Instant::now() >= deadline {
            // The CLI never wrote — nothing was hijacked, nothing to undo.
            return Ok(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }

    write_model_key(&path, previous.as_deref())?;
    Ok(true)
}

/// Set (or with None, remove) the `model` key in a settings file, leaving every
/// other key and its position alone. Split out from the command so the part
/// that rewrites a real config file can be tested against a scratch one.
fn write_model_key(path: &std::path::Path, model: Option<&str>) -> Result<(), String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;
    match model {
        Some(m) => {
            obj.insert("model".into(), serde_json::Value::String(m.to_string()));
        }
        None => {
            obj.remove("model");
        }
    }
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// A v4 UUID from `/dev/urandom`. `claude --resume` only accepts well-formed
/// UUIDs, and a transcript is named for its id, so the shape is load-bearing.
fn uuid_v4() -> Result<String, String> {
    let mut b = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| format!("no randomness available: {e}"))?;
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}

/// Rewrite only the two id *fields*, never every occurrence of the old id.
/// A transcript quotes its own session id in ordinary places — tool output,
/// scratchpad paths, prose — and a blind replace would rewrite that history.
/// In this session's own file, 776 occurrences of the id are only 736 fields.
/// Claude Code writes compact JSON (`"sessionId":"…"`, no spaces), so an exact
/// pattern match is precise and leaves every other byte untouched.
fn rewrite_session_ids(text: &str, old: &str, new: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut n = 0;
    for key in ["sessionId", "session_id"] {
        let from = format!("\"{key}\":\"{old}\"");
        let to = format!("\"{key}\":\"{new}\"");
        n += out.matches(&from).count();
        out = out.replace(&from, &to);
    }
    (out, n)
}

/// Branch a session: copy its transcript to a fresh id and hand back that id.
///
/// This is the whole fork. No process is started and no tab is opened, so the
/// session you forked from keeps running untouched and the branch shows up as
/// an ordinary inactive row you can resume later, at exactly the point you
/// forked. Doing it by launching `claude --fork-session --resume` instead
/// meant the branch did not exist until you typed into it, and cost a tab.
///
/// The id rewrite is mandatory, not cosmetic: `claude --resume` resolves a
/// conversation by the `sessionId` *inside* the file, not by its name. A plain
/// copy is a dead file — verified: it fails with "No conversation found with
/// session ID". (opcode's fork does exactly that, which is why its forks are
/// unresumable.)
#[tauri::command]
pub fn session_fork(session_id: String) -> Result<String, String> {
    let src = resolve_live_session_file(&session_id)
        .ok_or_else(|| "that session has no transcript left to fork".to_string())?;
    if src
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains(".orphaned-"))
    {
        return Err("that session was cleared or superseded — nothing to fork".into());
    }
    let old_id = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| "unreadable transcript name".to_string())?;
    let text = std::fs::read_to_string(&src).map_err(|e| format!("couldn't read transcript: {e}"))?;
    let new_id = uuid_v4()?;
    let (out, replaced) = rewrite_session_ids(&text, &old_id, &new_id);
    // Zero replacements means the format moved out from under us (spacing, a
    // renamed field). Writing the copy anyway would leave an unresumable file
    // on disk wearing a real-looking row, which is worse than failing loudly.
    if replaced == 0 {
        return Err("transcript has no session id fields to rewrite — not forking".into());
    }
    let dst = src.with_file_name(format!("{new_id}.jsonl"));
    std::fs::write(&dst, out).map_err(|e| format!("couldn't write the branch: {e}"))?;
    // Transcripts are private (0600); a fork must not be looser than its parent.
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600));
    // Record the lineage rather than leaving the scanner to infer it. A clean
    // copy has an intact `parentUuid` chain, so the in-file heuristic reads it
    // as an ordinary session and the ⑂ badge never appears — the branch looks
    // like an unexplained twin. We know the parent here; say so.
    record_aiterm_fork(&new_id, &old_id);
    inherit_session_source(&new_id, &old_id);
    Ok(new_id)
}

/// The parent id and boundary timestamp a `/fork` recorded, if this session is
/// one. `/fork` writes only a title stub and stores the actual content as a
/// promise in job state: "the parent's history, up to this instant". The
/// promise is redeemed when the background agent is first prompted — and if the
/// agent is stopped before that ever happens, the stub stays a 192-byte file
/// that `claude --resume` refuses. This is how we find out we can redeem it
/// ourselves.
fn fork_promise(session_id: &str) -> Option<(String, String)> {
    let jobs = dirs::home_dir()?.join(".claude/jobs");
    for job in std::fs::read_dir(jobs).ok()?.flatten() {
        let Ok(raw) = std::fs::read_to_string(job.path().join("state.json")) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if v.get("sessionId").and_then(|s| s.as_str()) != Some(session_id) {
            continue;
        }
        let parent = v.get("forkParentSessionId")?.as_str()?.to_string();
        let boundary = v.get("forkBoundaryAt")?.as_str()?.to_string();
        return Some((parent, boundary));
    }
    None
}

/// The parent's records from before a fork boundary. Cuts at the first record
/// stamped after it: ISO-8601 UTC timestamps compare correctly as strings, and
/// records carrying none (titles, modes, permission changes) travel with the
/// line order they were written in rather than being dropped.
fn history_up_to<'a>(text: &'a str, boundary: &str) -> Vec<&'a str> {
    let mut kept = Vec::new();
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                if ts > boundary {
                    break;
                }
            }
        }
        kept.push(line);
    }
    kept
}

/// Redeem a `/fork` promise: write the conversation the stub stands for, by
/// copying the parent's history up to the fork boundary under the stub's id.
///
/// This makes a console `/fork` behave like aiterm's own ⑂ — a real, resumable
/// branch — instead of a row that can only ever fail. It is the same copy and
/// id-rewrite as `session_fork`, with a cut at the boundary timestamp, and it
/// refuses to touch a stub that already has content.
#[tauri::command]
pub fn materialize_fork(session_id: String) -> Result<(), String> {
    let stub = find_session_file(&session_id)
        .ok_or_else(|| "no transcript for that session".to_string())?;
    if has_conversation(&stub) {
        return Err("that session already has a conversation".into());
    }
    let (parent_id, boundary) =
        fork_promise(&session_id).ok_or_else(|| "not a /fork — nothing to rebuild from".to_string())?;
    let parent = find_session_file(&parent_id)
        .ok_or_else(|| format!("the session it forked from ({parent_id}) is gone"))?;
    let parent_text =
        std::fs::read_to_string(&parent).map_err(|e| format!("couldn't read the parent: {e}"))?;
    let kept = history_up_to(&parent_text, &boundary);
    if kept.is_empty() {
        return Err("the parent has no history from before the fork".into());
    }
    let (history, replaced) = rewrite_session_ids(&kept.join("\n"), &parent_id, &session_id);
    if replaced == 0 {
        return Err("parent transcript has no session id fields to rewrite".into());
    }
    // Keep the stub's own two lines: they carry the "<project> ⑂" title that
    // makes a console fork recognisable, and they are already stamped with the
    // right id.
    let stub_text =
        std::fs::read_to_string(&stub).map_err(|e| format!("couldn't read the stub: {e}"))?;
    // …but they are not enough on their own. The parent's history ends with its
    // own `custom-title`, and a custom title outranks an `ai-title` when a row
    // is named — so inheriting the history would make the branch shed the ⑂ and
    // appear as a second row with the parent's name. Restate the stub's name as
    // a custom title, last, so the branch keeps saying where it came from.
    let stub_name = stub_text.lines().find_map(|l| {
        let v = serde_json::from_str::<serde_json::Value>(l).ok()?;
        v.get("aiTitle")
            .or_else(|| v.get("agentName"))?
            .as_str()
            .map(String::from)
    });
    let mut merged = format!("{}\n{}", history, stub_text.trim_end());
    if let Some(name) = stub_name {
        let rename = serde_json::json!({
            "type": "custom-title",
            "customTitle": name,
            "sessionId": session_id,
        });
        merged.push('\n');
        merged.push_str(&rename.to_string());
    }
    std::fs::write(&stub, merged + "\n").map_err(|e| format!("couldn't write the branch: {e}"))?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o600));
    record_aiterm_fork(&session_id, &parent_id);
    inherit_session_source(&session_id, &parent_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aiterm-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Restoring the model default must not disturb the rest of the file. The
    /// user's settings.json is hand-maintained; reordering or dropping keys
    /// would be a far worse bug than the one this fixes.
    #[test]
    fn write_model_key_preserves_other_keys_and_their_order() {
        let path = scratch(
            "order",
            r#"{"zeta":1,"model":"haiku","permissions":{"defaultMode":"bypassPermissions"},"alpha":2}"#,
        );
        write_model_key(&path, Some("opus")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        assert!(text.contains("\"model\": \"opus\""), "{text}");
        assert!(text.contains("bypassPermissions"), "{text}");
        let zeta = text.find("zeta").unwrap();
        let alpha = text.find("alpha").unwrap();
        assert!(zeta < alpha, "keys were reordered:\n{text}");
    }

    /// Context tokens come from the last main-chain reply; a sidechain record
    /// after it belongs to a subagent's own window and must not overwrite it.
    /// The model, by contrast, still takes last-wins as before.
    #[test]
    fn model_choice_sums_usage_and_skips_sidechains() {
        let main = r#"{"type":"assistant","timestamp":"t1","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_read_input_tokens":100000,"cache_creation_input_tokens":1000,"output_tokens":500}}}"#;
        let side = r#"{"type":"assistant","isSidechain":true,"timestamp":"t2","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":1,"cache_read_input_tokens":42,"cache_creation_input_tokens":0,"output_tokens":7}}}"#;
        let out = parse_model_choice([main, side].into_iter());
        assert_eq!(out.context_tokens, Some(101_502));
        assert_eq!(out.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(out.at.as_deref(), Some("t2"));
    }

    /// A synthetic record (no usage, `<synthetic>` model) must leave both the
    /// model and the token count from the real reply before it intact.
    #[test]
    fn model_choice_ignores_synthetic_records() {
        let real = r#"{"type":"assistant","timestamp":"t1","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":5}}}"#;
        let synth = r#"{"type":"assistant","timestamp":"t2","message":{"model":"<synthetic>"}}"#;
        let out = parse_model_choice([real, synth].into_iter());
        assert_eq!(out.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(out.context_tokens, Some(15));
    }

    /// A default that was never set must come back absent, not as null or "".
    #[test]
    fn write_model_key_removes_the_key_when_there_was_none() {
        let path = scratch("remove", r#"{"model":"haiku","other":true}"#);
        write_model_key(&path, None).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v.get("model").is_none());
        assert_eq!(v.get("other").and_then(|o| o.as_bool()), Some(true));
    }

    #[test]
    fn write_model_key_refuses_a_non_object_file() {
        let path = scratch("array", "[1,2,3]");
        assert!(write_model_key(&path, Some("opus")).is_err());
    }

    #[test]
    fn fork_rewrites_id_fields_only() {
        let old = "437fecea-45a2-409f-a0a0-ed2161621425";
        let new = "fa06996a-14c7-4e88-9502-508eb55d220d";
        // Third line quotes the id in ordinary content — a path and prose. A
        // blind replace would rewrite those too, silently editing history.
        let text = format!(
            "{{\"type\":\"user\",\"sessionId\":\"{old}\"}}\n\
             {{\"type\":\"assistant\",\"session_id\":\"{old}\",\"requestId\":\"x\"}}\n\
             {{\"type\":\"user\",\"sessionId\":\"{old}\",\"text\":\"see /tmp/{old}/out and id {old}\"}}\n"
        );
        let (out, n) = rewrite_session_ids(&text, old, new);
        assert_eq!(n, 3, "three id fields");
        assert_eq!(out.matches(new).count(), 3);
        // The two content mentions survive untouched.
        assert!(out.contains(&format!("see /tmp/{old}/out and id {old}")));
        assert!(!out.contains(&format!("\"sessionId\":\"{old}\"")));
    }

    #[test]
    fn permission_mode_prefers_the_project_and_rejects_junk() {
        let root = std::env::temp_dir().join("aiterm-test-permmode");
        let _ = std::fs::remove_dir_all(&root);
        let proj = root.join("proj");
        std::fs::create_dir_all(proj.join(".claude")).unwrap();

        // Project settings.json only: that's the answer.
        std::fs::write(
            proj.join(".claude/settings.json"),
            "{\"permissions\":{\"defaultMode\":\"acceptEdits\"}}",
        )
        .unwrap();
        assert_eq!(mode_from(&[proj.join(".claude/settings.json")]), Some("acceptEdits".into()));

        // settings.local.json outranks it.
        std::fs::write(
            proj.join(".claude/settings.local.json"),
            "{\"permissions\":{\"defaultMode\":\"bypassPermissions\"}}",
        )
        .unwrap();
        assert_eq!(
            mode_from(&[
                proj.join(".claude/settings.local.json"),
                proj.join(".claude/settings.json"),
            ]),
            Some("bypassPermissions".into())
        );

        // A mode the CLI would reject yields None — `claude` exits on an
        // unknown value, and a terminal that dies on open is worse than a prompt.
        std::fs::write(
            proj.join(".claude/settings.local.json"),
            "{\"permissions\":{\"defaultMode\":\"yolo\"}}",
        )
        .unwrap();
        assert_eq!(mode_from(&[proj.join(".claude/settings.local.json")]), None);

        // Missing file, and a file with no permissions block, both fall through.
        assert_eq!(mode_from(&[root.join("nope.json")]), None);
        std::fs::write(proj.join(".claude/bare.json"), "{\"tui\":\"fullscreen\"}").unwrap();
        assert_eq!(mode_from(&[proj.join(".claude/bare.json")]), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The lookup half of `claude_permission_mode`, over an explicit candidate
    /// list so precedence is testable without a fake $HOME.
    fn mode_from(candidates: &[std::path::PathBuf]) -> Option<String> {
        const ACCEPTED: [&str; 6] = [
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "manual",
            "dontAsk",
            "plan",
        ];
        for path in candidates {
            let Ok(raw) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            let Some(mode) = v
                .get("permissions")
                .and_then(|p| p.get("defaultMode"))
                .and_then(|m| m.as_str())
            else {
                continue;
            };
            return ACCEPTED.contains(&mode).then(|| mode.to_string());
        }
        None
    }

    #[test]
    fn job_dir_matches_on_recorded_id_not_directory_name() {
        let root = std::env::temp_dir().join("aiterm-test-jobs");
        let _ = std::fs::remove_dir_all(&root);
        let want = "7f8edb5a-26d1-4520-900e-aff9c9b42dbf";
        // A decoy whose *directory* is named like the session's first segment,
        // but whose recorded session is someone else's. Matching on the name
        // would trash Claude Code's records for an unrelated session.
        std::fs::create_dir_all(root.join("7f8edb5a")).unwrap();
        std::fs::write(
            root.join("7f8edb5a/state.json"),
            "{\"sessionId\":\"11111111-2222-3333-4444-555555555555\"}",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("someotherdir")).unwrap();
        std::fs::write(
            root.join("someotherdir/state.json"),
            format!("{{\"sessionId\":\"{want}\",\"daemonShort\":\"abcd1234\"}}"),
        )
        .unwrap();
        // A directory with no state.json at all must not blow up the scan.
        std::fs::create_dir_all(root.join("empty")).unwrap();

        let found = find_job_dir(&root, want).expect("matches by recorded id");
        assert!(found.ends_with("someotherdir"), "got {found:?}");
        assert!(find_job_dir(&root, "nobody-has-this-id").is_none());
        // Restore prefers the recorded daemonShort over re-deriving a name.
        assert_eq!(job_dir_name(&found, want), "abcd1234");
        assert_eq!(job_dir_name(&root.join("empty"), want), "7f8edb5a");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn history_cuts_at_the_fork_boundary() {
        let text = "\
{\"type\":\"user\",\"timestamp\":\"2026-07-25T18:00:00.000Z\"}
{\"type\":\"mode\",\"mode\":\"default\"}
{\"type\":\"assistant\",\"timestamp\":\"2026-07-25T18:49:00.000Z\"}
{\"type\":\"user\",\"timestamp\":\"2026-07-25T18:50:00.000Z\"}
{\"type\":\"assistant\",\"timestamp\":\"2026-07-25T18:51:00.000Z\"}";
        let kept = history_up_to(text, "2026-07-25T18:49:37.666Z");
        // Everything up to the boundary, including the untimestamped record
        // that sits between them; nothing from after it.
        assert_eq!(kept.len(), 3);
        assert!(kept[1].contains("\"mode\""), "untimestamped records ride along");
        assert!(!kept.iter().any(|l| l.contains("18:50")));
    }

    #[test]
    fn history_keeps_everything_when_the_fork_is_newer() {
        let text = "{\"type\":\"user\",\"timestamp\":\"2026-07-25T18:00:00.000Z\"}";
        assert_eq!(history_up_to(text, "2027-01-01T00:00:00.000Z").len(), 1);
    }

    #[test]
    fn a_title_only_stub_is_not_a_conversation() {
        let dir = std::env::temp_dir().join("aiterm-test-hasconv");
        let _ = std::fs::create_dir_all(&dir);
        let stub = dir.join("stub.jsonl");
        std::fs::write(
            &stub,
            "{\"type\":\"ai-title\",\"aiTitle\":\"aiterm ⑂\"}\n\
             {\"type\":\"agent-name\",\"agentName\":\"aiterm ⑂\"}\n",
        )
        .unwrap();
        assert!(!has_conversation(&stub), "a /fork stub has nothing to resume");
        let real = dir.join("real.jsonl");
        std::fs::write(&real, "{\"type\":\"ai-title\"}\n{\"type\":\"user\"}\n").unwrap();
        assert!(has_conversation(&real), "one message record is enough");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fork_reports_nothing_to_rewrite() {
        // Spaced-out JSON is not the format Claude Code writes; better to
        // report zero than to leave an unresumable copy on disk.
        let (_, n) = rewrite_session_ids("{\"sessionId\": \"abc\"}", "abc", "def");
        assert_eq!(n, 0);
    }

    #[test]
    fn uuid_v4_is_well_formed() {
        let u = uuid_v4().expect("urandom");
        let parts: Vec<&str> = u.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(u.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert_eq!(parts[2].as_bytes()[0], b'4', "version nibble");
        assert!(matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(u, uuid_v4().unwrap(), "not a constant");
    }

    #[test]
    fn strips_and_detects_noise() {
        assert_eq!(
            strip_system_tags("<local-command-caveat>Caveat: blah</local-command-caveat>hi there"),
            "hi there"
        );
        // Unbalanced known system tag drops the rest.
        assert_eq!(strip_system_tags("<local-command-caveat>Caveat: The mess"), "");
        assert!(is_system_meta_prompt("You are summarizing a Claude Code session for a log"));
        assert!(is_system_meta_prompt(
            "Caveat: The messages below were generated by the user while running local commands"
        ));
        assert!(!is_system_meta_prompt("hey claude fix my thing"));
    }

    #[test]
    fn drops_tmp_and_titleless_sessions() {
        let dir = std::env::temp_dir().join("aiterm-test-sessions");
        std::fs::create_dir_all(&dir).unwrap();

        let tmp_session = dir.join("11111111-1111-1111-1111-111111111111.jsonl");
        std::fs::write(&tmp_session,
            r#"{"type":"user","cwd":"/tmp","message":{"role":"user","content":"real prompt"}}"#).unwrap();
        assert!(parse_session(&tmp_session, None).is_none(), "/tmp session should be dropped");

        let meta_session = dir.join("22222222-2222-2222-2222-222222222222.jsonl");
        std::fs::write(&meta_session,
            r#"{"type":"user","cwd":"/home/x/proj","message":{"role":"user","content":"You are summarizing a Claude Code session for a daily memory log"}}"#).unwrap();
        assert!(parse_session(&meta_session, None).is_none(), "meta-only session should be dropped");

        let real_session = dir.join("33333333-3333-3333-3333-333333333333.jsonl");
        std::fs::write(&real_session, concat!(
            r#"{"type":"user","cwd":"/home/x/proj","message":{"role":"user","content":"<local-command-caveat>Caveat: The messages below were generated</local-command-caveat>"}}"#, "\n",
            r#"{"type":"user","cwd":"/home/x/proj","gitBranch":"main","message":{"role":"user","content":"hey fix the login bug"}}"#)).unwrap();
        let s = parse_session(&real_session, None).expect("real session kept");
        assert_eq!(s.title, "hey fix the login bug");
        assert_eq!(s.branch.as_deref(), Some("main"));
    }

    #[test]
    fn keeps_fork_stub_via_ai_title_and_backfilled_cwd() {
        // A Claude Code /fork stub: only an ai-title + agent-name, no cwd, no
        // prompt, no bridge. Must still surface so forks are visible.
        let dir = std::env::temp_dir().join("aiterm-test-fork-stub");
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("44444444-4444-4444-4444-444444444444.jsonl");
        std::fs::write(
            &stub,
            concat!(
                r#"{"type":"ai-title","aiTitle":"headroom ⑂","sessionId":"x"}"#,
                "\n",
                r#"{"type":"agent-name","agentName":"headroom ⑂","sessionId":"x"}"#,
            ),
        )
        .unwrap();
        // No cwd anywhere → still dropped (can't place it in a project).
        assert!(parse_session(&stub, None).is_none(), "stub w/o cwd dropped");
        // With the project dir's cwd backfilled, it shows under its ai-title.
        let s = parse_session(&stub, Some("/home/matt/Projects/headroom"))
            .expect("fork stub kept");
        assert_eq!(s.title, "headroom ⑂");
        assert_eq!(s.project_path, "/home/matt/Projects/headroom");
    }

    #[test]
    fn tells_a_fork_child_from_a_clear_child() {
        let dir = std::env::temp_dir().join("aiterm-test-fork-vs-clear");
        std::fs::create_dir_all(&dir).unwrap();

        // /fork child: opens mid-conversation, its first record chaining to a
        // uuid that lives in the parent's transcript (shape taken from a real
        // `sessionKind":"bg"` fork). The parent must not be superseded.
        let fork = dir.join("55555555-5555-5555-5555-555555555555.jsonl");
        std::fs::write(&fork, concat!(
            r#"{"type":"assistant","uuid":"aaaa","parentUuid":"b689eab6","sessionKind":"bg","cwd":"/home/x/proj","message":{"role":"assistant","content":"carrying on"}}"#, "\n",
            r#"{"type":"user","uuid":"bbbb","parentUuid":"aaaa","sessionKind":"bg","cwd":"/home/x/proj","gitBranch":"main","message":{"role":"user","content":"keep going on this branch"}}"#,
        )).unwrap();
        let s = parse_session(&fork, None).expect("fork child kept");
        assert!(s.forked, "dangling first parentUuid marks a fork child");
        // forked && background => a /fork child: no tab rebinds to it.
        assert!(s.background, "sessionKind bg marks the background fork");

        // A compact continuation also chains out of its file, but runs in the
        // foreground — the tab does follow that one.
        let compact = dir.join("88888888-8888-8888-8888-888888888888.jsonl");
        std::fs::write(&compact, concat!(
            r#"{"type":"system","subtype":"compact_boundary","uuid":"eeee","parentUuid":"prior9","cwd":"/home/x/proj"}"#, "\n",
            r#"{"type":"user","uuid":"ffff","parentUuid":"eeee","cwd":"/home/x/proj","gitBranch":"main","message":{"role":"user","content":"picking up after compaction"}}"#,
        )).unwrap();
        let s = parse_session(&compact, None).expect("compact continuation kept");
        assert!(s.forked, "compact continuation chains out of its own file");
        assert!(!s.background, "compaction stays in the foreground session");

        // /clear child: a fresh, self-contained conversation — its chain
        // resolves inside its own file, so the old row still gets hidden.
        let clear = dir.join("66666666-6666-6666-6666-666666666666.jsonl");
        std::fs::write(&clear, concat!(
            r#"{"type":"attachment","uuid":"cccc","parentUuid":null,"cwd":"/home/x/proj"}"#, "\n",
            r#"{"type":"user","uuid":"dddd","parentUuid":"cccc","cwd":"/home/x/proj","gitBranch":"main","message":{"role":"user","content":"starting something new"}}"#,
        )).unwrap();
        let s = parse_session(&clear, None).expect("clear child kept");
        assert!(!s.forked, "self-contained chain is a /clear, not a fork");
    }

    #[test]
    fn keeps_a_fork_whose_only_prompt_is_a_last_prompt_record() {
        // Shape of a real `/fork` child: a bridge-session marker (so the
        // promptless-bookkeeping filter is armed) and `user` records that are
        // nothing but replayed tool results — what the user actually typed
        // lives in `last-prompt`. Missing that dropped every fork from the
        // list, so no row ever appeared for a forked session.
        let dir = std::env::temp_dir().join("aiterm-test-fork-last-prompt");
        std::fs::create_dir_all(&dir).unwrap();
        let fork = dir.join("99999999-9999-9999-9999-999999999999.jsonl");
        std::fs::write(&fork, concat!(
            r#"{"type":"bridge-session","sessionId":"x","bridgeSessionId":"cse_1"}"#, "\n",
            r#"{"type":"assistant","uuid":"aaaa","parentUuid":"elsewhere","sessionKind":"bg","cwd":"/home/x/proj","message":{"role":"assistant","content":"working"}}"#, "\n",
            r#"{"type":"user","uuid":"bbbb","parentUuid":"aaaa","cwd":"/home/x/proj","message":{"role":"user","content":[{"type":"tool_result","content":"cargo test: ok"}]}}"#, "\n",
            r#"{"type":"last-prompt","lastPrompt":"take this branch and try the other approach","sessionId":"x"}"#,
        )).unwrap();
        let s = parse_session(&fork, None).expect("fork with only a last-prompt is kept");
        assert_eq!(s.title, "take this branch and try the other approach");
        assert!(s.forked && s.background, "still recognised as a /fork child");

        // A last-prompt holding only injected system text is still not a human
        // prompt — that bookkeeping stays filtered out.
        let noise = dir.join("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl");
        std::fs::write(&noise, concat!(
            r#"{"type":"bridge-session","sessionId":"y","bridgeSessionId":"cse_2"}"#, "\n",
            r#"{"type":"last-prompt","lastPrompt":"<system-reminder>be nice</system-reminder>","cwd":"/home/x/proj","sessionId":"y"}"#,
        )).unwrap();
        assert!(parse_session(&noise, None).is_none(), "system-only prompt stays filtered");
    }

    #[test]
    fn newborn_fork_stub_has_no_chain_but_job_state_knows() {
        // The exact shape that leaked a "hid superseded" toast: a `/fork` stub
        // the moment it is created is an ai-title and an agent-name, nothing
        // else. No parentUuid exists, so the transcript heuristic cannot see
        // the fork and the parent got hidden. Job state carries the lineage
        // from the start, which is why the scan overrides `forked` from it.
        let dir = std::env::temp_dir().join("aiterm-test-newborn-fork");
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("1eeedeca-05a5-41f4-a2c6-dc791d1a2a61.jsonl");
        std::fs::write(&stub, concat!(
            r#"{"type":"ai-title","aiTitle":"aiterm ⑂","sessionId":"1eeedeca"}"#, "\n",
            r#"{"type":"agent-name","agentName":"aiterm","sessionId":"1eeedeca"}"#,
        )).unwrap();
        let s = parse_session(&stub, Some("/home/x/proj")).expect("stub is listed");
        assert!(!s.forked, "no chain in the file yet — heuristic cannot tell");
        assert_eq!(s.fork_parent, None, "parse_session never sets lineage");

        // Job state supplies what the transcript cannot.
        let jobs = std::env::temp_dir().join("aiterm-test-newborn-fork-jobs");
        let job = jobs.join("1eeedeca");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(job.join("state.json"), concat!(
            r#"{"forkSessionId":"1eeedeca-05a5-41f4-a2c6-dc791d1a2a61","#,
            r#""forkParentSessionId":"9c82d668-97de-469a-9cd4-ca25319bb145"}"#,
        )).unwrap();
        let map = fork_parent_map(&jobs);
        assert_eq!(
            map.get("1eeedeca-05a5-41f4-a2c6-dc791d1a2a61").map(String::as_str),
            Some("9c82d668-97de-469a-9cd4-ca25319bb145"),
            "lineage is available before the fork writes any message",
        );

        // A fork OF a fork omits `forkSessionId` and carries only `sessionId`
        // + `forkParentSessionId` — the real shape of job 457d5d29, which was
        // skipped entirely while the map demanded both keys.
        let nested = jobs.join("457d5d29");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("state.json"), concat!(
            r#"{"sessionId":"457d5d29-1111-4111-8111-111111111111","#,
            r#""forkParentSessionId":"1eeedeca-05a5-41f4-a2c6-dc791d1a2a61","#,
            r#""forkSourceAlive":true}"#,
        )).unwrap();
        let map = fork_parent_map(&jobs);
        assert_eq!(
            map.get("457d5d29-1111-4111-8111-111111111111").map(String::as_str),
            Some("1eeedeca-05a5-41f4-a2c6-dc791d1a2a61"),
            "a fork with no forkSessionId still resolves via sessionId",
        );

        // A plain (non-fork) job must still be ignored — `sessionId` alone is
        // not lineage, or every session would claim a parent.
        let plain = jobs.join("4c8c8287");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("state.json"),
            r#"{"sessionId":"4c8c8287-2222-4222-8222-222222222222"}"#).unwrap();
        let map = fork_parent_map(&jobs);
        assert_eq!(map.len(), 2, "non-fork job must not appear");

        let _ = std::fs::remove_dir_all(&jobs);
    }

    #[test]
    fn worktree_sessions_group_under_their_repo() {
        assert_eq!(
            worktree_repo_root("/home/matt/Projects/aiterm/.claude/worktrees/fix-agents-sync"),
            Some("/home/matt/Projects/aiterm".into()),
        );
        // /fork can land the agent in a subdir of the worktree.
        assert_eq!(
            worktree_repo_root(
                "/home/matt/Projects/aiterm/.claude/worktrees/fix-agents-sync/src-tauri"
            ),
            Some("/home/matt/Projects/aiterm".into()),
        );
        assert_eq!(worktree_repo_root("/home/matt/Projects/aiterm"), None);

        // The session keeps its real cwd for spawning, but groups under the repo.
        let dir = std::env::temp_dir().join("aiterm-test-worktree-group");
        std::fs::create_dir_all(&dir).unwrap();
        let wt = dir.join("77777777-7777-7777-7777-777777777777.jsonl");
        std::fs::write(&wt,
            r#"{"type":"user","uuid":"a","parentUuid":null,"cwd":"/home/x/proj/.claude/worktrees/wip/src-tauri","message":{"role":"user","content":"fork work"}}"#).unwrap();
        let s = parse_session(&wt, None).expect("worktree session kept");
        assert_eq!(s.project_path, "/home/x/proj/.claude/worktrees/wip/src-tauri");
        assert_eq!(s.group_path, "/home/x/proj");
    }
}

#[tauri::command]
pub fn list_sessions() -> Vec<Session> {
    // Adding an agent means adding a backend in `agents.rs` and nothing here.
    crate::agents::scan_all_with_paths()
        .into_iter()
        .map(|(s, _)| s)
        .collect()
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use std::io::Write;

    fn write_jsonl(dir: &Path, name: &str, rows: &[&str]) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = File::create(&p).unwrap();
        for r in rows {
            writeln!(f, "{r}").unwrap();
        }
        p
    }

    /// Shape taken from a real migration captured 2026-07-26: the child's
    /// ancestry uuids resolve into the parent, and only the child is `bg`.
    #[test]
    fn lineage_links_need_both_bg_and_an_ancestry_uuid() {
        let tmp = std::env::temp_dir().join(format!("aiterm-mig-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);

        let parent = write_jsonl(
            &tmp,
            "parent.jsonl",
            &[r#"{"type":"user","uuid":"AAA","sessionId":"parent"}"#],
        );
        let child = write_jsonl(&tmp, "child.jsonl", &[
            r#"{"type":"system","subtype":"compact_boundary","logicalParentUuid":"AAA","uuid":"BBB","sessionKind":"bg"}"#,
        ]);
        // A plain --fork-session branch: shares ancestry, but is not a daemon
        // session. Re-keying a tab onto one of these would swap a running
        // conversation for a copy of it.
        let branch = write_jsonl(
            &tmp,
            "branch.jsonl",
            &[r#"{"type":"user","parentUuid":"AAA","uuid":"CCC"}"#],
        );

        let (links, is_bg) = read_lineage_links(&child).unwrap();
        assert!(is_bg, "child should be recognised as a daemon session");
        assert!(links.contains("AAA"), "child should claim the parent's uuid");
        assert!(file_has_any_uuid(&parent, &links), "AAA lives in the parent");

        let (blinks, bbg) = read_lineage_links(&branch).unwrap();
        assert!(!bbg, "a --fork-session branch is not a daemon session");
        assert!(
            file_has_any_uuid(&parent, &blinks),
            "the branch does share ancestry — which is exactly why bg is required too"
        );

        let unrelated = write_jsonl(
            &tmp,
            "unrelated.jsonl",
            &[r#"{"type":"user","parentUuid":"ZZZ","uuid":"DDD","sessionKind":"bg"}"#],
        );
        let (ulinks, _) = read_lineage_links(&unrelated).unwrap();
        assert!(
            !file_has_any_uuid(&parent, &ulinks),
            "an unrelated bg session must not resolve into this parent"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Run with `cargo test -- --ignored` on a machine that still has the
    /// 2026-07-26 specimen. Non-hermetic by design: it checks the real files
    /// the rule was derived from, which no fixture can stand in for.
    #[test]
    #[ignore]
    fn finds_the_captured_specimen() {
        let got = session_migrated_to("2eb3a23f-e4f1-4263-beb0-e3c7b768dcba".into());
        assert_eq!(got.as_deref(), Some("6b37ca79-7e8f-4b86-9817-eaeb1b1fe95c"));
    }
}

/* ---- source records ------------------------------------------------------ */

#[cfg(test)]
mod source_tests {
    use super::*;

    fn sess(id: &str) -> Session {
        Session {
            id: id.into(),
            agent: "claude".into(),
            title: "t".into(),
            project_path: "/p".into(),
            group_path: "/p".into(),
            branch: None,
            forked: false,
            background: false,
            fork_parent: None,
            last_active: 0,
            source: None,
            source_label: None,
            source_model: None,
        }
    }

    fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// The case the whole record exists for: a session that *is* Claude Code on
    /// disk, started against someone else's endpoint. `agent` must stay honest
    /// about the engine while `source` says where it was pointed — if the two
    /// ever collapse into one field the rows go back to being identical.
    #[test]
    fn an_api_started_session_keeps_its_engine_and_gains_a_source() {
        let map = parse_source_map(
            r#"{"s1":{"agent":"api:openrouter","model":"anthropic/claude-sonnet-5"}}"#,
        );
        let mut s = sess("s1");
        attach_source(&mut s, &map, &labels(&[("api:openrouter", "OpenRouter")]));
        assert_eq!(s.agent, "claude", "the transcript is still Claude Code's");
        assert_eq!(s.source.as_deref(), Some("api:openrouter"));
        assert_eq!(s.source_label.as_deref(), Some("OpenRouter"));
        assert_eq!(s.source_model.as_deref(), Some("anthropic/claude-sonnet-5"));
    }

    /// Most rows on a real machine predate the record, or were started from a
    /// shell. They must come back untouched rather than defaulted to something:
    /// "aiterm did not start this" is the answer, not a gap to fill.
    #[test]
    fn a_session_with_no_record_is_left_alone() {
        let mut s = sess("unknown");
        attach_source(&mut s, &parse_source_map("{}"), &labels(&[]));
        assert_eq!(s.source, None);
        assert_eq!(s.source_label, None);
    }

    /// A provider deleted from settings takes its backend with it, so there is
    /// no display name left to look up. The row still ran against something,
    /// and saying "openrouter" beats silently demoting it to look local.
    #[test]
    fn a_source_whose_backend_is_gone_still_reads_as_itself() {
        let map = parse_source_map(r#"{"s1":{"agent":"api:openrouter"}}"#);
        let mut s = sess("s1");
        attach_source(&mut s, &map, &labels(&[]));
        assert_eq!(s.source_label.as_deref(), Some("openrouter"));
        assert_eq!(s.source_model, None, "no model was recorded, so none is claimed");
    }

    /// A row that already knows its own model — a Codex rollout states it —
    /// must not have it overwritten by a picker choice recorded elsewhere.
    #[test]
    fn a_row_that_knows_its_model_keeps_it() {
        let map = parse_source_map(r#"{"s1":{"agent":"codex","model":"from-the-picker"}}"#);
        let mut s = sess("s1");
        s.source_model = Some("from-the-rollout".into());
        attach_source(&mut s, &map, &labels(&[]));
        assert_eq!(s.source_model.as_deref(), Some("from-the-rollout"));
    }

    /// A truncated or hand-edited file must cost the scan nothing. Every row in
    /// the sidebar is real and on disk; losing the list over a display detail
    /// would be a far worse failure than losing the detail.
    #[test]
    fn an_unusable_source_file_degrades_to_no_records() {
        for junk in ["", "not json", "[]", r#"{"s1":42}"#, r#"{"s1":{"nope":1}}"#] {
            assert!(parse_source_map(junk).is_empty(), "accepted {junk:?}");
        }
    }

    /// An entry with no model round-trips as absent rather than as `""`, so
    /// "the agent's own default" and "nobody said" never have to be told apart
    /// downstream.
    #[test]
    fn a_default_model_is_stored_as_absent() {
        let rec = SessionSource { agent: "claude".into(), model: None };
        let text = serde_json::to_string(&HashMap::from([("s1".to_string(), rec)])).unwrap();
        assert!(!text.contains("model"), "{text}");
        assert_eq!(parse_source_map(&text).get("s1").unwrap().model, None);
    }
}

/* ---- Codex rollouts ------------------------------------------------------ */

#[cfg(test)]
mod codex_tests {
    use super::*;

    /// Trimmed from the real file `codex exec` wrote on 2026-07-27
    /// (`rollout-2026-07-27T20-22-44-019fa61a-….jsonl`, codex-cli 0.145.0).
    /// `base_instructions` — several kilobytes of system prompt — and most of
    /// the injected `response_item` records are dropped; nothing else is edited.
    const EXEC_ROLLOUT: &str = r#"
{"timestamp":"2026-07-28T00:22:44.537Z","type":"session_meta","payload":{"session_id":"019fa61a-39f4-7923-a717-215dd3b0aa58","id":"019fa61a-39f4-7923-a717-215dd3b0aa58","cwd":"/home/admin/AI-OS","originator":"codex_exec","cli_version":"0.145.0","source":"exec","model_provider":"openai","git":{"commit_hash":"7cef5e66","branch":"master"}}}
{"timestamp":"2026-07-28T00:22:44.537Z","type":"event_msg","payload":{"type":"task_started","turn_id":"019fa61a-3a33"}}
{"timestamp":"2026-07-28T00:22:46.308Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context><cwd>/home/admin/AI-OS</cwd></environment_context>"}]}}
{"timestamp":"2026-07-28T00:22:46.308Z","type":"turn_context","payload":{"turn_id":"019fa61a-3a33","cwd":"/home/admin/AI-OS","approval_policy":"never","model":"gpt-5.6-sol"}}
{"timestamp":"2026-07-28T00:22:46.314Z","type":"event_msg","payload":{"type":"user_message","message":"Reply with exactly: CODEX OK","images":[]}}
{"timestamp":"2026-07-28T00:22:48.337Z","type":"event_msg","payload":{"type":"agent_message","message":"CODEX OK","phase":"final_answer"}}
{"timestamp":"2026-07-28T00:22:48.390Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"019fa61a-3a33"}}
"#;

    /// The same, from the *interactive* TUI, driven on a pty the same evening
    /// (`rollout-2026-07-27T21-32-09-019fa659-….jsonl`). This is the mode
    /// aiterm actually launches, and the differences are why it was captured
    /// separately rather than assumed: `originator`, `source`, and no `git` key
    /// at all, because its cwd was not a repository.
    const TUI_ROLLOUT: &str = r#"
{"timestamp":"2026-07-28T01:32:09.732Z","type":"session_meta","payload":{"session_id":"019fa659-c884-7620-94fa-606596862c11","id":"019fa659-c884-7620-94fa-606596862c11","cwd":"/tmp","originator":"codex-tui","cli_version":"0.145.0","source":"cli","model_provider":"openai"}}
{"timestamp":"2026-07-28T01:32:09.800Z","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}
{"timestamp":"2026-07-28T01:32:15.000Z","type":"event_msg","payload":{"type":"user_message","message":"with exactly: TUI OK","images":[]}}
{"timestamp":"2026-07-28T01:32:17.000Z","type":"event_msg","payload":{"type":"agent_message","message":"TUI OK","phase":"final_answer"}}
"#;

    #[test]
    fn an_exec_rollout_yields_id_cwd_branch_and_model() {
        let head = parse_codex_head(EXEC_ROLLOUT.lines());
        assert_eq!(head.id.as_deref(), Some("019fa61a-39f4-7923-a717-215dd3b0aa58"));
        assert_eq!(head.cwd.as_deref(), Some("/home/admin/AI-OS"));
        assert_eq!(head.branch.as_deref(), Some("master"));
        assert_eq!(head.source.as_deref(), Some("exec"));
        assert_eq!(head.model.as_deref(), Some("gpt-5.6-sol"));
    }

    /// The title must be what the human typed, not the first thing with
    /// `role: "user"` on it. Codex injects the permissions block, the skills
    /// catalogue and `<environment_context>` as user-role `response_item`s, and
    /// a sidebar of rows titled "<environment_context><cwd>/home/admin…" would
    /// be worse than no rows at all.
    #[test]
    fn the_title_is_the_prompt_and_not_the_injected_context() {
        let head = parse_codex_head(EXEC_ROLLOUT.lines());
        assert_eq!(head.title.as_deref(), Some("Reply with exactly: CODEX OK"));
    }

    /// The interactive mode is the one aiterm launches, and it differs in every
    /// field that identifies it. Pinned so a future version that changes them
    /// fails here rather than silently emptying the sidebar.
    #[test]
    fn the_interactive_rollout_parses_the_same_way() {
        let head = parse_codex_head(TUI_ROLLOUT.lines());
        assert_eq!(head.id.as_deref(), Some("019fa659-c884-7620-94fa-606596862c11"));
        assert_eq!(head.cwd.as_deref(), Some("/tmp"));
        assert_eq!(head.source.as_deref(), Some("cli"));
        assert_eq!(head.title.as_deref(), Some("with exactly: TUI OK"));
        // No `git` key: this one ran outside a repository. Absent, not empty.
        assert_eq!(head.branch, None);
        // No turn_context in this capture, so no model is claimed.
        assert_eq!(head.model, None);
    }

    /// Nothing in a rollout is guaranteed, and a half-written file is the
    /// normal state of a session that is running right now. Every field is
    /// independently optional and none of them may panic.
    #[test]
    fn a_rollout_with_nothing_useful_yields_nothing() {
        let head = parse_codex_head(["", "not json", "{}", r#"{"type":"session_meta"}"#]);
        assert_eq!(head, CodexHead::default());
    }

    /// A session opened and closed without a prompt has no title and nothing to
    /// resume to. `parse_codex_rollout` drops it, matching what `parse_session`
    /// does with a promptless Claude transcript — one rule across both stores.
    #[test]
    fn a_rollout_with_no_prompt_has_no_title() {
        let no_prompt: String = EXEC_ROLLOUT
            .lines()
            .filter(|l| !l.contains("user_message"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(parse_codex_head(no_prompt.lines()).title, None);
    }

    /// The preview pane reads both formats through one function. Codex writes
    /// the assistant's turn as `agent_message`, which must arrive as
    /// `assistant` — the pane styles and labels on that string, and a third
    /// value renders as nothing at all.
    #[test]
    fn codex_turns_map_onto_the_roles_the_preview_pane_knows() {
        let roles: Vec<Option<String>> = TUI_ROLLOUT
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .map(|v| codex_role(&v))
            .collect();
        assert!(roles.contains(&Some("user".into())));
        assert!(roles.contains(&Some("assistant".into())));
        // Bookkeeping records are not conversation.
        assert!(roles.contains(&None));
    }

    /// A Claude transcript must not be read as a Codex one. The two now share
    /// the preview loop, and `"type":"user"` is one closing quote away from
    /// `"type":"user_message"`.
    #[test]
    fn claude_records_are_not_mistaken_for_codex_ones() {
        for line in [
            r#"{"type":"user","message":{"content":"hi"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
        ] {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(codex_role(&v), None, "claimed a Claude record: {line}");
        }
    }
}
