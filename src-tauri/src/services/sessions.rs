//! Transport-independent session operations.
//!
//! The desktop commands and authenticated gateway both call this service. The
//! rooted constructor exists so destructive behavior can be exercised against
//! an explicit fixture tree without consulting the process home directory.

use crate::sessions::{PreviewMsg, Session};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_SESSION_ID_BYTES: usize = 256;
const MAX_ROOTED_FILES: usize = 4096;

#[derive(Clone, Debug)]
pub struct SessionRoots {
    sessions: PathBuf,
    trash: PathBuf,
    tasks: PathBuf,
    jobs: PathBuf,
    forks: PathBuf,
}

impl SessionRoots {
    pub fn new(
        sessions: PathBuf,
        trash: PathBuf,
        tasks: PathBuf,
        jobs: PathBuf,
        forks: PathBuf,
    ) -> Self {
        Self {
            sessions,
            trash,
            tasks,
            jobs,
            forks,
        }
    }
}

#[derive(Clone)]
pub struct SessionService {
    source: Arc<SessionSource>,
}

enum SessionSource {
    Desktop,
    Rooted(SessionRoots),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionServiceError {
    code: &'static str,
    message: String,
}

impl SessionServiceError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for SessionServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SessionServiceError {}

impl Default for SessionService {
    fn default() -> Self {
        Self::desktop()
    }
}

impl SessionService {
    pub fn desktop() -> Self {
        Self {
            source: Arc::new(SessionSource::Desktop),
        }
    }

    pub fn from_roots(roots: SessionRoots) -> Self {
        Self {
            source: Arc::new(SessionSource::Rooted(roots)),
        }
    }

    pub fn list(&self) -> Result<Vec<Session>, SessionServiceError> {
        match &*self.source {
            SessionSource::Desktop => Ok(crate::agents::scan_all_with_paths()
                .into_iter()
                .map(|(session, _)| session)
                .collect()),
            SessionSource::Rooted(roots) => Ok(scan_rooted(roots)),
        }
    }

    pub fn find(&self, session_id: &str) -> Result<Session, SessionServiceError> {
        validate_session_id(session_id)?;
        self.list()?
            .into_iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| SessionServiceError::new("session.not_found", "session not found"))
    }

    pub fn validate_id(&self, session_id: &str) -> Result<(), SessionServiceError> {
        validate_session_id(session_id)
    }

    pub fn preview(&self, session_id: &str) -> Result<Vec<PreviewMsg>, SessionServiceError> {
        validate_session_id(session_id)?;
        match &*self.source {
            SessionSource::Desktop => Ok(crate::sessions::session_preview_service(session_id)),
            SessionSource::Rooted(roots) => {
                let path = rooted_path(roots, session_id)?;
                Ok(crate::sessions::preview_file_service(&path))
            }
        }
    }

    pub fn delete(&self, session_id: &str) -> Result<(), SessionServiceError> {
        validate_session_id(session_id)?;
        match &*self.source {
            SessionSource::Desktop => crate::sessions::session_delete_service(session_id)
                .map_err(|message| SessionServiceError::new(delete_code(&message), message)),
            SessionSource::Rooted(roots) => delete_rooted(roots, session_id),
        }
    }

    pub fn fork(&self, session_id: &str) -> Result<String, SessionServiceError> {
        validate_session_id(session_id)?;
        match &*self.source {
            SessionSource::Desktop => crate::sessions::session_fork_service(session_id)
                .map_err(|message| SessionServiceError::new(fork_code(&message), message)),
            SessionSource::Rooted(roots) => fork_rooted(roots, session_id),
        }
    }

    pub fn stop(&self, session_id: &str) -> Result<(), SessionServiceError> {
        validate_session_id(session_id)?;
        match &*self.source {
            SessionSource::Desktop => crate::sessions::stop_session_service(session_id)
                .map_err(|message| SessionServiceError::new("session.stop_failed", message)),
            // A rooted service has no process roster. A fixture transcript is
            // therefore already stopped, matching the desktop command's
            // idempotent "not in roster" behavior.
            SessionSource::Rooted(_) => Ok(()),
        }
    }
}

fn validate_session_id(session_id: &str) -> Result<(), SessionServiceError> {
    if session_id.is_empty()
        || session_id.len() > MAX_SESSION_ID_BYTES
        || session_id.contains('/')
        || session_id.contains("..")
        || session_id.contains('\0')
    {
        return Err(SessionServiceError::new(
            "session.invalid_id",
            "invalid session id",
        ));
    }
    Ok(())
}

fn scan_rooted(roots: &SessionRoots) -> Vec<Session> {
    let mut files = Vec::new();
    collect_jsonl(&roots.sessions, &mut files);
    files.sort();
    let mut sessions: Vec<_> = files
        .into_iter()
        .filter_map(|path| crate::sessions::parse_session_service(&path))
        .collect();
    let parents = read_forks(&roots.forks);
    for session in &mut sessions {
        if let Some(parent) = parents.get(&session.id) {
            session.fork_parent = Some(parent.clone());
            session.forked = true;
        }
    }
    sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    sessions
}

