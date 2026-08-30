//! What agents change on disk, seen at the filesystem.
//!
//! Every harness leaves files behind differently — Claude's tool calls are
//! in its transcript, Codex's image skill writes to a directory of its own,
//! a shell script says nothing at all. Parsing prompts and transcripts can
//! only ever cover the ones we know. The filesystem covers all of them: a
//! watcher over every open session's workspace (and each harness's own
//! output directory) records each create, modify and delete, and pins it
//! on the session that was active in that folder at that moment — which
//! the desktop already knows from its terminals.
//!
//! The ledger is kept in memory and appended to `changes.jsonl` beside the
//! config, so what a session produced last night is still listed today.
//! Both UIs read it: the desktop's agent panel and the phone's Files view.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Serialize, Deserialize)]
pub struct Change {
    pub path: String,
    pub name: String,
    /// "created" | "modified" | "deleted"
    pub kind: String,
    /// Unix seconds.
    pub at: u64,
    /// The session the change is pinned on. `None` = nobody was active in
    /// that folder; the person, probably.
    pub session_id: Option<String>,
    pub bytes: u64,
}

const KEEP: usize = 5000;
const SETTLE: Duration = Duration::from_millis(600);
/// A session counts as active in its folder for this long after its
/// terminal last showed work: a tool call finishes, the file lands a
/// moment later.
const RECENT: Duration = Duration::from_secs(20);

const SKIP: &[&str] = &[
    "/.git/", "/node_modules/", "/target/", "/.cache/", "/__pycache__/", "/.gradle/", "/.kotlin/",
    "/build/", "/dist/", "/.next/", "/.venv/", "/venv/", "/.mypy_cache/", "/.vite/",
];

struct Inner {
    watcher: Option<notify::RecommendedWatcher>,
    roots: HashSet<PathBuf>,
    /// session id → workspace root, for every bound tab.
    session_roots: HashMap<String, PathBuf>,
    /// When each session's terminal last reported work.
    recent: HashMap<String, Instant>,
    entries: Vec<Change>,
}

pub struct ChangeLedger {
    inner: Mutex<Inner>,
    tx: Mutex<Option<mpsc::Sender<notify::Result<notify::Event>>>>,
}

impl Default for ChangeLedger {
    fn default() -> Self {
        ChangeLedger {
            inner: Mutex::new(Inner {
                watcher: None,
                roots: HashSet::new(),
                session_roots: HashMap::new(),
                recent: HashMap::new(),
                entries: load(),
            }),
            tx: Mutex::new(None),
        }
    }
}

fn ledger_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("aiterm").join("changes.jsonl"))
}

fn load() -> Vec<Change> {
    let Some(p) = ledger_path() else { return vec![] };
    let Ok(text) = std::fs::read_to_string(p) else { return vec![] };
    let mut v: Vec<Change> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    if v.len() > KEEP {
        v.drain(..v.len() - KEEP);
    }
    v
}

fn append(c: &Change) {
    use std::io::Write;
    let Some(p) = ledger_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        if let Ok(line) = serde_json::to_string(c) {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Start the pump once, at setup. Harness output directories are watched
/// from the start; workspaces join as tabs bind to sessions.
pub fn start(app: &AppHandle) {
    let ledger = app.state::<ChangeLedger>();
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let sender = tx.clone();
    let seen = std::sync::atomic::AtomicU32::new(0);
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        // Reads are not changes, and a big tree is read constantly — by the
        // dev server, by git, by builds. Forward only what can matter, or the
        // pump never sees a quiet moment to settle a burst.
        let Ok(ev) = res else { return };
        if matches!(ev.kind, notify::EventKind::Access(_) | notify::EventKind::Other | notify::EventKind::Any) {
            return;
        }
        if ev.paths.iter().all(|p| {
            let s = p.to_string_lossy();
            SKIP.iter().any(|k| s.contains(k))
        }) {
            return;
        }
        let n = seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 {
            crate::diag!("changes", "event: {:?} {:?}", ev.kind, ev.paths.first());
        }
        if sender.send(Ok(ev)).is_err() {
            crate::diag!("changes", "callback: pump gone");
        }
    });
    match watcher {
        Ok(w) => {
            crate::diag!("changes", "watcher ready");
            ledger.inner.lock().unwrap().watcher = Some(w);
            *ledger.tx.lock().unwrap() = Some(tx);
        }
        Err(e) => {
            crate::diag!("changes", "no filesystem watcher: {e}");
            return;
        }
    }
    if let Some(home) = dirs::home_dir() {
        let root = home.join(".codex").join("generated_images");
        watch_root(app, root.clone());
        backfill_output_dir(app, &root);
    }
    let app = app.clone();
    std::thread::spawn(move || pump(app, rx));
}

