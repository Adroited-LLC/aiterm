//! Transport-independent session operations.
//!
//! The desktop commands and authenticated gateway both call this service. The
//! rooted constructor exists so destructive behavior can be exercised against
//! an explicit fixture tree without consulting the process home directory.

use crate::sessions::{PreviewMsg, Session};
use std::collections::HashSet;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_SESSION_ID_BYTES: usize = 256;
const MAX_ROOTED_FILES: usize = 4096;
const MAX_DISCOVERY_DEPTH: usize = 16;

#[derive(Clone, Debug)]
pub struct SessionRoots {
    sessions: PathBuf,
    pinned_sessions: Option<Arc<std::fs::File>>,
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
            pinned_sessions: pin_directory(&sessions),
            sessions,
            trash,
            tasks,
            jobs,
            forks,
        }
    }

    fn sessions_path(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        if let Some(directory) = &self.pinned_sessions {
            use std::os::fd::AsRawFd;
            return PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        }
        self.sessions.clone()
    }
}

fn pin_directory(path: &Path) -> Option<Arc<std::fs::File>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .ok()
            .map(Arc::new)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[derive(Clone)]
pub struct SessionService {
    catalog: Arc<dyn SessionCatalog>,
}

trait SessionCatalog: Send + Sync {
    // Provider adapters supply storage-specific I/O only. SessionService owns
    // id validation, existence checks, discovery bounds, stable errors and
    // operation ordering for every transport and every catalog.
    fn list(&self) -> Result<Vec<Session>, SessionServiceError>;
    fn preview(&self, session_id: &str) -> Result<Vec<PreviewMsg>, SessionServiceError>;
    fn delete(&self, session_id: &str) -> Result<(), SessionServiceError>;
    fn fork(&self, session_id: &str) -> Result<String, SessionServiceError>;
    fn stop(&self, session_id: &str) -> Result<(), SessionServiceError>;
}

struct DesktopCatalog;
struct FilesystemCatalog(SessionRoots);

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
            catalog: Arc::new(DesktopCatalog),
        }
    }

    pub fn from_roots(roots: SessionRoots) -> Self {
        Self {
            catalog: Arc::new(FilesystemCatalog(roots)),
        }
    }

    pub fn list(&self) -> Result<Vec<Session>, SessionServiceError> {
        self.catalog.list()
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
        self.find(session_id)?;
        self.catalog.preview(session_id)
    }

    pub fn delete(&self, session_id: &str) -> Result<(), SessionServiceError> {
        validate_session_id(session_id)?;
        // Resolve through the same bounded catalog before any provider hook is
        // allowed to create trash or mutate storage. This is the common
        // unknown-delete no-side-effect boundary.
        self.find(session_id)?;
        self.catalog.delete(session_id)
    }

    pub fn fork(&self, session_id: &str) -> Result<String, SessionServiceError> {
        validate_session_id(session_id)?;
        self.find(session_id)?;
        self.catalog.fork(session_id)
    }

    pub fn stop(&self, session_id: &str) -> Result<(), SessionServiceError> {
        validate_session_id(session_id)?;
        self.catalog.stop(session_id)
    }
}

impl SessionCatalog for DesktopCatalog {
    fn list(&self) -> Result<Vec<Session>, SessionServiceError> {
        Ok(crate::agents::scan_all_with_paths()
            .into_iter()
            .map(|(session, _)| session)
            .collect())
    }

    fn preview(&self, session_id: &str) -> Result<Vec<PreviewMsg>, SessionServiceError> {
        Ok(crate::sessions::session_preview_service(session_id))
    }

    fn delete(&self, session_id: &str) -> Result<(), SessionServiceError> {
        crate::sessions::session_delete_service(session_id)
            .map_err(|message| SessionServiceError::new(delete_code(&message), message))
    }

    fn fork(&self, session_id: &str) -> Result<String, SessionServiceError> {
        crate::sessions::session_fork_service(session_id)
            .map_err(|message| SessionServiceError::new(fork_code(&message), message))
    }

    fn stop(&self, session_id: &str) -> Result<(), SessionServiceError> {
        crate::sessions::stop_session_service(session_id)
            .map_err(|message| SessionServiceError::new("session.stop_failed", message))
    }
}

impl SessionCatalog for FilesystemCatalog {
    fn list(&self) -> Result<Vec<Session>, SessionServiceError> {
        Ok(scan_rooted(&self.0))
    }