fn collect_jsonl(path: &Path, out: &mut Vec<PathBuf>) {
    if out.len() >= MAX_ROOTED_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_ROOTED_FILES {
            break;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_jsonl(&path, out);
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            && !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".orphaned-"))
        {
            out.push(path);
        }
    }
}

fn rooted_path(roots: &SessionRoots, session_id: &str) -> Result<PathBuf, SessionServiceError> {
    let mut files = Vec::new();
    collect_jsonl(&roots.sessions, &mut files);
    let candidate = files
        .into_iter()
        .find(|path| {
            path.file_stem()
                .is_some_and(|stem| stem.to_string_lossy() == session_id)
        })
        .ok_or_else(|| SessionServiceError::new("session.not_found", "session not found"))?;
    let root = std::fs::canonicalize(&roots.sessions)
        .map_err(|_| SessionServiceError::new("session.not_found", "session not found"))?;
    let candidate = std::fs::canonicalize(candidate)
        .map_err(|_| SessionServiceError::new("session.not_found", "session not found"))?;
    if !candidate.starts_with(&root) || !candidate.is_file() {
        return Err(SessionServiceError::new(
            "session.not_found",
            "session not found",
        ));
    }
    Ok(candidate)
}

fn delete_rooted(roots: &SessionRoots, session_id: &str) -> Result<(), SessionServiceError> {
    // Resolve before creating or purging anything. This ordering is the
    // destructive-operation boundary: an unknown id has no disk side effect.
    let path = rooted_path(roots, session_id)?;
    std::fs::create_dir_all(&roots.trash)
        .map_err(|error| SessionServiceError::new("session.delete_failed", error.to_string()))?;
    let destination = roots.trash.join(format!("{session_id}.jsonl"));
    std::fs::rename(&path, &destination)
        .map_err(|error| SessionServiceError::new("session.delete_failed", error.to_string()))?;
    let origin = roots.trash.join(format!("{session_id}.origin"));
    let _ = std::fs::write(origin, path.to_string_lossy().as_bytes());
    let tasks = roots.tasks.join(session_id);
    if tasks.is_dir() {
        let _ = std::fs::rename(tasks, roots.trash.join(format!("{session_id}.tasks")));
    }
    if let Some(job) = rooted_job(&roots.jobs, session_id) {
        let _ = std::fs::rename(job, roots.trash.join(format!("{session_id}.job")));
    }
    Ok(())
}

fn rooted_job(jobs: &Path, session_id: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(jobs).ok()?.flatten() {
        let Ok(raw) = std::fs::read_to_string(entry.path().join("state.json")) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if value.get("sessionId").and_then(|id| id.as_str()) == Some(session_id) {
            return Some(entry.path());
        }
    }
    None
}

fn fork_rooted(roots: &SessionRoots, session_id: &str) -> Result<String, SessionServiceError> {
    let source = rooted_path(roots, session_id)?;
    let text = std::fs::read_to_string(&source)
        .map_err(|error| SessionServiceError::new("session.fork_failed", error.to_string()))?;
    let new_id = uuid::Uuid::new_v4().to_string();
    let (rewritten, replacements) = rewrite_ids(&text, session_id, &new_id);
    if replacements == 0 {
        return Err(SessionServiceError::new(
            "session.fork_failed",
            "transcript has no session id fields to rewrite — not forking",
        ));
    }
    let destination = source.with_file_name(format!("{new_id}.jsonl"));
    std::fs::write(&destination, rewritten)
        .map_err(|error| SessionServiceError::new("session.fork_failed", error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600));
    }
    let mut forks = read_forks(&roots.forks);
    forks.insert(new_id.clone(), session_id.to_owned());
    if let Some(parent) = roots.forks.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(encoded) = serde_json::to_vec_pretty(&forks) {
        let _ = std::fs::write(&roots.forks, encoded);
    }
    Ok(new_id)
}

fn rewrite_ids(text: &str, old: &str, new: &str) -> (String, usize) {
    let mut rewritten = text.to_owned();
    let mut replacements = 0;
    for key in ["sessionId", "session_id"] {
        let from = format!("\"{key}\":\"{old}\"");
        let to = format!("\"{key}\":\"{new}\"");
        replacements += rewritten.matches(&from).count();
        rewritten = rewritten.replace(&from, &to);
    }
    (rewritten, replacements)
}

fn read_forks(path: &Path) -> std::collections::HashMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn delete_code(message: &str) -> &'static str {
    if message == "session not found" {
        "session.not_found"
    } else if message == "invalid session id" {
        "session.invalid_id"
    } else {
        "session.delete_failed"
    }
}

fn fork_code(message: &str) -> &'static str {
    if message.contains("no transcript") {
        "session.not_found"
    } else {
        "session.fork_failed"
    }
}