/// Files that landed while no watcher was running. The app restarts often
/// (every dev rebuild) and a harness writes whenever it pleases, so a file
/// created in the gap would otherwise never exist as far as the ledger is
/// concerned — an image generated at 13:14 is invisible to a watcher started
/// at 13:52. A harness output directory names its session in the path, so
/// these are attributable after the fact; workspace files are not (nobody
/// remembers who was active) and stay watcher-only.
fn backfill_output_dir(app: &AppHandle, root: &Path) {
    let ledger = app.state::<ChangeLedger>();
    let known: HashSet<String> = ledger.inner.lock().unwrap().entries.iter().map(|e| e.path.clone()).collect();
    let mut found: Vec<Change> = Vec::new();
    let Ok(sessions) = std::fs::read_dir(root) else { return };
    for s in sessions.flatten() {
        let dir = s.path();
        if !dir.is_dir() {
            continue;
        }
        let sid = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        for f in files.flatten() {
            let Ok(md) = f.metadata() else { continue };
            if !md.is_file() {
                continue;
            }
            let path = f.path().to_string_lossy().into_owned();
            if known.contains(&path) {
                continue;
            }
            let at = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            let name = f.file_name().to_string_lossy().into_owned();
            found.push(Change { path, name, kind: "created".into(), at, session_id: sid.clone(), bytes: md.len() });
        }
    }
    if found.is_empty() {
        return;
    }
    found.sort_by_key(|c| c.at);
    crate::diag!("changes", "backfill: {} file(s) under {}", found.len(), root.display());
    let mut inner = ledger.inner.lock().unwrap();
    for c in found {
        append(&c);
        inner.entries.push(c);
    }
}

fn watch_root(app: &AppHandle, root: PathBuf) {
    if !root.is_dir() {
        return;
    }
    let ledger = app.state::<ChangeLedger>();
    let mut inner = ledger.inner.lock().unwrap();
    if inner.roots.contains(&root) {
        return;
    }
    if let Some(w) = inner.watcher.as_mut() {
        match w.watch(&root, RecursiveMode::Recursive) {
            Ok(()) => {
                crate::diag!("changes", "watching {}", root.display());
                inner.roots.insert(root);
            }
            Err(e) => crate::diag!("changes", "cannot watch {}: {e}", root.display()),
        }
    }
}

/// A tab now runs this session: remember its workspace and watch it.
pub fn track(app: &AppHandle, session_id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let sessions = crate::sessions::list_sessions().await;
        let Some(s) = sessions.into_iter().find(|s| s.id == session_id) else { return };
        let root = PathBuf::from(&s.project_path);
        app.state::<ChangeLedger>().inner.lock().unwrap().session_roots.insert(session_id, root.clone());
        watch_root(&app, root);
    });
}

/// A session's terminal just showed work. Called from the activity report.
pub fn touch(app: &AppHandle, session_id: &str) {
    if let Some(l) = app.try_state::<ChangeLedger>() {
        l.inner.lock().unwrap().recent.insert(session_id.to_string(), Instant::now());
    }
}

fn pump(app: AppHandle, rx: mpsc::Receiver<notify::Result<notify::Event>>) {
    loop {
        let first = match rx.recv() {
            Ok(v) => v,
            Err(_) => {
                crate::diag!("changes", "pump: channel closed");
                break;
            }
        };
        // Coalesce a burst: an editor writes a temp file, renames, touches.
        let mut pending: HashMap<PathBuf, &'static str> = HashMap::new();
        let mut note = |ev: notify::Result<notify::Event>| {
            let Ok(ev) = ev else { return };
            let kind = match ev.kind {
                notify::EventKind::Create(_) => "created",
                notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => "created",
                notify::EventKind::Modify(_) => "modified",
                notify::EventKind::Remove(_) => "deleted",
                _ => return,
            };
            for p in ev.paths {
                let s = p.to_string_lossy();
                if SKIP.iter().any(|k| s.contains(k)) || is_scratch(&p) {
                    continue;
                }
                let e = pending.entry(p).or_insert(kind);
                // created then modified is still created; anything then deleted is deleted.
                if kind == "deleted" || *e == "modified" {
                    *e = kind;
                }
            }
        };
        note(first);
        // Settle, but not forever: a tree that is never quiet still gets
        // its changes recorded, two seconds at a time.
        let deadline = Instant::now() + Duration::from_secs(2);
        while let Ok(ev) = rx.recv_timeout(SETTLE.min(deadline.saturating_duration_since(Instant::now()))) {
            note(ev);
            if Instant::now() >= deadline {
                break;
            }
        }
        crate::diag!("changes", "burst: {} path(s)", pending.len());
        for (path, kind) in pending {
            record(&app, path, kind);
        }
    }
}

