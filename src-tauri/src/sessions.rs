use serde::Serialize;
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

    /// The conversation as `(role, text)`, for a backend whose sessions are not
    /// a file.
    ///
    /// `None` means "read the transcript path", which is every existing
    /// provider and stays the default. It exists because OpenCode keeps its
    /// sessions in a SQLite database: there is no file for the preview panel or
    /// the search indexer to open, and both of them assumed a session *is* a
    /// jsonl. A provider that answers here is read through this instead, and
    /// its path is never opened.
    fn messages(&self, session_id: &str) -> Option<Vec<(String, String)>> {
        let _ = session_id;
        None
    }

    /// Everything the hover flyout shows, for an engine whose records say
    /// more than `messages()` can carry — grok's `summary.json` has the
    /// model, the effort, the branch and the timestamps; OpenCode's database
    /// has token counts and tool calls. `None` means "read it the generic
    /// way" — `messages()`, then the transcript — which stays the default.
    fn detail(&self, session_id: &str) -> Option<crate::detail::SessionDetail> {
        let _ = session_id;
        None
    }

    /// The session's task list, for an engine that records one in its own
    /// shape — grok's `todo_write`, codex's `update_plan`. `None` means
    /// "scan the claude transcript", which stays the default.
    fn tasks(&self, session_id: &str) -> Option<Vec<SessionTask>> {
        let _ = session_id;
        None
    }

    /// Files the session wrote, same contract as [`Self::tasks`].
    fn artifacts(&self, session_id: &str) -> Option<Vec<Artifact>> {
        let _ = session_id;
        None
    }

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
pub(crate) fn strip_system_tags(text: &str) -> String {
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

/// Whether a message is entirely system-injected markup, with nothing a
/// person wrote left once the tagged blocks come out.
///
/// Used by the indexer to drop boilerplate that would otherwise appear in
/// every session of an agent and make a search for any of its words match all
/// of them. Deliberately all-or-nothing: a message that mixes a context block
/// with real text is indexed whole, because guessing which half to keep is how
/// you lose the half somebody wanted to find.
pub(crate) fn is_only_system_block(text: &str) -> bool {
    if is_codex_agents_preamble(text) {
        return true;
    }
    !text.trim().is_empty() && strip_system_tags(text).trim().is_empty()
}

/// Codex sends the repo's AGENTS.md as its own first "user" message: an
/// untagged `# AGENTS.md instructions for <cwd>` header ahead of an
/// `<INSTRUCTIONS>…</INSTRUCTIONS>` block. The whole-block system filter
/// keeps it — stripping the tags leaves the header line — so it has to be
/// named here: harness preamble, never a preview row or an index entry.
/// Both the header AND the block are required, so a genuine message that
/// merely mentions AGENTS.md is not swallowed. Checked against the RAW
/// text, before tags are stripped. Older rollouts (0.147.0–0.149.1) put a
/// `<recommended_plugins>` block ahead of the header — skip it before the
/// prefix check. Mirrors `detail.rs`'s predicate of the same name — keep
/// the two in step. [observed: codex-cli 0.150.1]
fn is_codex_agents_preamble(text: &str) -> bool {
    let mut t = text.trim_start();
    if t.starts_with("<recommended_plugins>") {
        if let Some(end) = t.find("</recommended_plugins>") {
            t = t[end + "</recommended_plugins>".len()..].trim_start();
        }
    }
    t.starts_with("# AGENTS.md instructions for ") && t.contains("<INSTRUCTIONS>")
}

/// System-injected meta prompts (memory summarizers, compression runs) that
/// should never be shown as a session title.
pub(crate) fn is_system_meta_prompt(text: &str) -> bool {
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
pub async fn session_status(session_id: String) -> SessionStatus {
    crate::run_blocking(move || session_status_sync(session_id)).await
}

fn session_status_sync(session_id: String) -> SessionStatus {
    if panels_denied(&session_id) {
        return SessionStatus::default();
    }
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

/// Who owns `session_id` and where its transcript is, if its engine permits a
/// delete at all.
///
/// The gate on the destructive path, and it lives in Rust rather than in the
/// sidebar because hiding a button is not a boundary: `session_delete` is an
/// IPC command, and a command that destroys data must not depend on the UI
/// having declined to call it.
///
/// The backend comes back with the path because the path is not always the
/// thing to delete. `find_session_file` returns whatever the owning provider
/// considers the session's location, and for OpenCode that is
/// `~/.local/share/opencode/opencode.db` — the single database holding every
/// OpenCode conversation on the machine. The generic delete below is a
/// `rename` into `~/.claude/trash/<id>.jsonl`; run against that path it would
/// move all of them, under one session's name, on one click. So the caller
/// branches on the backend and sends OpenCode ids to a row-level delete that
/// never touches the rename.
///
/// Taken over an explicit backend list so the refusal can be tested with fake
/// backends rather than by having a real engine installed.
/// Whether the owning engine has said the transcript panels do not apply.
///
/// Tasks, artifacts, agents and status all parse Claude Code's record types out
/// of a JSONL transcript. An engine that keeps its sessions some other way
/// answers `find_session_file` with something that is not one — OpenCode's is
/// the database itself — and reading that as a conversation is at best lines of
/// binary. `caps.panels` already hides these in the UI; refusing here is what
/// makes the answer not depend on that.
///
/// Deliberately only refuses when an owner is *found*: a live claude session
/// whose transcript has not landed yet, or one a compaction moved, has no owner
/// by this lookup and must keep the behaviour it has always had.
fn panels_denied(session_id: &str) -> bool {
    crate::agents::owner_in(&crate::agents::backends(), session_id)
        .is_some_and(|(b, _)| !b.caps().panels)
}

fn deletable<'a>(
    list: &'a [Box<dyn crate::agents::AgentBackend>],
    session_id: &str,
) -> Result<(&'a dyn crate::agents::AgentBackend, std::path::PathBuf), String> {
    let (backend, path) =
        crate::agents::owner_in(list, session_id).ok_or("session not found")?;
    if !backend.caps().delete {
        return Err(format!(
            "{} sessions cannot be deleted from aiterm — its store is not aiterm's to move.",
            backend.display_name()
        ));
    }
    Ok((backend, path))
}

/// Delete a session: its transcript jsonl and task store move to
/// ~/.claude/trash (kept for TRASH_KEEP_DAYS as an undo safety net,
/// purged lazily on later deletes). An OpenCode session has no file of its
/// own to move; its rows are dumped to the trash and deleted from
/// `opencode.db` instead.
#[tauri::command]
pub async fn session_delete(session_id: String) -> Result<(), String> {
    crate::run_blocking(move || session_delete_sync(session_id)).await
}

fn session_delete_sync(session_id: String) -> Result<(), String> {
    if session_id.contains('/') || session_id.contains("..") {
        return Err("invalid session id".into());
    }
    let backends = crate::agents::backends();
    let (backend, path) = deletable(&backends, &session_id)?;
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

    // OpenCode first, because for it `path` is the whole database and must
    // never meet the rename below. Its delete dumps the session's rows to
    // `<id>.jsonl` in the trash — readable for the keep window like any other
    // trashed session — then removes exactly those rows.
    if backend.id() == "opencode" {
        return crate::opencode::delete_to_trash(&session_id, &trash);
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
    // Where it came from, so restore can put it back rather than deduce a
    // destination. Deducing worked while every session was claude's and the
    // convention was known; a Codex rollout lives at
    // `~/.codex/sessions/<y>/<m>/<d>/rollout-<stamp>-<id>.jsonl`, which that
    // deduction cannot produce and would silently restore into claude's tree
    // instead — a file neither agent would ever find again.
    //
    // Best-effort: a missing sidecar falls back to the old behaviour, which is
    // still right for everything trashed before this existed.
    let origin = trash.join(format!("{session_id}.origin"));
    if std::fs::write(&origin, path.to_string_lossy().as_bytes()).is_ok() {
        touch(&origin);
    }
    // A Codex conversation is spread across every rollout that shares its
    // session id, and the rename above only took the newest. Leaving the rest
    // means the next scan collapses them straight back into a row: the delete
    // appears to work, then undoes itself, showing older content. Take the
    // whole set. Runs after the rename, so the file already moved is not in
    // the list this finds.
    if backend.id() == "codex" {
        stash_codex_rollouts(&session_id, &trash);
    }
    // Claude Code 2.1.251 keeps a per-session DIRECTORY beside the transcript —
    // `<project>/<sid>/`, holding `subagents/` transcripts and `tool-results/` —
    // so the jsonl alone is no longer the whole session. Left behind it is
    // orphaned litter no scan will ever list again. It rides the same trash
    // mechanism as everything else (rename, never a remove), so restore puts it
    // back and the keep-window purge is what finally lets go of it.
    // [observed: Claude Code 2.1.251]
    if backend.id() == "claude" {
        let sess_dir = path.with_extension("");
        if sess_dir.is_dir() {
            let dir_dest = trash.join(format!("{session_id}.sessiondir"));
            if std::fs::rename(&sess_dir, &dir_dest).is_ok() {
                touch(&dir_dest);
            }
        }
    }
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
pub async fn trash_list() -> Vec<TrashedSession> {
    crate::run_blocking(trash_list_sync).await
}

fn trash_list_sync() -> Vec<TrashedSession> {
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
            // An OpenCode dump is not a transcript at all — its header line
            // carries the name.
            let (title, project_path) = match parse_session(&p, None) {
                Some(s) => (s.title, s.project_path),
                None => crate::opencode::dump_meta(&p).unwrap_or_else(|| {
                    (format!("session {}", &id[..8.min(id.len())]), String::new())
                }),
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
pub async fn trash_restore(session_id: String) -> Result<(), String> {
    crate::run_blocking(move || trash_restore_sync(session_id)).await
}

fn trash_restore_sync(session_id: String) -> Result<(), String> {
    valid_id(&session_id)?;
    let trash = trash_dir().ok_or("no home dir")?;
    let src = trash.join(format!("{session_id}.jsonl"));
    if !src.exists() {
        return Err("session not in trash".into());
    }

    // An OpenCode dump is rows pulled out of `opencode.db`, not a file that
    // ever had a home to go back to. Putting the rows back is a write this
    // app does not make; refusing plainly beats the alternatives — restoring
    // it into claude's tree, or renaming it onto the database. The dump stays
    // readable in the trash for the keep window.
    if crate::opencode::dump_meta(&src).is_some() {
        return Err(
            "OpenCode sessions can't be restored automatically — the full \
             conversation stays readable in ~/.claude/trash until it is purged."
                .into(),
        );
    }

    // Where it was when it was deleted, if that was recorded. Exact beats
    // deduced: it is right for any agent, including ones aiterm does not know
    // about yet, and it survives the layout of a store changing underneath us.
    let origin = trash.join(format!("{session_id}.origin"));
    let home_dir = dirs::home_dir().ok_or("no home dir")?;
    if let Some(dest) = std::fs::read_to_string(&origin)
        .ok()
        .as_deref()
        .and_then(recorded_origin)
        .filter(|p| !is_claude_transcript(p, &home_dir))
    {
        // Refuse rather than overwrite. Something already sitting at that path
        // is a session with the same id that came back by another route, and
        // clobbering it would destroy a real transcript to restore a deleted
        // one — the one outcome worse than a failed restore.
        if dest.exists() {
            return Err(format!("{} already exists", dest.display()));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&src, &dest).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&origin);
        restore_codex_rollouts(&trash, &session_id);
        return restore_claude_sidecars(&trash, &session_id, &dest);
    }

    // No sidecar: trashed before this was recorded, so fall back to deducing a
    // claude project directory from the transcript's cwd. Only ever correct
    // for claude, which is all that could have been trashed back then.
    //
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
    let dest = proj_dir.join(format!("{session_id}.jsonl"));
    std::fs::rename(&src, &dest).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(trash.join(format!("{session_id}.origin")));
    restore_claude_sidecars(&trash, &session_id, &dest)
}

/// The path a `.origin` sidecar names, if it names a usable one.
///
/// The sidecar is a plain path written by this app, so this is a sanity check
/// rather than a trust boundary — but it decides where a `rename` lands, and
/// an empty or relative one would put a transcript somewhere nobody would look
/// for it. Anything that does not read as an absolute path falls back to the
/// old deduction instead.
fn recorded_origin(text: &str) -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(text.trim());
    (p.is_absolute() && p.file_name().is_some()).then_some(p)
}

/// Whether a path is a claude transcript, i.e. lives under
/// `~/.claude/projects`.
///
/// Claude's restore does not put a transcript back where it was — it works out
/// the project directory the cwd belongs in, which quietly repairs one that
/// was filed wrongly. That is worth keeping, so the recorded origin is used
/// only where that reasoning cannot reach: a store whose layout aiterm has no
/// rule for. For claude, deduction still wins.
fn is_claude_transcript(path: &Path, home: &Path) -> bool {
    path.starts_with(home.join(".claude/projects"))
}

/// Put back the two records Claude Code keeps beside a transcript.
///
/// Shared by both restore paths. Only claude has these — a Codex rollout is
/// the whole of a Codex session — so for anything else these are simply
/// absent, which is why nothing here treats a miss as a failure.
/// Move a Codex conversation's remaining rollouts into the trash as a set.
///
/// They go into `<id>.rollouts/` beside the entry rather than alongside it, so
/// `trash_list` still sees exactly one `<id>.jsonl` per deleted session and the
/// preview still reads the newest file. `origins.json` inside records where
/// each came from, because a rollout's path encodes the date it was written
/// (`sessions/<y>/<m>/<d>/`) and nothing else could put it back.
///
/// Best-effort throughout: a rollout that will not move is left where it is
/// rather than failing a delete that has already happened.
fn stash_codex_rollouts(session_id: &str, trash: &Path) {
    let extras = crate::agents::codex_session_files(session_id);
    if extras.is_empty() {
        return;
    }
    let dir = trash.join(format!("{session_id}.rollouts"));
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut origins = serde_json::Map::new();
    for from in extras {
        let Some(name) = from.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if std::fs::rename(&from, dir.join(&name)).is_ok() {
            origins.insert(name, serde_json::Value::String(from.to_string_lossy().into_owned()));
        }
    }
    let _ = std::fs::write(
        dir.join("origins.json"),
        serde_json::Value::Object(origins).to_string(),
    );
}

/// Put a Codex conversation's stashed rollouts back where they came from.
///
/// A rollout already sitting at the destination is left alone: that is a file
/// that came back by another route, and overwriting it to restore an older copy
/// is the one outcome worse than an incomplete restore.
fn restore_codex_rollouts(trash: &Path, session_id: &str) {
    let dir = trash.join(format!("{session_id}.rollouts"));
    if !dir.is_dir() {
        return;
    }
    let Ok(raw) = std::fs::read_to_string(dir.join("origins.json")) else {
        return;
    };
    let Ok(origins) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw)
    else {
        return;
    };
    for (name, dest) in origins {
        let Some(dest) = dest.as_str().map(std::path::PathBuf::from) else {
            continue;
        };
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::rename(dir.join(&name), &dest);
    }
    let _ = std::fs::remove_file(dir.join("origins.json"));
    // Only if everything left with it; anything that would not move stays in
    // the trash rather than being silently dropped.
    let _ = std::fs::remove_dir(&dir);
}

fn restore_claude_sidecars(
    trash: &Path,
    session_id: &str,
    transcript_dest: &Path,
) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("no home dir")?;
    let tasks_src = trash.join(format!("{session_id}.tasks"));
    if tasks_src.is_dir() {
        let _ = std::fs::rename(&tasks_src, home.join(".claude/tasks").join(session_id));
    }
    let job_src = trash.join(format!("{session_id}.job"));
    if job_src.is_dir() {
        let jobs = home.join(".claude/jobs");
        let _ = std::fs::create_dir_all(&jobs);
        let _ = std::fs::rename(&job_src, jobs.join(job_dir_name(&job_src, session_id)));
    }
    // The per-session directory (`<project>/<sid>/` — subagents, tool-results;
    // see session_delete) goes back beside wherever the transcript landed. One
    // already sitting there came back by another route and is left alone, per
    // the rollout-restore stance. [observed: Claude Code 2.1.251]
    let dir_src = trash.join(format!("{session_id}.sessiondir"));
    if dir_src.is_dir() {
        let dir_dest = transcript_dest.with_extension("");
        if !dir_dest.exists() {
            let _ = std::fs::rename(&dir_src, &dir_dest);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn trash_delete(session_id: String) -> Result<(), String> {
    crate::run_blocking(move || trash_delete_sync(session_id)).await
}

fn trash_delete_sync(session_id: String) -> Result<(), String> {
    valid_id(&session_id)?;
    let trash = trash_dir().ok_or("no home dir")?;
    std::fs::remove_file(trash.join(format!("{session_id}.jsonl"))).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(trash.join(format!("{session_id}.origin")));
    let tasks = trash.join(format!("{session_id}.tasks"));
    if tasks.is_dir() {
        let _ = std::fs::remove_dir_all(tasks);
    }
    let job = trash.join(format!("{session_id}.job"));
    if job.is_dir() {
        let _ = std::fs::remove_dir_all(job);
    }
    let sess_dir = trash.join(format!("{session_id}.sessiondir"));
    if sess_dir.is_dir() {
        let _ = std::fs::remove_dir_all(sess_dir);
    }
    // The rest of a Codex conversation goes with it — the entry that named
    // them is gone, so leaving them would be leaking a set nothing can restore.
    let rollouts = trash.join(format!("{session_id}.rollouts"));
    if rollouts.is_dir() {
        let _ = std::fs::remove_dir_all(rollouts);
    }
    Ok(())
}

#[tauri::command]
pub async fn trash_empty() -> Result<(), String> {
    crate::run_blocking(trash_empty_sync).await
}

fn trash_empty_sync() -> Result<(), String> {
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
/// Whether a line is worth parsing as JSON at all.
///
/// Both formats are one JSON object per line and most lines are neither a
/// user nor an assistant message — tool calls, token counts, world state. A
/// substring test costs a fraction of a parse, and transcripts run to
/// megabytes.
pub(crate) fn line_may_hold_message(line: &str) -> bool {
    line.contains("\"type\":\"user\"")
        || line.contains("\"type\":\"assistant\"")
        || line.contains("\"type\":\"response_item\"")
}

/// The conversation message on one transcript line, as `(role, text)`.
///
/// One function for every agent rather than one per agent, because the two
/// callers — the preview panel and the search indexer — must agree about what
/// a session contains. They did not: both understood claude's shape only, so a
/// Codex session previewed as blank and was indexed as having said nothing,
/// while still appearing in the sidebar. A row you can see, cannot read and
/// cannot find is worse than no row.
///
/// The shapes are distinct enough to handle in one pass, so nothing has to
/// know which agent wrote the file:
///
/// - claude: `{"type":"user"|"assistant","message":{"content":…}}`, where
///   content is a string or blocks of `{"type":"text","text":…}`.
/// - Codex: `{"type":"response_item","payload":{"type":"message","role":…,
///   "content":[{"type":"input_text"|"output_text","text":…}]}}`.
///
/// Codex's `developer` role is dropped. It carries the sandbox and permission
/// preamble, which is not something you said and not something worth matching
/// a search against — the same reason claude's meta-prompts are filtered.
pub(crate) fn line_message(v: &serde_json::Value) -> Option<(String, String)> {
    let (role, content) = match v.get("type").and_then(|t| t.as_str()) {
        Some(r @ ("user" | "assistant")) => (r.to_string(), v.pointer("/message/content")?),
        Some("response_item") => {
            let p = v.get("payload")?;
            if p.get("type").and_then(|t| t.as_str()) != Some("message") {
                return None;
            }
            match p.get("role").and_then(|r| r.as_str()) {
                Some(r @ ("user" | "assistant")) => (r.to_string(), p.get("content")?),
                _ => return None,
            }
        }
        _ => return None,
    };

    let mut text = String::new();
    match content {
        serde_json::Value::String(s) => text.push_str(s),
        serde_json::Value::Array(blocks) => {
            for b in blocks {
                // `text` for claude, `input_text`/`output_text` for Codex.
                // Matched on having text rather than on the tag, so a new tag
                // for the same thing does not silently empty a transcript.
                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
        }
        _ => return None,
    }
    (!text.trim().is_empty()).then_some((role, text))
}

#[tauri::command]
pub async fn session_preview(session_id: String) -> Vec<PreviewMsg> {
    crate::run_blocking(move || session_preview_sync(session_id)).await
}

/// How many messages the preview keeps, and how much of each.
const PREVIEW_KEEP: usize = 12;
const PREVIEW_MAX_CHARS: usize = 700;

fn session_preview_sync(session_id: String) -> Vec<PreviewMsg> {
    // The owning backend, not just the file: a backend may keep its sessions
    // somewhere that is not a file at all, and OpenCode does — its
    // `find_session_file` answers with `opencode.db`, which read as a
    // transcript is binary noise. Ask for the conversation first; only fall
    // back to opening the path when the owner says it has none to give, which
    // is every other engine.
    let list = crate::agents::backends();
    let Some((backend, path)) = crate::agents::owner_in(&list, &session_id) else {
        return vec![];
    };
    if let Some(msgs) = backend.sessions().messages(&session_id) {
        return preview_from_messages(msgs);
    }
    let Ok(file) = File::open(&path) else {
        return vec![];
    };
    let mut out: std::collections::VecDeque<PreviewMsg> = std::collections::VecDeque::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // Cheap substring filter before JSON parsing.
        if !line_may_hold_message(&line) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        // Subagent traffic isn't part of the main conversation.
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        let Some((role, text)) = line_message(&v) else {
            continue;
        };
        // On the RAW text: stripped, the codex preamble is a bare header
        // line the empty-check below would keep.
        if role == "user" && is_codex_agents_preamble(&text) {
            continue;
        }
        let text = strip_system_tags(&text);
        if text.trim().is_empty() || (role == "user" && is_system_meta_prompt(&text)) {
            continue;
        }
        let truncated = text.chars().count() > PREVIEW_MAX_CHARS;
        let mut text: String = text.chars().take(PREVIEW_MAX_CHARS).collect();
        if truncated {
            text.push('…');
        }
        let at = v.get("timestamp").and_then(|t| t.as_str()).map(String::from);
        out.push_back(PreviewMsg { role, text, at });
        if out.len() > PREVIEW_KEEP {
            out.pop_front();
        }
    }
    out.into()
}

/// Preview rows from a conversation the owning backend handed over whole.
///
/// The same shaping the file path applies — system-tag stripping, the
/// `PREVIEW_MAX_CHARS` truncation with its ellipsis, the last `PREVIEW_KEEP`
/// messages — so a preview looks the same whether it came from a transcript or
/// from a database. `at` is `None`: the timestamp on a claude row is an ISO
/// string off the transcript line, and `(role, text)` carries no such thing.
fn preview_from_messages(msgs: Vec<(String, String)>) -> Vec<PreviewMsg> {
    let mut out: Vec<PreviewMsg> = msgs
        .into_iter()
        .filter_map(|(role, text)| {
            if role == "user" && is_codex_agents_preamble(&text) {
                return None;
            }
            let text = strip_system_tags(&text);
            if text.trim().is_empty() || (role == "user" && is_system_meta_prompt(&text)) {
                return None;
            }
            let truncated = text.chars().count() > PREVIEW_MAX_CHARS;
            let mut text: String = text.chars().take(PREVIEW_MAX_CHARS).collect();
            if truncated {
                text.push('…');
            }
            Some(PreviewMsg { role, text, at: None })
        })
        .collect();
    if out.len() > PREVIEW_KEEP {
        out.drain(..out.len() - PREVIEW_KEEP);
    }
    out
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
pub async fn session_tasks(session_id: String) -> Vec<SessionTask> {
    crate::run_blocking(move || session_tasks_sync(session_id)).await
}

fn session_tasks_sync(session_id: String) -> Vec<SessionTask> {
    let list = crate::agents::backends();
    if let Some((owner, _)) = crate::agents::owner_in(&list, &session_id) {
        if !owner.caps().tasks {
            return vec![];
        }
        // An engine that records tasks in its own shape answers through its
        // provider; claude's `None` falls through to the transcript scan.
        if let Some(tasks) = owner.sessions().tasks(&session_id) {
            return tasks;
        }
    }
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
pub async fn session_agents(session_id: String) -> Vec<AgentRun> {
    crate::run_blocking(move || session_agents_sync(session_id)).await
}

fn session_agents_sync(session_id: String) -> Vec<AgentRun> {
    if panels_denied(&session_id) {
        return vec![];
    }
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
pub async fn resolve_resumable_id(session_id: String) -> Option<String> {
    crate::run_blocking(move || resolve_resumable_id_sync(session_id)).await
}

fn resolve_resumable_id_sync(session_id: String) -> Option<String> {
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

/// Frontend-to-journal logging. The webview's console goes nowhere in a
/// release build; errors the UI swallows (a failed invoke, a rejected promise)
/// were invisible, which is how a dead code path survives testing. Low volume:
/// callers log outcomes and errors, not chatter.
#[tauri::command]
pub fn ui_log(msg: String) {
    crate::diag!("ui", "{msg}");
}

/// Why a tab's pinned session id stopped being the live one. The two need
/// telling apart, because they leave the old conversation in opposite states.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveKind {
    /// The agents view moved the conversation to the daemon. The old id is a
    /// dead end — it holds the history, but nothing will write it again.
    Background,
    /// `/clear` started a fresh conversation in the same terminal. The old id is
    /// a complete conversation that stays independently resumable, so its row
    /// belongs in the sidebar; what changes is which row the tab owns.
    Cleared,
}

/// The session that took over a tab's conversation, and how.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SessionMove {
    pub id: String,
    pub kind: MoveKind,
}

/// The session that took over `session_id`'s conversation, if one has. `None`
/// is the normal answer. Two things do this, and they are detected differently
/// because they leave completely different traces.
///
/// **Migration to the daemon.** Opening Claude Code's agents view — left arrow,
/// on an empty prompt — moves the running conversation to the daemon. What lands
/// on disk is a *new* transcript under a new session id: the original stops at
/// that instant and never moves again, while the pty in the tab goes on
/// rendering the child. A tab pinned to the parent then shows live text over
/// dead panels — its clock stops and Agents/Tasks/Artifacts read a file nothing
/// is writing.
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
///
/// **`/clear`.** Same visible symptom, no shared trace at all: the child's
/// first record has `parentUuid: null`, it is not `bg`, and not one uuid is
/// common to the two files, so the rule above is structurally blind to it. That
/// blindness is what left a cleared tab pinned to a frozen transcript while its
/// live conversation sat unowned in the sidebar — click that row and a *second*
/// claude opens on it. Detected instead from the child's own head, which carries
/// Claude Code's echo of the command that made it (see [`CLEAR_ECHO`]), paired
/// with being the first transcript written after the parent stopped.
#[tauri::command]
pub async fn session_moved_to(session_id: String) -> Option<SessionMove> {
    crate::run_blocking(move || session_moved_to_sync(session_id)).await
}

fn session_moved_to_sync(session_id: String) -> Option<SessionMove> {
    let out = session_moved_to_inner(&session_id);
    // Only the answer, not the asking. This polls every 15s per active tab, and
    // a line per poll buried the one line that mattered — which is the failure
    // mode a log is supposed to prevent.
    if let Some(moved) = &out {
        crate::diag!(
            "session",
            "{session_id} moved to {} ({:?})",
            moved.id,
            moved.kind
        );
    }
    out
}

fn session_moved_to_inner(session_id: &str) -> Option<SessionMove> {
    let parent = find_session_file(session_id)?;
    if parent
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains(".orphaned-"))
    {
        return None; // retired transcripts are resolve_live_session_file's job
    }
    moved_to_in_dir(&parent)
}

/// Split from `session_moved_to_inner` so the rules can be tested against a
/// temp directory: everything above this point resolves a session id through
/// the real `~/.claude`, and everything below is decided by the files alone.
///
/// Instrumented because this is where the decision gets made, and when it
/// decides wrong the only useful question is which candidate it saw and what it
/// believed about each one. Compiled away in release (see [`crate::trace`]).
#[tracing::instrument(level = "debug", skip_all, fields(parent = ?parent.file_name()))]
fn moved_to_in_dir(parent: &Path) -> Option<SessionMove> {
    let parent_mtime = mtime_of(parent).unwrap_or(0);
    let dir = parent.parent()?;

    // Every sibling written no earlier than the instant the parent stopped — a
    // child cannot predate its parent's last word. Oldest first, because the
    // `/clear` rule below turns on which one came *next*.
    let mut siblings: Vec<(u64, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path == parent || path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if name.contains(".orphaned-") {
            continue;
        }
        let m = mtime_of(&path).unwrap_or(0);
        if m >= parent_mtime {
            siblings.push((m, path));
        }
    }
    siblings.sort_by(|a, b| a.0.cmp(&b.0));
    tracing::debug!(
        parent_mtime,
        candidates = siblings.len(),
        oldest = ?siblings.first().map(|(_, p)| p.file_name()),
        "siblings written since the parent stopped"
    );

    // Daemon migration first: it is the stronger claim of the two, checked
    // against the parent's own uuids rather than inferred from timing, so the
    // newest sibling that satisfies it wins.
    for (_, path) in siblings.iter().rev() {
        let Some(facts) = read_head_facts(path) else {
            continue;
        };
        if facts.is_bg && !facts.links.is_empty() && file_has_any_uuid(parent, &facts.links) {
            if let Some(stem) = path.file_stem() {
                return Some(SessionMove {
                    id: stem.to_string_lossy().into_owned(),
                    kind: MoveKind::Background,
                });
            }
        }
    }

    // `/clear`, which leaves nothing to verify against: the child shares no
    // ancestry with the parent, so timing is the only link there is. Only the
    // *first* transcript written after the parent stopped is eligible — that is
    // what "what this terminal did next" means, and it is what keeps a second
    // idle tab in the same project from being re-keyed onto someone else's
    // cleared conversation.
    //
    // The honest limitation: two idle tabs in one project, and the disk cannot
    // say which of them was cleared. This rule picks the one that spoke last,
    // which is the one a person was just using. When it picks wrong the cost is
    // no re-key, not a tab pointed at a stranger's conversation — which is the
    // safe direction to be wrong in, and the reason the rule is this narrow.
    let (_, first) = siblings.first()?;
    let facts = read_head_facts(first)?;
    tracing::debug!(
        candidate = ?first.file_name(),
        born_from_clear = facts.born_from_clear,
        links = facts.links.len(),
        "testing the first transcript written after the parent stopped"
    );
    if facts.born_from_clear && !file_has_any_uuid(parent, &facts.links) {
        return Some(SessionMove {
            id: first.file_stem()?.to_string_lossy().into_owned(),
            kind: MoveKind::Cleared,
        });
    }
    None
}

/// Claude Code's own echo of the slash command that opened a transcript.
///
/// `/clear` keeps the terminal and starts a brand-new session id, and this
/// marker in the new transcript's head is the only record anywhere that says
/// why it exists — the child shares no uuid, no `sessionKind` and no job-state
/// link with the conversation it replaced. Measured against a specimen captured
/// 2026-07-29 (Claude Code 2.1.220): the child's first real user record is this
/// echo, and its first record's `parentUuid` is `null`.
const CLEAR_ECHO: &str = "<command-name>/clear</command-name>";

/// What a transcript's own head says about where it came from.
struct HeadFacts {
    /// uuids it claims as ancestry, via `parentUuid`/`logicalParentUuid`.
    links: std::collections::HashSet<String>,
    /// Runs under the daemon (`sessionKind: "bg"`).
    is_bg: bool,
    /// Its first real user turn is the `/clear` echo. "First" matters: it is
    /// what separates a transcript *created by* the command from one that
    /// merely mentions it in a later prompt.
    born_from_clear: bool,
}

/// Read only the head of a transcript — copied history and the opening command
/// both sit at the front, so scanning a multi-megabyte tail to re-learn the
/// same answer is waste.
fn read_head_facts(path: &Path) -> Option<HeadFacts> {
    const MAX_RECORDS: usize = 500;
    const MAX_LINKS: usize = 64;
    let file = File::open(path).ok()?;
    let mut links = std::collections::HashSet::new();
    let mut is_bg = false;
    let mut born_from_clear = false;
    // Set once the opening turn has been seen, so nothing later can change the
    // answer about how this transcript started.
    let mut opening_settled = false;
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
        if opening_settled {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            // A reply means the conversation was already under way, so whatever
            // opened it was not a command echo.
            Some("assistant") => opening_settled = true,
            // Claude Code writes its own bookkeeping as `user` records flagged
            // `isMeta` — the local-command caveat sits directly in front of the
            // echo — so those are not the opening turn.
            Some("user") if v.get("isMeta").and_then(|m| m.as_bool()) != Some(true) => {
                opening_settled = true;
                born_from_clear = message_text(&v).contains(CLEAR_ECHO);
            }
            _ => {}
        }
    }
    Some(HeadFacts {
        links,
        is_bg,
        born_from_clear,
    })
}

/// A record's message content as searchable text. Content is a bare string for
/// command echoes and an array of blocks for ordinary turns; serialising the
/// non-string case keeps one match arm working for both.
fn message_text(v: &serde_json::Value) -> String {
    match v.pointer("/message/content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
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
pub async fn running_session_ids() -> Vec<String> {
    crate::run_blocking(running_session_ids_sync).await
}

fn running_session_ids_sync() -> Vec<String> {
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
/// whose conversation has moved on is a renderer, not a conversation. The
/// client process stays alive under the old id for as long as the tab is open,
/// so the roster reports it forever — and its row wore a green dot over a
/// transcript nothing will ever write again. If the same linkage the tab re-key
/// trusts says the conversation moved, the old id is not "alive" in any sense
/// the sidebar should report. Background entries are never filtered: the
/// moved-to session IS the live one.
///
/// Both kinds of move count here. A `/clear`ed id is as finished as a migrated
/// one — and dropping it has a second benefit, since a session with no live
/// process resumes in place instead of being forked.
///
/// Cost note: the scan is mtime-gated, so for a healthy interactive session
/// (its own transcript newest in the dir) it rejects every candidate without
/// reading them.
#[tauri::command]
pub async fn live_session_ids() -> Vec<String> {
    crate::run_blocking(live_session_ids_sync).await
}

fn live_session_ids_sync() -> Vec<String> {
    read_roster()
        .into_iter()
        .filter(|e| e.background || session_moved_to_inner(&e.session_id).is_none())
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
/// a session with `state: "done"` — and pid-less `state: "blocked"` corpses
/// weeks after their processes died — so "appears in the roster" is not the
/// same question as "is running": counting those made dead sessions look alive
/// and suppressed Resume on rows that were perfectly resumable.
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

/// Read the roster straight from Claude Code's own live-session registry:
/// one `~/.claude/sessions/<pid>.json` per live client, written on start,
/// deleted on clean exit, covering interactive and background sessions alike.
/// `None` means the registry isn't there to read (old claude, moved dir) —
/// the caller falls back to asking the CLI.
fn roster_from_dir(dir: &Path) -> Option<Vec<RosterEntry>> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let Ok(raw) = std::fs::read(entry.path()) else {
            continue;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            continue;
        };
        let Some(session_id) = v.get("sessionId").and_then(|s| s.as_str()) else {
            continue;
        };
        let Some(pid) = v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32) else {
            continue;
        };
        // A file describing a process that is gone — or a *different* process
        // the kernel has since reissued the pid to — is a crash leftover, not
        // a session. procStart is the incarnation check; a file without one
        // (older claude) gets plain existence.
        let live = match v.get("procStart").and_then(|p| p.as_str()) {
            Some(want) => proc_starttime(pid).as_deref() == Some(want),
            None => crate::pty::pid_alive(pid),
        };
        if !live {
            continue;
        }
        out.push(RosterEntry {
            session_id: session_id.to_owned(),
            pid: Some(pid),
            // The files say "bg"; only the CLI's output says "background".
            background: v.get("kind").and_then(|k| k.as_str()) == Some("bg"),
        });
    }
    Some(out)
}

/// The process's start time — field 22 of `/proc/<pid>/stat`, the same value
/// the registry stores as `procStart`. Comparing them is what separates "this
/// file describes the process that holds pid N" from a leftover of a crashed
/// client whose pid the kernel has since reissued.
fn proc_starttime(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm (field 2) is parenthesized and may itself hold spaces or parens,
    // so field counting is only safe after the *last* ')'. starttime is field
    // 22 overall = 20th after state, which is the first past the comm.
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19).map(str::to_owned)
}

fn read_roster_uncached() -> Vec<RosterEntry> {
    // The registry files are the cheap, authoritative source — reading them
    // costs microseconds where `claude agents --json` costs a whole Node
    // process (~0.26s wall, ~300 MB RSS, measured 2026-07-27), and that spawn
    // used to run on the main thread every TTL expiry. The CLI remains for
    // two cases the files cannot answer:
    //
    //   - the dir is missing entirely (older claude, relocated state), and
    //   - a background entry is present: whether a bg job has *finished* is
    //     computed by the CLI (`state: "done"`, observed 2026-08-01 with the
    //     client pid still alive), and a finished job must not wear a live dot.
    if let Some(entries) = dirs::home_dir()
        .map(|h| h.join(".claude/sessions"))
        .and_then(|d| roster_from_dir(&d))
    {
        if entries.iter().all(|e| !e.background) {
            return entries;
        }
    }
    roster_from_cli()
}

fn roster_from_cli() -> Vec<RosterEntry> {
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
        // `state:"blocked"` with no `pid` field is a dead entry the daemon
        // never reaped: a roster read on 2026-08-31 listed a dozen of them
        // with weeks-old `startedAt` and no matching process anywhere.
        // Counted as live they wear green dots forever and land on the
        // unstoppable list, warning Resume off perfectly resumable rows. A
        // *live* blocked session reports a pid (observed beside them: a
        // blocked bg entry whose `bg-spare` pid was alive) and is kept —
        // blocked-with-a-dead-pid falls to the pid check below.
        // [observed: Claude Code 2.1.251]
        .filter(|a| {
            a.get("state").and_then(|s| s.as_str()) != Some("blocked") || a.get("pid").is_some()
        })
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
pub async fn bg_agent_session_ids() -> Vec<String> {
    crate::run_blocking(bg_agent_session_ids_sync).await
}

fn bg_agent_session_ids_sync() -> Vec<String> {
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
pub async fn unstoppable_session_ids() -> Vec<String> {
    crate::run_blocking(unstoppable_session_ids_sync).await
}

fn unstoppable_session_ids_sync() -> Vec<String> {
    read_roster()
        .into_iter()
        .filter(|e| e.background || e.pid.is_none())
        .map(|e| e.session_id)
        .collect()
}

/// Files this session created or modified, newest first — parsed from
/// Write/Edit/NotebookEdit tool calls in the transcript.
#[tauri::command]
pub async fn session_artifacts(session_id: String) -> Vec<Artifact> {
    crate::run_blocking(move || session_artifacts_sync(session_id)).await
}

fn session_artifacts_sync(session_id: String) -> Vec<Artifact> {
    let list = crate::agents::backends();
    if let Some((owner, _)) = crate::agents::owner_in(&list, &session_id) {
        if !owner.caps().tasks {
            return vec![];
        }
        if let Some(artifacts) = owner.sessions().artifacts(&session_id) {
            return artifacts;
        }
    }
    let Some(path) = resolve_live_session_file(&session_id) else {
        return vec![];
    };
    let mut latest: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    scan_artifact_writes(&path, &mut latest);
    // Subagent writes belong to this session too. Claude Code 2.1.251 keeps
    // sidechain transcripts as separate files — `<project>/<sid>/subagents/
    // agent-<agentId>.jsonl`, records stamped with the *parent's* sessionId —
    // no longer inline in the main jsonl, so a file a subagent wrote never
    // appeared here. Same record shapes as the main transcript, same scan.
    // Older transcripts with inline `isSidechain:true` records keep working:
    // the main-file scan never filtered on it. [observed: Claude Code 2.1.251]
    let subagents = path.with_extension("").join("subagents");
    if let Ok(rd) = std::fs::read_dir(&subagents) {
        let mut files: Vec<std::path::PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        // Deterministic merge order; ties on a path still resolve by timestamp.
        files.sort();
        for f in files {
            scan_artifact_writes(&f, &mut latest);
        }
    }
    let mut artifacts: Vec<Artifact> = latest
        .into_iter()
        .map(|(path, (tool, at))| Artifact { path, tool, at })
        .collect();
    artifacts.sort_by(|a, b| b.at.cmp(&a.at));
    artifacts
}

/// One transcript's Write/Edit/NotebookEdit/MultiEdit `tool_use` calls, merged
/// into `latest` keyed by file path — the newest call per path wins, compared
/// by the record's ISO timestamp so merging the main transcript with subagent
/// transcripts cannot let an older write shadow a newer one.
fn scan_artifact_writes(
    path: &Path,
    latest: &mut std::collections::HashMap<String, (String, String)>,
) {
    let Ok(file) = File::open(path) else {
        return;
    };
    const TOOLS: [&str; 4] = ["Write", "Edit", "NotebookEdit", "MultiEdit"];
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
                // ISO timestamps order lexicographically; `>=` keeps the old
                // last-line-wins behaviour within a single file.
                let newer = latest.get(fp).is_none_or(|(_, at)| ts.as_str() >= at.as_str());
                if newer {
                    latest.insert(fp.to_string(), (tool.to_string(), ts.clone()));
                }
            }
        }
    }
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
pub async fn session_model(session_id: String) -> ModelChoice {
    crate::run_blocking(move || session_model_sync(session_id)).await
}

fn session_model_sync(session_id: String) -> ModelChoice {
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

/// A classifier refusal the transcript recorded — the trigger for the
/// downgrade lane's one-tap actions.
///
/// Every field is copied straight from the record, which self-describes the
/// event; nothing here is inferred. `original_model` is what the session was on
/// before the switch (what "restore my model" targets), `refused_prompt` is the
/// message that got flagged (what "kick to OpenCode" hands off).
#[derive(Serialize, Debug, PartialEq, Default)]
pub struct Refusal {
    /// The refusal record's own uuid. The frontend remembers the last one it
    /// raised so an old refusal in the tail never re-triggers the banner.
    pub uuid: String,
    /// `model_refusal_fallback` (soft switch to the fallback model) or
    /// `model_refusal_no_fallback` (hard block, no switch happened).
    pub subtype: String,
    /// The hard-block case: nothing auto-switched, so restoring is the only move.
    pub hard: bool,
    /// The classifier notice, shown to the user verbatim.
    pub content: String,
    /// The model in use before the refusal — the target of "restore my model".
    pub original_model: Option<String>,
    /// What it switched to (typically `claude-opus-4-8`).
    pub fallback_model: Option<String>,
    /// The category the classifier assigned (e.g. `cyber`).
    pub category: Option<String>,
    /// The flagged user message's text — the prompt to hand OpenCode.
    pub refused_prompt: Option<String>,
    /// ISO-8601 time the refusal was recorded.
    pub at: Option<String>,
}

/// The most recent classifier refusal in `session_id`'s transcript, if the tail
/// holds one.
///
/// A `type:system` record whose `subtype` starts with `model_refusal`. The
/// prefix match is deliberate: `_fallback` and `_no_fallback` are the only two
/// today, but a future variant should still raise the banner rather than slip
/// through silently. Returns the last such record; `uuid` lets the caller tell a
/// fresh refusal from one it has already handled.
#[tauri::command]
pub async fn session_refusal(session_id: String) -> Option<Refusal> {
    crate::run_blocking(move || session_refusal_sync(session_id)).await
}

fn session_refusal_sync(session_id: String) -> Option<Refusal> {
    let path = find_session_file(&session_id)?;
    // Wider than the model reader's window: this must also reach back to the
    // flagged user message the refusal points at, which sits just before it.
    const TAIL: u64 = 512 * 1024;
    let mut file = File::open(&path).ok()?;
    let meta = file.metadata().ok()?;
    let start = meta.len().saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    parse_refusal(&text)
}

/// The scan behind [`session_refusal`], split out so the record shape can be
/// tested against captured lines without a transcript on disk.
fn parse_refusal(text: &str) -> Option<Refusal> {
    let mut latest: Option<serde_json::Value> = None;
    for line in text.lines() {
        // Cheap substring gate before the parse, matching the other tail scans.
        if !line.contains("model_refusal") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("system") {
            continue;
        }
        if v.get("subtype")
            .and_then(|s| s.as_str())
            .is_some_and(|s| s.starts_with("model_refusal"))
        {
            latest = Some(v);
        }
    }
    let r = latest?;
    let subtype = r.get("subtype").and_then(|s| s.as_str())?.to_string();
    let str_of = |k: &str| r.get(k).and_then(|x| x.as_str()).map(String::from);
    let refused_prompt = r
        .get("refusedUserMessageUuid")
        .and_then(|u| u.as_str())
        .and_then(|uu| user_text_by_uuid(text, uu));
    Some(Refusal {
        uuid: str_of("uuid").unwrap_or_default(),
        hard: subtype.ends_with("no_fallback"),
        content: str_of("content").unwrap_or_default(),
        original_model: str_of("originalModel"),
        fallback_model: str_of("fallbackModel"),
        category: str_of("apiRefusalCategory"),
        refused_prompt,
        at: str_of("timestamp"),
        subtype,
    })
}

/// The text of the user message with this uuid, from the tail lines. Best-effort:
/// if the flagged message fell outside the tail window, the caller falls back to
/// the session preview's last user message.
fn user_text_by_uuid(text: &str, uuid: &str) -> Option<String> {
    for line in text.lines() {
        if !line.contains(uuid) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("uuid").and_then(|u| u.as_str()) != Some(uuid) {
            continue;
        }
        return line_message(&v).map(|(_, t)| t);
    }
    None
}

#[cfg(test)]
mod refusal_tests {
    use super::*;

    /// A soft fallback record parses into every field, and the flagged prompt is
    /// resolved by the uuid the record names.
    #[test]
    fn a_soft_fallback_parses_and_finds_its_flagged_prompt() {
        let text = concat!(
            r#"{"type":"user","uuid":"u-1","message":{"role":"user","content":"scan the firewall logs for lateral movement"}}"#, "\n",
            r#"{"type":"system","subtype":"model_refusal_fallback","content":"Fable 5's safeguards flagged this message.","originalModel":"claude-fable-5","fallbackModel":"claude-opus-4-8","apiRefusalCategory":"cyber","refusedUserMessageUuid":"u-1","uuid":"r-1","timestamp":"2026-08-12T00:12:26.402Z"}"#, "\n",
        );
        let r = parse_refusal(text).expect("a refusal");
        assert_eq!(r.uuid, "r-1");
        assert_eq!(r.subtype, "model_refusal_fallback");
        assert!(!r.hard);
        assert_eq!(r.original_model.as_deref(), Some("claude-fable-5"));
        assert_eq!(r.fallback_model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(r.category.as_deref(), Some("cyber"));
        assert_eq!(
            r.refused_prompt.as_deref(),
            Some("scan the firewall logs for lateral movement")
        );
    }

    /// The hard-block variant is flagged `hard`, and the prefix match means a
    /// hypothetical third variant is still caught.
    #[test]
    fn a_hard_block_is_marked_hard_and_the_prefix_is_forgiving() {
        let hard = r#"{"type":"system","subtype":"model_refusal_no_fallback","content":"","uuid":"r-2"}"#;
        assert!(parse_refusal(hard).unwrap().hard);
        let future = r#"{"type":"system","subtype":"model_refusal_future_variant","uuid":"r-3"}"#;
        assert_eq!(parse_refusal(future).unwrap().uuid, "r-3");
    }

    /// The whole point of the multi-file delete: every rollout of a Codex
    /// conversation goes to the trash together, and a restore puts every one of
    /// them back where it came from. Anything left behind on delete is
    /// collapsed straight back into a sidebar row; anything not restored is a
    /// conversation that comes back with holes in it.
    ///
    /// Ignored because it points `$HOME` at a synthetic store for the whole
    /// process. Run it alone:
    ///   cargo test --lib codex_rollouts_round_trip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn codex_rollouts_round_trip() {
        let root = std::env::temp_dir().join("aiterm-codex-trash-roundtrip");
        let _ = std::fs::remove_dir_all(&root);
        let day = root.join(".codex/sessions/2026/08/16");
        std::fs::create_dir_all(&day).unwrap();
        let trash = root.join("trash");
        std::fs::create_dir_all(&trash).unwrap();
        std::env::set_var("HOME", &root);

        let sid = "01a00b9c-0011-7973-a7ac-759454839aaf";
        let made: Vec<std::path::PathBuf> = ["rollout-1.jsonl", "rollout-2.jsonl", "rollout-3.jsonl"]
            .iter()
            .map(|n| {
                let p = day.join(n);
                std::fs::write(
                    &p,
                    format!("{{\"payload\":{{\"session_id\":\"{sid}\",\"cwd\":\"/home/m/p\"}}}}\n"),
                )
                .unwrap();
                p
            })
            .collect();

        stash_codex_rollouts(sid, &trash);
        let stashed = trash.join(format!("{sid}.rollouts"));
        assert!(stashed.is_dir(), "the set went to the trash together");
        for p in &made {
            assert!(!p.exists(), "{} should have left the store", p.display());
        }
        assert!(stashed.join("origins.json").is_file(), "where they came from is recorded");

        restore_codex_rollouts(&trash, sid);
        for p in &made {
            assert!(p.exists(), "{} should be back where it was", p.display());
        }
        assert!(!stashed.exists(), "nothing left in the trash once restored");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The last refusal wins, and a transcript with none yields nothing.
    #[test]
    fn the_latest_refusal_wins_and_a_clean_tail_is_none() {
        let two = concat!(
            r#"{"type":"system","subtype":"model_refusal_fallback","uuid":"old"}"#, "\n",
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8"}}"#, "\n",
            r#"{"type":"system","subtype":"model_refusal_fallback","uuid":"new"}"#, "\n",
        );
        assert_eq!(parse_refusal(two).unwrap().uuid, "new");
        assert_eq!(
            parse_refusal(r#"{"type":"assistant","message":{"model":"claude-fable-5"}}"#),
            None
        );
    }
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
pub async fn claude_permission_mode(project_path: String) -> Option<String> {
    crate::run_blocking(move || claude_permission_mode_sync(project_path)).await
}

fn claude_permission_mode_sync(project_path: String) -> Option<String> {
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
pub async fn claude_model_default() -> Option<String> {
    crate::run_blocking(claude_model_default_sync).await
}

fn claude_model_default_sync() -> Option<String> {
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
        if claude_model_default_sync() != previous {
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
pub async fn session_fork(session_id: String) -> Result<String, String> {
    crate::run_blocking(move || session_fork_sync(session_id)).await
}

fn session_fork_sync(session_id: String) -> Result<String, String> {
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
pub async fn materialize_fork(session_id: String) -> Result<(), String> {
    crate::run_blocking(move || materialize_fork_sync(session_id)).await
}

fn materialize_fork_sync(session_id: String) -> Result<(), String> {
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(line: &str) -> Option<(String, String)> {
        assert!(
            line_may_hold_message(line),
            "prefilter would drop this line before it was ever parsed"
        );
        line_message(&serde_json::from_str(line).unwrap())
    }

    /* ---- the delete gate ------------------------------------------------ */

    /// A backend that claims a session, over a path that does not exist and is
    /// never touched: what is under test is the refusal, which must come before
    /// anything reaches the filesystem.
    struct FakeOwner {
        id: &'static str,
        can_delete: bool,
    }

    impl SessionProvider for FakeOwner {
        fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)> {
            vec![]
        }
        fn find_session_file(&self, session_id: &str) -> Option<std::path::PathBuf> {
            (session_id == "owned").then(|| std::path::PathBuf::from("/nonexistent/store.db"))
        }
    }

    impl crate::agents::AgentBackend for FakeOwner {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &'static str {
            "Fake Engine"
        }
        fn detect(&self) -> crate::agents::Detection {
            crate::agents::Detection {
                id: self.id.into(),
                display_name: self.display_name().into(),
                available: true,
                version: None,
                path: None,
                caps: self.caps(),
            }
        }
        fn sessions(&self) -> &dyn SessionProvider {
            self
        }
        fn caps(&self) -> crate::agents::Caps {
            crate::agents::Caps { delete: self.can_delete, ..Default::default() }
        }
        fn launch(&self, _spec: &crate::agents::LaunchSpec) -> String {
            String::new()
        }
    }

    fn fake(can_delete: bool) -> Vec<Box<dyn crate::agents::AgentBackend>> {
        vec![Box::new(FakeOwner { id: "fake", can_delete })]
    }

    /// The 🗑 is hidden for an engine that declares no delete, but hiding a
    /// button is not a boundary — `session_delete` is an IPC command anything
    /// can call. It refuses on the backend's own answer, before it has a path
    /// to rename, so the destructive step is never reached.
    #[test]
    fn delete_refuses_for_an_engine_that_does_not_claim_it() {
        let err = deletable(&fake(false), "owned").map(|_| ()).expect_err("must refuse");
        assert!(err.contains("Fake Engine"), "the refusal should name the engine: {err}");
        assert_eq!(
            deletable(&fake(true), "owned").ok().map(|(b, p)| (b.id(), p)),
            Some(("fake", std::path::PathBuf::from("/nonexistent/store.db"))),
            "an engine that claims the delete still gets its path",
        );
        assert_eq!(
            deletable(&fake(true), "someone-elses").map(|_| ()).unwrap_err(),
            "session not found",
        );
    }

    /* ---- preview from a backend-supplied conversation ------------------- */

    /// A backend that hands over its conversation gets the same shaping as one
    /// that hands over a file: the last `PREVIEW_KEEP` messages, each cut at
    /// `PREVIEW_MAX_CHARS` with an ellipsis. Anything else and an OpenCode
    /// preview would be a different thing from a claude one.
    #[test]
    fn a_supplied_conversation_is_shaped_like_a_transcript_one() {
        let long = "x".repeat(PREVIEW_MAX_CHARS + 50);
        let mut msgs: Vec<(String, String)> = (0..PREVIEW_KEEP + 5)
            .map(|i| ("user".to_string(), format!("turn {i}")))
            .collect();
        msgs.push(("assistant".into(), long));
        let out = preview_from_messages(msgs);
        assert_eq!(out.len(), PREVIEW_KEEP, "only the tail is kept");
        // 18 in (PREVIEW_KEEP + 5 turns, then the long one), 12 out.
        assert_eq!(out[0].text, "turn 6", "the oldest turns fall off the front");
        let last = out.last().unwrap();
        assert_eq!(last.text.chars().count(), PREVIEW_MAX_CHARS + 1);
        assert!(last.text.ends_with('…'));
        assert!(last.at.is_none(), "(role, text) carries no timestamp to invent one from");
    }

    /// The same two filters the file path applies: a message that is nothing
    /// but an injected system block, and a meta-prompt nobody typed.
    #[test]
    fn a_supplied_conversation_drops_what_nobody_said() {
        let out = preview_from_messages(vec![
            ("user".into(), "<system-reminder>be good</system-reminder>".into()),
            (
                "user".into(),
                "You are summarizing a Claude Code session and should…".into(),
            ),
            ("user".into(), "   ".into()),
            ("assistant".into(), "kept".into()),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "kept");
    }

    #[test]
    fn reads_a_claude_message_in_both_content_shapes() {
        assert_eq!(
            msg(r#"{"type":"user","message":{"content":"plain string"}}"#),
            Some(("user".into(), "plain string".into()))
        );
        assert_eq!(
            msg(
                r#"{"type":"assistant","message":{"content":[
                   {"type":"text","text":"first"},{"type":"text","text":"second"}]}}"#
            ),
            Some(("assistant".into(), "first\nsecond".into()))
        );
    }

    #[test]
    fn reads_a_codex_message() {
        // Shape from a real rollout, codex-cli 0.145.0.
        assert_eq!(
            msg(
                r#"{"type":"response_item","payload":{"type":"message","role":"user",
                   "content":[{"type":"input_text","text":"what does this do"}]}}"#
            ),
            Some(("user".into(), "what does this do".into()))
        );
        assert_eq!(
            msg(
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant",
                   "content":[{"type":"output_text","text":"it lists sessions"}]}}"#
            ),
            Some(("assistant".into(), "it lists sessions".into()))
        );
    }

    #[test]
    fn skips_what_is_not_conversation() {
        // Codex's `developer` role is the sandbox preamble, not something the
        // user said — indexing it would match every session on the same words.
        assert_eq!(
            msg(
                r#"{"type":"response_item","payload":{"type":"message","role":"developer",
                   "content":[{"type":"input_text","text":"<permissions instructions>"}]}}"#
            ),
            None
        );
        // A tool call carried on a response_item, not a message.
        assert_eq!(
            msg(r#"{"type":"response_item","payload":{"type":"function_call","name":"shell"}}"#),
            None
        );
        // Empty content is not a message worth showing or searching.
        assert_eq!(
            msg(r#"{"type":"user","message":{"content":[{"type":"text","text":"   "}]}}"#),
            None
        );
        // And the cheap prefilter must not waste a parse on the rest.
        assert!(!line_may_hold_message(r#"{"type":"event_msg","payload":{"type":"token_count"}}"#));
        assert!(!line_may_hold_message(r#"{"type":"world_state","payload":{"full":true}}"#));
    }

    #[test]
    fn boilerplate_is_kept_out_of_the_index_but_real_text_is_not() {
        // Codex opens every rollout with this. Indexed, one query matches
        // every Codex session there is.
        assert!(is_only_system_block(
            "<environment_context>cwd /home/m, sandbox on</environment_context>"
        ));
        // A block plus something you actually said is indexed whole.
        assert!(!is_only_system_block(
            "<environment_context>noise</environment_context> why is the build failing"
        ));
        assert!(!is_only_system_block("just a normal question"));
        // Angle brackets that are not a block — a generic in pasted code —
        // must not look like boilerplate.
        assert!(!is_only_system_block("fn f(v: Vec<String>) -> Option<u8>"));
        assert!(!is_only_system_block(""));
    }

    /// Codex's AGENTS.md preamble — an untagged `# AGENTS.md instructions
    /// for <cwd>` header ahead of an `<INSTRUCTIONS>` block, opening the
    /// first user message of every rollout in that repo — is boilerplate,
    /// not something anyone typed. Both halves are required: a genuine
    /// message that merely mentions AGENTS.md stays indexed and previewed.
    /// Shape as observed in real rollouts on this machine.
    /// [observed: codex-cli 0.150.1]
    #[test]
    fn the_codex_agents_preamble_is_boilerplate_not_a_message() {
        let preamble = "# AGENTS.md instructions for /home/john/nanoclaw\n\n\
            <INSTRUCTIONS>\n# Agent start — /home/john/nanoclaw\n\n\
            **This file is the rulebook.**\n</INSTRUCTIONS>";
        assert!(is_codex_agents_preamble(preamble));
        assert!(is_only_system_block(preamble), "the indexer consumes it through this predicate");
        // Header without the block, and the block without the header: kept.
        assert!(!is_codex_agents_preamble(
            "# AGENTS.md instructions for /x say, why does codex send this?"
        ));
        assert!(!is_codex_agents_preamble(
            "please add an <INSTRUCTIONS> block to AGENTS.md"
        ));
        // A stripped preamble is the bare header line — the reason the
        // check runs on RAW text, before tags come out.
        assert!(!is_codex_agents_preamble(&strip_system_tags(preamble)));
    }

    #[test]
    fn a_recorded_origin_sends_a_rollout_back_where_it_came_from() {
        // The case this whole sidecar exists for: deducing a destination from
        // the transcript can only ever produce a claude project directory, and
        // a Codex rollout does not live in one.
        assert_eq!(
            recorded_origin(
                "/home/m/.codex/sessions/2026/07/28/rollout-2026-07-28T20-43-28-abc.jsonl\n"
            ),
            Some(std::path::PathBuf::from(
                "/home/m/.codex/sessions/2026/07/28/rollout-2026-07-28T20-43-28-abc.jsonl"
            ))
        );
    }

    #[test]
    fn claude_still_restores_by_deduction_and_other_agents_by_record() {
        let home = std::path::Path::new("/home/m");
        // Claude: keep deducing, so a misfiled transcript is still repaired.
        assert!(is_claude_transcript(
            std::path::Path::new("/home/m/.claude/projects/-tmp/abc.jsonl"),
            home
        ));
        // Codex: nothing here knows how to derive that path, so use the record.
        assert!(!is_claude_transcript(
            std::path::Path::new("/home/m/.codex/sessions/2026/07/28/rollout-x-abc.jsonl"),
            home
        ));
        // Not fooled by a lookalike outside the projects tree.
        assert!(!is_claude_transcript(
            std::path::Path::new("/home/m/.claude/trash/abc.jsonl"),
            home
        ));
    }

    #[test]
    fn an_unusable_origin_falls_back_instead_of_restoring_somewhere_odd() {
        // Each of these would otherwise decide where a `rename` lands.
        assert_eq!(recorded_origin(""), None);
        assert_eq!(recorded_origin("   \n"), None);
        assert_eq!(recorded_origin("relative/path.jsonl"), None);
        assert_eq!(recorded_origin("/"), None);
    }

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

    /* ---- grok session_kind / fork lineage ------------------------------- */

    /// Field locations as grok 1.0.13 writes them, recorded off real
    /// summary.json bodies on 2026-08-31: `session_kind` and
    /// `parent_session_id` sit at the TOP level, beside `info` — not inside
    /// it. A subagent child hides; a headless run stays; a fork child (the
    /// live `--fork-session` specimen was headless AND forked) carries its
    /// parent. [observed: grok 1.0.13]
    #[test]
    fn grok_summary_kind_and_lineage_read_where_1_0_13_writes_them() {
        // Verbatim (trimmed) from a real subagent child under
        // ~/.grok/sessions/%2Fhome%2Fjohn%2Fnanoclaw/.
        let subagent = r#"{"info":{"id":"01a01a76-6574-7d03-b980-dbf13a21f3b4","cwd":"/home/john/nanoclaw"},"session_summary":"Version-control live aan-native theme (#130)","session_kind":"subagent","current_model_id":"grok-4.6","agent_name":"general-purpose"}"#;
        let m = grok_row_meta_from_summary(subagent);
        assert!(m.subagent, "session_kind:\"subagent\" must hide the row");
        assert_eq!(m.fork_parent, None);

        // Verbatim (trimmed) from the audit's live fork specimen — a headless
        // fork child. Headless must NOT hide; the parent must be read.
        let fork_child = r#"{"info":{"id":"3dfec75c-813a-4ef0-a55e-cf0e5d0a50d4","cwd":"/home/john/.claude/jobs/8f258164/tmp/grok-specimen"},"session_summary":"Create a file named hello.txt in the current directory containing","session_kind":"headless","parent_session_id":"e761294d-49af-4c36-8a22-481d77a05f39","current_model_id":"grok-4.6","agent_name":"grok-build-plan"}"#;
        let m = grok_row_meta_from_summary(fork_child);
        assert!(!m.subagent, "headless -p runs are the person's own sessions");
        assert_eq!(
            m.fork_parent.as_deref(),
            Some("e761294d-49af-4c36-8a22-481d77a05f39"),
        );

        // Interactive sessions carry neither field.
        let interactive = r#"{"info":{"id":"01a043d8-3c53-7900-a612-ef62d4c72245","cwd":"/home/john/nanoclaw"},"session_summary":"Some chat","current_model_id":"grok-4.6"}"#;
        let m = grok_row_meta_from_summary(interactive);
        assert!(!m.subagent);
        assert_eq!(m.fork_parent, None);
    }

    /// `grok_row_meta` reads summary.json beside the row's transcript path —
    /// the path grok's scan hands over points inside the session directory.
    #[test]
    fn grok_row_meta_reads_beside_the_transcript() {
        let dir = std::env::temp_dir().join("aiterm-test-grok-row-meta");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("summary.json"),
            r#"{"info":{"id":"x","cwd":"/tmp"},"session_summary":"t","session_kind":"subagent"}"#,
        )
        .unwrap();
        let chat = dir.join("chat_history.jsonl");
        std::fs::write(&chat, "").unwrap();
        assert!(grok_row_meta(&chat).subagent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /* ---- claude subagent artifact writes -------------------------------- */

    /// Subagent transcripts (`<project>/<sid>/subagents/agent-<id>.jsonl`,
    /// Claude Code 2.1.251) carry the same Write/Edit tool_use shapes as the
    /// main jsonl and merge into the same artifact set — with the newest write
    /// per path winning by timestamp, whichever file it came from.
    #[test]
    fn subagent_writes_merge_into_the_session_artifacts() {
        let dir = std::env::temp_dir().join("aiterm-test-subagent-artifacts");
        let _ = std::fs::remove_dir_all(&dir);
        let sid = "99999999-9999-4999-8999-999999999999";
        let sub = dir.join(sid).join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        let main = dir.join(format!("{sid}.jsonl"));
        // The main transcript wrote a.txt late; a subagent wrote a.txt early
        // and b.css (record shape as observed: isSidechain:true, the PARENT's
        // sessionId, ISO timestamps).
        std::fs::write(&main,
            format!(r#"{{"type":"assistant","sessionId":"{sid}","timestamp":"2026-08-27T16:00:00.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_1","name":"Edit","input":{{"file_path":"/w/a.txt"}}}}]}}}}"#)).unwrap();
        std::fs::write(sub.join("agent-a4f351af645be8f44.jsonl"),
            format!(r#"{{"type":"assistant","isSidechain":true,"agentId":"a4f351af645be8f44","sessionId":"{sid}","timestamp":"2026-08-27T15:27:46.767Z","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_2","name":"Write","input":{{"file_path":"/w/a.txt"}}}}]}}}}
{{"type":"assistant","isSidechain":true,"agentId":"a4f351af645be8f44","sessionId":"{sid}","timestamp":"2026-08-27T15:30:00.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_3","name":"Write","input":{{"file_path":"/w/b.css"}}}}]}}}}"#)).unwrap();

        let mut latest = std::collections::HashMap::new();
        scan_artifact_writes(&main, &mut latest);
        scan_artifact_writes(&sub.join("agent-a4f351af645be8f44.jsonl"), &mut latest);
        assert_eq!(
            latest.get("/w/a.txt"),
            Some(&("Edit".to_string(), "2026-08-27T16:00:00.000Z".to_string())),
            "the main transcript's later Edit must not be shadowed by the subagent's earlier Write",
        );
        assert_eq!(
            latest.get("/w/b.css"),
            Some(&("Write".to_string(), "2026-08-27T15:30:00.000Z".to_string())),
            "a file only a subagent wrote must reach the session's artifacts",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[tauri::command]
pub async fn list_sessions() -> Vec<Session> {
    crate::run_blocking(list_sessions_sync).await
}

fn list_sessions_sync() -> Vec<Session> {
    // Adding an agent means adding a backend in `agents.rs` and nothing here.
    //
    // A session has up to three names, resolved HERE and nowhere else so
    // every surface — sidebar, tab, phone, rename box — says the same
    // thing: the person's rename beats the librarian's label beats the
    // engine's own title.
    let titles = load_titles();
    let lib = crate::librarian::load_store();
    crate::agents::scan_all_with_paths()
        .into_iter()
        .filter_map(|(mut s, path)| {
            // Grok's row scan predates two summary.json fields its rows now
            // need (see grok_row_meta); patched here, at the one place every
            // surface's list is composed, so sidebar and phone agree.
            if s.agent == "grok" {
                let meta = grok_row_meta(&path);
                if meta.subagent {
                    return None;
                }
                if let Some(parent) = meta.fork_parent {
                    s.forked = true;
                    s.fork_parent = Some(parent);
                }
            }
            if let Some(t) = titles.get(&s.id) {
                s.title = t.clone();
            } else if let Some(e) = lib.sessions.get(&s.id) {
                if !e.name.trim().is_empty() {
                    s.title = e.name.clone();
                }
            }
            Some(s)
        })
        .collect()
}

/// What a grok row's `summary.json` says beyond what the row itself carries.
struct GrokRowMeta {
    /// `"session_kind": "subagent"` — a subagent's child session. grok 1.0.13
    /// gives subagent children full session directories in the normal sessions
    /// tree (the parent's `subagents/<id>/meta.json` points at them), so
    /// unfiltered they list as top-level sidebar rows. Only this kind hides a
    /// row: `"headless"` (`-p` runs) and absent (interactive) are sessions the
    /// person started and stay listed. [observed: grok 1.0.13]
    subagent: bool,
    /// `"parent_session_id"` — the source session of a fork or restore,
    /// stamped by grok 1.0.13 in the child's summary.json (verified off a live
    /// `--resume <id> --fork-session` child). [observed: grok 1.0.13]
    fork_parent: Option<String>,
}

/// Read [`GrokRowMeta`] for the row whose transcript is at `transcript` —
/// grok's row paths point inside the session directory, beside `summary.json`.
/// Both fields are top level, NOT under `info`. [observed: grok 1.0.13]
fn grok_row_meta(transcript: &Path) -> GrokRowMeta {
    transcript
        .parent()
        .and_then(|dir| std::fs::read_to_string(dir.join("summary.json")).ok())
        .map(|raw| grok_row_meta_from_summary(&raw))
        .unwrap_or(GrokRowMeta { subagent: false, fork_parent: None })
}

/// The parse behind [`grok_row_meta`], separated so the field locations can be
/// tested against recorded summary.json bodies without a session on disk.
fn grok_row_meta_from_summary(raw: &str) -> GrokRowMeta {
    let v: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return GrokRowMeta { subagent: false, fork_parent: None },
    };
    GrokRowMeta {
        subagent: v.get("session_kind").and_then(|k| k.as_str()) == Some("subagent"),
        fork_parent: v
            .get("parent_session_id")
            .and_then(|p| p.as_str())
            .filter(|p| !p.is_empty())
            .map(String::from),
    }
}

/// Person-chosen session titles, id → title, kept beside the config. Every
/// engine names sessions its own way (or not at all — "Quick check", "AI-OS");
/// a name the person typed wins over all of them, uniformly, without
/// touching any harness's own files.
fn titles_path() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join("aiterm").join("titles.json"))
}

fn load_titles() -> std::collections::HashMap<String, String> {
    titles_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Rename a session. An empty (or whitespace) title removes the override,
/// letting the engine's own name show again.
pub fn rename_session(session_id: &str, title: &str) -> Result<(), String> {
    let Some(p) = titles_path() else { return Err("no data dir".into()) };
    let mut m = load_titles();
    let t = title.trim();
    if t.is_empty() {
        m.remove(session_id);
    } else {
        m.insert(session_id.to_string(), t.to_string());
    }
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| e.to_string())
}

/// The override map itself, for the UI: a person-chosen name must outrank
/// every other naming layer (the librarian's labels included), and the row
/// can only rank what it can see.
#[tauri::command]
pub fn session_titles() -> std::collections::HashMap<String, String> {
    load_titles()
}

/// Starred sessions, id list, kept beside the config — the ones that stay
/// on top of every list, on every device.
fn stars_path() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join("aiterm").join("stars.json"))
}

pub fn load_stars() -> Vec<String> {
    stars_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn set_star(session_id: &str, on: bool) -> Result<(), String> {
    let Some(p) = stars_path() else { return Err("no data dir".into()) };
    let mut stars = load_stars();
    stars.retain(|s| s != session_id);
    if on {
        stars.insert(0, session_id.to_string());
    }
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&p, serde_json::to_string_pretty(&stars).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn session_stars() -> Vec<String> {
    load_stars()
}

/// Relay lineage: brought-in session → the master it was brought into.
/// Written when a relay reports; read so every surface can group a
/// workspace's agents under the one that owns the conversation.
fn brought_in_path() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join("aiterm").join("brought_in.json"))
}

pub fn load_brought_in() -> std::collections::HashMap<String, String> {
    brought_in_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub fn session_brought_in() -> std::collections::HashMap<String, String> {
    load_brought_in()
}

pub fn record_brought_in(b_session: &str, master: &str) {
    let Some(p) = brought_in_path() else { return };
    let mut m = load_brought_in();
    if m.get(b_session).map(String::as_str) == Some(master) {
        return;
    }
    m.insert(b_session.to_string(), master.to_string());
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&m) {
        let _ = std::fs::write(&p, text);
    }
}

#[tauri::command]
pub fn session_star(app: tauri::AppHandle, session_id: String, on: bool) -> Result<(), String> {
    set_star(&session_id, on)?;
    crate::remote::notify(&app, crate::remote::Event::SessionsChanged);
    use tauri::Emitter;
    let _ = app.emit("sessions://changed", ());
    Ok(())
}

#[tauri::command]
pub fn session_rename(app: tauri::AppHandle, session_id: String, title: String) -> Result<(), String> {
    rename_session(&session_id, &title)?;
    // Both UIs list the same sessions; tell them the list moved. (The
    // transcript watcher can't see titles.json, so say it ourselves.)
    crate::remote::notify(&app, crate::remote::Event::SessionsChanged);
    use tauri::Emitter;
    let _ = app.emit("sessions://changed", ());
    Ok(())
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

        let facts = read_head_facts(&child).unwrap();
        assert!(facts.is_bg, "child should be recognised as a daemon session");
        assert!(
            facts.links.contains("AAA"),
            "child should claim the parent's uuid"
        );
        assert!(
            file_has_any_uuid(&parent, &facts.links),
            "AAA lives in the parent"
        );

        let bfacts = read_head_facts(&branch).unwrap();
        assert!(!bfacts.is_bg, "a --fork-session branch is not a daemon session");
        assert!(
            file_has_any_uuid(&parent, &bfacts.links),
            "the branch does share ancestry — which is exactly why bg is required too"
        );

        let unrelated = write_jsonl(
            &tmp,
            "unrelated.jsonl",
            &[r#"{"type":"user","parentUuid":"ZZZ","uuid":"DDD","sessionKind":"bg"}"#],
        );
        let ufacts = read_head_facts(&unrelated).unwrap();
        assert!(
            !file_has_any_uuid(&parent, &ufacts.links),
            "an unrelated bg session must not resolve into this parent"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Deterministic mtimes. The `/clear` rule turns on which sibling was
    /// written first, and consecutive writes in a test can land in the same
    /// millisecond — which would make the ordering, and the test, a coin flip.
    fn touch_at(path: &Path, millis: u64) {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis);
        let f = File::options().write(true).open(path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap();
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("aiterm-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The head of a real `/clear` child, captured 2026-07-29: a hook
    /// attachment, the local-command caveat as an `isMeta` user record, then the
    /// command echo. Nothing here names the parent — that is the whole point.
    const CLEAR_CHILD: &[&str] = &[
        r#"{"type":"custom-title","customTitle":"work-pc","sessionId":"child"}"#,
        r#"{"parentUuid":null,"isSidechain":false,"attachment":{"type":"hook_success","hookName":"SessionStart:clear"}}"#,
        r#"{"type":"user","isMeta":true,"uuid":"m1","parentUuid":null,"message":{"role":"user","content":"<local-command-caveat>Caveat: …</local-command-caveat>"}}"#,
        r#"{"type":"user","uuid":"c1","parentUuid":"m1","message":{"role":"user","content":"<command-name>/clear</command-name>\n<command-args></command-args>"}}"#,
    ];

    #[test]
    fn a_transcript_says_for_itself_that_clear_made_it() {
        let tmp = temp_dir("clear-head");
        let child = write_jsonl(&tmp, "child.jsonl", CLEAR_CHILD);
        let facts = read_head_facts(&child).unwrap();
        assert!(
            facts.born_from_clear,
            "the command echo is the child's own account of why it exists"
        );
        assert!(!facts.is_bg, "/clear does not move anything to the daemon");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_session_that_opened_with_a_prompt_was_not_cleared() {
        let tmp = temp_dir("clear-negative");
        // An ordinary session. It later *mentions* the command, which must not
        // count: only the opening turn says how a transcript began.
        let plain = write_jsonl(&tmp, "plain.jsonl", &[
            r#"{"type":"user","uuid":"p1","message":{"role":"user","content":"fix the build"}}"#,
            r#"{"type":"assistant","uuid":"p2","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}"#,
            r#"{"type":"user","uuid":"p3","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
        ]);
        assert!(!read_head_facts(&plain).unwrap().born_from_clear);

        // And a resumed session, which opens on an assistant record.
        let resumed = write_jsonl(&tmp, "resumed.jsonl", &[
            r#"{"type":"assistant","uuid":"r1","message":{"role":"assistant","content":[{"type":"text","text":"back"}]}}"#,
            r#"{"type":"user","uuid":"r2","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
        ]);
        assert!(!read_head_facts(&resumed).unwrap().born_from_clear);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_frozen_transcript_hands_its_tab_to_the_clear_child() {
        let tmp = temp_dir("clear-pair");
        let parent = write_jsonl(&tmp, "parent.jsonl", &[
            r#"{"type":"user","uuid":"AAA","message":{"role":"user","content":"hello"}}"#,
        ]);
        let child = write_jsonl(&tmp, "child.jsonl", CLEAR_CHILD);
        touch_at(&parent, 1_000);
        touch_at(&child, 2_000);

        assert_eq!(
            moved_to_in_dir(&parent),
            Some(SessionMove {
                id: "child".into(),
                kind: MoveKind::Cleared
            }),
        );
        // Not symmetric: the child is the live one, and it has no successor.
        assert_eq!(moved_to_in_dir(&child), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_clear_child_is_only_claimed_by_the_session_that_spoke_last() {
        let tmp = temp_dir("clear-ambiguous");
        // Two idle sessions in one project. `older` stopped long before the
        // clear happened, so the child is not its business — something else was
        // written in between.
        let older = write_jsonl(&tmp, "older.jsonl", &[
            r#"{"type":"user","uuid":"OLD","message":{"role":"user","content":"a"}}"#,
        ]);
        let newer = write_jsonl(&tmp, "newer.jsonl", &[
            r#"{"type":"user","uuid":"NEW","message":{"role":"user","content":"b"}}"#,
        ]);
        let child = write_jsonl(&tmp, "child.jsonl", CLEAR_CHILD);
        touch_at(&older, 1_000);
        touch_at(&newer, 2_000);
        touch_at(&child, 3_000);

        assert_eq!(
            moved_to_in_dir(&newer).map(|m| m.id),
            Some("child".to_string()),
            "the session that spoke last is the one the terminal cleared"
        );
        assert_eq!(
            moved_to_in_dir(&older),
            None,
            "re-keying this tab would point it at another tab's conversation"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The daemon-migration half of `moved_to_in_dir`, hermetically. It used to
    /// be covered only by `finds_the_captured_specimen`, which is `#[ignore]`d
    /// and, since the parent transcript was deleted from this machine, no longer
    /// runnable at all — so the path had no live test.
    #[test]
    fn a_migrated_child_takes_the_tab() {
        let tmp = temp_dir("moved-bg");
        let parent = write_jsonl(&tmp, "parent.jsonl", &[
            r#"{"type":"user","uuid":"AAA","message":{"role":"user","content":"hello"}}"#,
        ]);
        let child = write_jsonl(&tmp, "child.jsonl", &[
            r#"{"type":"system","subtype":"compact_boundary","logicalParentUuid":"AAA","uuid":"BBB","sessionKind":"bg"}"#,
            r#"{"type":"user","uuid":"CCC","parentUuid":"BBB","sessionKind":"bg","message":{"role":"user","content":"carrying on"}}"#,
        ]);
        touch_at(&parent, 1_000);
        touch_at(&child, 2_000);
        assert_eq!(
            moved_to_in_dir(&parent),
            Some(SessionMove {
                id: "child".into(),
                kind: MoveKind::Background
            }),
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_fork_sibling_never_takes_a_tab() {
        let tmp = temp_dir("clear-fork");
        let parent = write_jsonl(&tmp, "parent.jsonl", &[
            r#"{"type":"user","uuid":"AAA","message":{"role":"user","content":"hello"}}"#,
        ]);
        // A `--fork-session` branch is newer and shares ancestry, but the parent
        // is still running and still resumable at its own point.
        let branch = write_jsonl(&tmp, "branch.jsonl", &[
            r#"{"type":"user","uuid":"BBB","parentUuid":"AAA","message":{"role":"user","content":"branch"}}"#,
        ]);
        touch_at(&parent, 1_000);
        touch_at(&branch, 2_000);
        assert_eq!(moved_to_in_dir(&parent), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Run with `cargo test -- --ignored` on a machine that still has the
    /// 2026-07-26 specimen. Non-hermetic by design: it checks the real files
    /// the rule was derived from, which no fixture can stand in for.
    ///
    /// Known dead as of 2026-07-29 on this machine: the parent transcript
    /// (`2eb3a23f-…`) has been deleted, so this now fails with `None` wherever
    /// the specimen is gone. Kept for machines that still hold it;
    /// `a_migrated_child_takes_the_tab` is the hermetic cover for the same rule.
    #[test]
    #[ignore]
    fn finds_the_captured_specimen() {
        let got = session_moved_to_sync("2eb3a23f-e4f1-4263-beb0-e3c7b768dcba".into());
        assert_eq!(
            got,
            Some(SessionMove {
                id: "6b37ca79-7e8f-4b86-9817-eaeb1b1fe95c".into(),
                kind: MoveKind::Background,
            })
        );
    }

    /// The `/clear` counterpart, from the pair captured 2026-07-29 in
    /// `~/.claude/projects/-home-matt-Projects-work-pc`. Same deal: run with
    /// `cargo test -- --ignored` on a machine that still has the specimen.
    #[test]
    #[ignore]
    fn finds_the_captured_clear_specimen() {
        let got = session_moved_to_sync("605b9dad-8aff-4422-ac85-e553739f3d2b".into());
        assert_eq!(
            got,
            Some(SessionMove {
                id: "7047782e-6b33-4757-b138-89551041d670".into(),
                kind: MoveKind::Cleared,
            })
        );
    }

    /// A registry dir with one interactive entry, one background entry, one
    /// dead-pid leftover, and one file of garbage. Only the two live entries
    /// come back, and `kind: "bg"` — the value the files actually use, not the
    /// `"background"` the CLI normalizes it to — sets the background flag.
    #[test]
    fn roster_reads_live_entries_from_registry_files() {
        let dir = std::env::temp_dir()
            .join(format!("aiterm-test-roster-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.join(format!("{me}.json")),
            format!(r#"{{"pid":{me},"sessionId":"aaaaaaaa-0000-0000-0000-000000000001","kind":"interactive"}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("999999998.json"),
            format!(r#"{{"pid":{me},"sessionId":"aaaaaaaa-0000-0000-0000-000000000002","kind":"bg"}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("999999999.json"),
            r#"{"pid":999999999,"sessionId":"aaaaaaaa-0000-0000-0000-000000000003","kind":"interactive"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("junk.json"), "not json at all").unwrap();

        let mut got = roster_from_dir(&dir).unwrap();
        got.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(got.len(), 2, "live entries only");
        assert_eq!(got[0].session_id, "aaaaaaaa-0000-0000-0000-000000000001");
        assert!(!got[0].background);
        assert_eq!(got[0].pid, Some(me));
        assert_eq!(got[1].session_id, "aaaaaaaa-0000-0000-0000-000000000002");
        assert!(got[1].background);
    }

    /// `procStart` is the stale-file detector: a pid alone can be reissued by
    /// the kernel to an unrelated process after a crash left the file behind.
    /// An entry whose procStart matches the live process is kept; one whose
    /// procStart names a different incarnation of the same pid is dropped.
    #[test]
    fn roster_rejects_an_entry_whose_procstart_does_not_match() {
        let dir = std::env::temp_dir()
            .join(format!("aiterm-test-roster-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        let real = proc_starttime(me).expect("own starttime readable");
        assert_ne!(real, "1", "the mismatch fixture must actually mismatch");
        std::fs::write(
            dir.join("a.json"),
            format!(r#"{{"pid":{me},"sessionId":"bbbbbbbb-0000-0000-0000-000000000001","kind":"interactive","procStart":"{real}"}}"#),
        )
        .unwrap();
        std::fs::write(
            dir.join("b.json"),
            format!(r#"{{"pid":{me},"sessionId":"bbbbbbbb-0000-0000-0000-000000000002","kind":"interactive","procStart":"1"}}"#),
        )
        .unwrap();

        let got = roster_from_dir(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].session_id, "bbbbbbbb-0000-0000-0000-000000000001");
    }

    /// No registry dir means "can't answer", not "no sessions" — the caller
    /// falls back to asking the CLI, which must not be confused with the very
    /// different reading "the dir is there and empty, nothing is running".
    #[test]
    fn roster_missing_dir_is_none_but_empty_dir_is_empty() {
        let missing = std::env::temp_dir()
            .join(format!("aiterm-test-roster-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        assert!(roster_from_dir(&missing).is_none());

        let empty = std::env::temp_dir()
            .join(format!("aiterm-test-roster-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(roster_from_dir(&empty).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