    fn preview(&self, session_id: &str) -> Result<Vec<PreviewMsg>, SessionServiceError> {
        let path = rooted_path(&self.0, session_id)?;
        let file = open_regular_nofollow(&path)
            .map_err(|_| SessionServiceError::new("session.not_found", "session not found"))?;
        Ok(crate::sessions::preview_open_file_service(file))
    }

    fn delete(&self, session_id: &str) -> Result<(), SessionServiceError> {
        delete_rooted(&self.0, session_id)
    }

    fn fork(&self, session_id: &str) -> Result<String, SessionServiceError> {
        fork_rooted(&self.0, session_id)
    }

    fn stop(&self, _session_id: &str) -> Result<(), SessionServiceError> {
        // A fixture catalog has no process roster. An absent process is an
        // idempotent success, matching the desktop roster operation.
        Ok(())
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
    let mut visited = HashSet::new();
    collect_jsonl(&roots.sessions_path(), 0, &mut visited, &mut files);
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

fn collect_jsonl(
    path: &Path,
    depth: usize,
    visited: &mut HashSet<(u64, u64)>,
    out: &mut Vec<PathBuf>,
) {
    if out.len() >= MAX_ROOTED_FILES || depth > MAX_DISCOVERY_DEPTH {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // `path` may be Linux's `/proc/self/fd/<pinned-root>` handle. The
        // entries below are still classified with no-follow `DirEntry`
        // metadata; following this one procfs link is what retains the pinned
        // directory inode after its original pathname is replaced.
        let Ok(metadata) = std::fs::metadata(path) else {
            return;
        };
        if !metadata.file_type().is_dir() || !visited.insert((metadata.dev(), metadata.ino())) {
            return;
        }
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
            collect_jsonl(&path, depth + 1, visited, out);
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
    let root_path = roots.sessions_path();
    let mut visited = HashSet::new();
    collect_jsonl(&root_path, 0, &mut visited, &mut files);
    let candidate = files
        .into_iter()
        .find(|path| {
            path.file_stem()
                .is_some_and(|stem| stem.to_string_lossy() == session_id)
        })
        .ok_or_else(|| SessionServiceError::new("session.not_found", "session not found"))?;
    let root = std::fs::canonicalize(&root_path)
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
    let job = rooted_job(&roots.jobs, session_id)
        .map_err(|error| SessionServiceError::new("session.delete_failed", error))?;
    let session_root = std::fs::canonicalize(roots.sessions_path())
        .map_err(|error| SessionServiceError::new("session.delete_failed", error.to_string()))?;
    crate::sessions::archive_rooted_session_sources(
        &session_root,
        &path,
        &roots.tasks,
        &roots.jobs,
        job.as_deref(),
        &roots.trash,
        session_id,
    )
    .map_err(|error| SessionServiceError::new("session.delete_failed", error))
}

fn rooted_job(jobs: &Path, session_id: &str) -> Result<Option<PathBuf>, String> {
    let entries = match std::fs::read_dir(jobs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let state = entry.path().join("state.json");
        let metadata = match std::fs::symlink_metadata(&state) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err("session job state is not a verified regular file".into());
        }
        let raw = std::fs::read_to_string(&state).map_err(|error| error.to_string())?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if value.get("sessionId").and_then(|id| id.as_str()) == Some(session_id) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

fn fork_rooted(roots: &SessionRoots, session_id: &str) -> Result<String, SessionServiceError> {
    let source = rooted_path(roots, session_id)?;
    let mut source_file = open_regular_nofollow(&source)
        .map_err(|error| SessionServiceError::new("session.fork_failed", error.to_string()))?;
    let mut text = String::new();
    use std::io::{Read, Write};
    source_file
        .read_to_string(&mut text)
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
    let mut destination_file = create_private_new_nofollow(&destination)
        .map_err(|error| SessionServiceError::new("session.fork_failed", error.to_string()))?;
    destination_file
        .write_all(rewritten.as_bytes())
        .map_err(|error| SessionServiceError::new("session.fork_failed", error.to_string()))?;
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

fn create_private_new_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no-follow session operations are unsupported",
        ))
    }
}

fn open_regular_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        if !file.metadata()?.file_type().is_file() {
            return Err(std::io::Error::other(
                "session location is not a regular file",
            ));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no-follow session operations are unsupported",
        ))
    }
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