/// Editors' droppings and lock files: not what anyone means by "a file".
fn is_scratch(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".tmp")
        || name.ends_with(".part")
        || name.starts_with(".#")
        || name == "4913"
        || name.ends_with(".lock")
}

fn record(app: &AppHandle, path: PathBuf, kind: &str) {
    let md = std::fs::metadata(&path).ok();
    if kind != "deleted" {
        match &md {
            Some(m) if m.is_file() => {}
            _ => return, // a directory, or gone again already
        }
    }
    let bytes = md.map(|m| m.len()).unwrap_or(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ledger = app.state::<ChangeLedger>();
    let sessions = {
        let inner = ledger.inner.lock().unwrap();
        attribute(&inner, &path, app)
    };
    let at = now();
    let owners: Vec<Option<String>> = if sessions.is_empty() { vec![None] } else { sessions.into_iter().map(Some).collect() };
    for session_id in owners {
        let c = Change { path: path.to_string_lossy().into_owned(), name: name.clone(), kind: kind.into(), at, session_id: session_id.clone(), bytes };
        append(&c);
        {
            let mut inner = ledger.inner.lock().unwrap();
            inner.entries.push(c.clone());
            if inner.entries.len() > KEEP + 500 {
                let n = inner.entries.len() - KEEP;
                inner.entries.drain(..n);
            }
        }
        let _ = app.emit("changes://file", &c);
        if let Some(sid) = session_id {
            crate::remote::notify(app, crate::remote::Event::FileChanged { session_id: sid, path: c.path.clone(), kind: kind.into() });
        }
    }
}

/// Which session(s) a change belongs to. A harness output directory names
/// its session in the path. Otherwise: sessions whose workspace holds the
/// file and whose terminal showed work in the last while; several at once
/// share the credit, none at all means nobody's agent did it.
fn attribute(inner: &Inner, path: &Path, app: &AppHandle) -> Vec<String> {
    let s = path.to_string_lossy();
    if let Some(rest) = s.split("/.codex/generated_images/").nth(1) {
        if let Some(sid) = rest.split('/').next() {
            return vec![sid.to_string()];
        }
    }
    // Only a session that is working, or was a moment ago, gets the credit.
    // An idle tab in the folder does not — the person editing in another
    // window, or another tool entirely, is not the agent's doing.
    let active: HashSet<String> = app
        .state::<crate::pty::PtyManager>()
        .activities()
        .into_iter()
        .filter(|(_, a)| a != "idle")
        .map(|(id, _)| id)
        .collect();
    inner
        .session_roots
        .iter()
        .filter(|(_, root)| path.starts_with(root))
        .map(|(id, _)| id)
        .filter(|id| active.contains(*id) || inner.recent.get(*id).is_some_and(|t| t.elapsed() < RECENT))
        .cloned()
        .collect()
}

/// A session's changes, newest first, one row per path (its latest state).
pub fn for_session(app: &AppHandle, session_id: &str) -> Vec<Change> {
    let ledger = app.state::<ChangeLedger>();
    let inner = ledger.inner.lock().unwrap();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in inner.entries.iter().rev() {
        if c.session_id.as_deref() != Some(session_id) {
            continue;
        }
        if seen.insert(c.path.clone()) {
            out.push(c.clone());
        }
    }
    // Backfilled entries append out of order; time, not file position, is
    // what "newest first" means.
    out.sort_by(|a, b| b.at.cmp(&a.at));
    out
}

#[tauri::command]
pub fn session_changes(app: AppHandle, session_id: String) -> Vec<Change> {
    for_session(&app, &session_id)
}

#[derive(Serialize)]
pub struct FilePreview {
    pub mime: String,
    /// Base64 of the bytes.
    pub data: String,
}

const PREVIEW_LIMIT: u64 = 12 * 1024 * 1024;

/// An image (or any small file) as base64 for the renderer's preview — the
/// asset protocol is not enabled here, and this is one read, on demand.
#[tauri::command]
pub async fn read_file_base64(path: String) -> Result<FilePreview, String> {
    crate::run_blocking(move || {
        let md = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if md.len() > PREVIEW_LIMIT {
            return Err("too large to preview".into());
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let ext = Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let mime = match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "mp4" | "m4v" => "video/mp4",
            "webm" => "video/webm",
            _ => "application/octet-stream",
        };
        Ok(FilePreview { mime: mime.into(), data: base64_encode(&bytes) })
    })
    .await
}

fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn scratch_files_are_not_changes() {
        assert!(is_scratch(Path::new("/x/a.rs~")));
        assert!(is_scratch(Path::new("/x/.a.rs.swp")));
        assert!(is_scratch(Path::new("/x/4913")));
        assert!(!is_scratch(Path::new("/x/a.rs")));
    }
}
