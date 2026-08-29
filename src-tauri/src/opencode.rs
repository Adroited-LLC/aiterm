//! OpenCode's session store, which is a SQLite database rather than a
//! directory of transcripts.
//!
//! Every other engine aiterm reads keeps one file per conversation, and the
//! rest of the app is shaped around that: a session *is* a path, previews and
//! the search indexer open it and read lines. OpenCode keeps everything in
//! `~/.local/share/opencode/opencode.db` — three tables, `session`, `message`
//! and `part` — so there is no per-session file to hand anyone. That is why
//! [`crate::sessions::SessionProvider::messages`] exists: this backend answers
//! with the conversation itself instead of a path to it.
//!
//! ## Why `sqlite3` and not a crate
//!
//! The database is read by shelling out to the `sqlite3` binary in JSON mode,
//! the same way `providers.rs` and `usage.rs` shell out to `curl` rather than
//! linking an HTTP stack. Every SQLite crate is a C build, and this is one
//! small reader for one optional engine — not worth a native dependency in
//! every build of the app. `sqlite3` missing, or the database missing, is the
//! ordinary case (most machines have no OpenCode), so both mean "no sessions",
//! never an error and never a panic — exactly how `CodexSessions` behaves when
//! `~/.codex/sessions` is not there.
//!
//! Every query is read-only except one: [`delete_to_trash`], the single write
//! this module makes, which removes one session's rows after dumping them to
//! `~/.claude/trash`. Reads keep `-readonly` so nothing else can ever grow a
//! write by accident.
//!
//! Read-only is `-readonly`, and deliberately *not* `immutable=1`: OpenCode
//! runs in WAL mode, and `immutable` tells SQLite to ignore the write-ahead log
//! — which would hide every message written since the last checkpoint, i.e.
//! exactly the conversation you just had.
//!
//! ## Why session ids are validated rather than bound
//!
//! Session ids reach [`messages`] and [`has_session`] from the frontend, so
//! they cannot be trusted into SQL text. The `sqlite3` CLI *does* have
//! parameters, but `.parameter set NAME VALUE` evaluates VALUE as a SQL
//! expression — binding a string through it still means producing a quoted SQL
//! literal ourselves, which is the interpolation we were trying to avoid, with
//! an extra dot-command in between. So the id is checked against
//! `^ses_[A-Za-z0-9]+$` first (see [`valid_id`]). That alphabet contains no
//! quote, no backslash, no semicolon and no whitespace, so a validated id
//! cannot leave the string literal it is written into; anything else never
//! reaches the database at all.

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::Duration;

use crate::sessions::{Session, SessionProvider};

/// How long a query may take before we give up on it. Generous for a local
/// SQLite read, and bounded because a database locked by something else must
/// not be able to hang a sidebar refresh.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// OpenCode's database, if it is on this machine.
pub fn db_path() -> Option<PathBuf> {
    let p = dirs::home_dir()?.join(".local/share/opencode/opencode.db");
    p.is_file().then_some(p)
}

/// An OpenCode session id: `ses_` followed by its base62 suffix.
///
/// The gate in front of every query — see the module doc for why validation is
/// the mechanism rather than parameter binding. It doubles as a cheap "not
/// ours": a claude uuid or an aiterm chat id fails here, so an ownership lookup
/// for another engine's session never spawns `sqlite3` at all.
pub(crate) fn valid_id(id: &str) -> bool {
    id.len() > 4 && id.starts_with("ses_") && id[4..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// Run one read-only query and return sqlite3's JSON output.
///
/// `None` covers every way this can decline — no `sqlite3`, no database, a
/// timeout, a non-zero exit — because they are all the same thing to a caller
/// listing sessions: OpenCode has nothing to show.
fn query(sql: &str) -> Option<String> {
    let bin = crate::agents::which("sqlite3")?;
    let db = db_path()?;
    let out = crate::agents::run_bounded(
        &bin.to_string_lossy(),
        &["-json", "-readonly", &db.to_string_lossy(), sql],
        QUERY_TIMEOUT,
    )?;
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// One row of the session query, spelled the way sqlite3's `-json` mode emits
/// it. `title` and `directory` are `NOT NULL` in the schema; they are optional
/// here so a future column change costs a field rather than the whole list.
#[derive(serde::Deserialize)]
struct SessionRow {
    id: String,
    title: Option<String>,
    directory: Option<String>,
    time_updated: Option<u64>,
    parent_id: Option<String>,
    time_archived: Option<i64>,
}

/// The sessions a person means when they say "my OpenCode sessions".
///
/// The `WHERE` clause is repeated in [`parse_sessions`] rather than trusted
/// once: the filter is the whole point of this query and a parser that only
/// works because of its caller's SQL is a parser that cannot be tested.
const SESSIONS_SQL: &str = "select id, title, directory, time_updated, parent_id, time_archived \
     from session where parent_id is null and time_archived is null \
     order by time_updated desc";

/// Sidebar rows for every top-level OpenCode session, newest first.
pub fn sessions() -> Vec<Session> {
    query(SESSIONS_SQL)
        .map(|json| parse_sessions(&json))
        .unwrap_or_default()
}

fn sessions_bounded(limit: usize) -> Vec<Session> {
    if limit == 0 {
        return Vec::new();
    }
    let sql = format!("{SESSIONS_SQL} limit {limit}");
    query(&sql)
        .map(|json| parse_sessions(&json))
        .unwrap_or_default()
}

/// sqlite3's JSON for [`SESSIONS_SQL`] → session rows.
///
/// Child sessions are dropped because OpenCode mints one per subagent run:
/// they are steps inside a conversation, not conversations, and listing them
/// would bury the sessions someone actually had. Archived ones are dropped
/// because that is what archiving means.
pub(crate) fn parse_sessions(json: &str) -> Vec<Session> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    let Ok(rows) = serde_json::from_str::<Vec<SessionRow>>(trimmed) else {
        return vec![];
    };
    rows.into_iter()
        .filter(|r| r.parent_id.is_none() && r.time_archived.is_none())
        .map(|r| {
            let cwd = r.directory.unwrap_or_default();
            Session {
                id: r.id,
                agent: "opencode".into(),
                title: r.title.unwrap_or_default(),
                project_path: cwd.clone(),
                // No worktree concept to regroup around, so a row groups under
                // the directory it ran in — the same thing `scan_chats` does.
                group_path: cwd,
                branch: None,
                forked: false,
                background: false,
                fork_parent: None,
                // Already unix millis in the database, which is the unit the
                // rest of `Session` uses.
                last_active: r.time_updated.unwrap_or(0),
            }
        })
        .collect()
}

/// One text part with the message it belongs to.
#[derive(serde::Deserialize)]
struct PartRow {
    message_id: String,
    message_data: String,
    part_data: String,
}

/// The conversation for one session, as `(role, text)` in the order it
/// happened.
///
/// One join rather than a query per message: a long session has hundreds of
/// messages and the indexer walks every session it has not seen.
///
/// `json_extract` filters to text parts in the database rather than in Rust so
/// tool outputs — which are the bulk of the table and can be megabytes — never
/// cross the pipe. [`parse_messages`] skips non-text parts too; that is the
/// tested behaviour, and this is the cheap version of it.
pub fn messages(session_id: &str) -> Vec<(String, String)> {
    if !valid_id(session_id) {
        return vec![];
    }
    let sql = format!(
        "select p.message_id as message_id, m.data as message_data, p.data as part_data \
         from part p join message m on m.id = p.message_id \
         where p.session_id = '{session_id}' \
           and json_extract(p.data, '$.type') = 'text' \
         order by m.time_created, p.time_created"
    );
    query(&sql)
        .map(|json| parse_messages(&json))
        .unwrap_or_default()
}

/// sqlite3's JSON for the join above → `(role, text)` per message.
///
/// A message's text parts are joined into one turn: OpenCode splits an
/// assistant reply across parts whenever a step boundary falls inside it, and
/// two half-sentences in the preview would be an artifact of its storage rather
/// than anything that happened.
pub(crate) fn parse_messages(json: &str) -> Vec<(String, String)> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    let Ok(rows) = serde_json::from_str::<Vec<PartRow>>(trimmed) else {
        return vec![];
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<String> = None;
    for r in rows {
        let Some(text) = part_text(&r.part_data) else {
            continue;
        };
        let Some(role) = message_role(&r.message_data) else {
            continue;
        };
        match current.as_deref() {
            Some(id) if id == r.message_id => {
                if let Some(last) = out.last_mut() {
                    last.1.push('\n');
                    last.1.push_str(&text);
                    continue;
                }
                out.push((role, text));
            }
            _ => {
                current = Some(r.message_id);
                out.push((role, text));
            }
        }
    }
    out.retain(|(_, text)| !text.trim().is_empty());
    out
}

/// The role on a `message.data` blob, when it is one of the two a conversation
/// is made of. OpenCode writes only `user` and `assistant` today; anything else
/// is not a turn and is dropped rather than guessed at.
fn message_role(data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    match v.get("role").and_then(|r| r.as_str()) {
        Some(r @ ("user" | "assistant")) => Some(r.to_string()),
        _ => None,
    }
}

/// The prose on a `part.data` blob. Only `text` parts carry any: `step-start`,
/// `step-finish`, `patch`, `reasoning` and `tool` are machinery, and indexing
/// them would make a search for a filename match every session that ever
/// touched it.
fn part_text(data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("text") {
        return None;
    }
    v.get("text").and_then(|t| t.as_str()).map(String::from)
}

/// What one session was running on: `(providerID, modelID)` from its newest
/// assistant message — `("openrouter", "z-ai/glm-5.2")`.
///
/// This is what makes resume honest about the model. `opencode --session <id>`
/// alone reopens the conversation on OpenCode's *default* model, not the
/// session's — measured 2026-08-10, when a GLM session came back as
/// Llama-4-Scout — so the resume command has to say the model, and the only
/// place that knows it is OpenCode's own record of the last reply.
///
/// The newest assistant message rather than the first: a session whose model
/// was deliberately switched mid-way should resume on the model it ended on.
pub fn session_model(session_id: &str) -> Option<(String, String)> {
    if !valid_id(session_id) {
        return None;
    }
    let sql = format!(
        "select json_extract(data, '$.providerID') as provider, \
                json_extract(data, '$.modelID') as model \
         from message \
         where session_id = '{session_id}' \
           and json_extract(data, '$.role') = 'assistant' \
         order by time_created desc limit 1"
    );
    query(&sql).and_then(|json| parse_session_model(&json))
}

/// sqlite3's JSON for the query above → the pair, when both halves are there.
/// A row with either missing is no answer at all: a bare model id could not be
/// spelled into OpenCode's `provider/model` argument anyway.
pub(crate) fn parse_session_model(json: &str) -> Option<(String, String)> {
    #[derive(serde::Deserialize)]
    struct Row {
        provider: Option<String>,
        model: Option<String>,
    }
    let rows: Vec<Row> = serde_json::from_str(json.trim()).ok()?;
    let r = rows.into_iter().next()?;
    match (r.provider, r.model) {
        (Some(p), Some(m)) if !p.is_empty() && !m.is_empty() => Some((p, m)),
        _ => None,
    }
}

/// Does OpenCode know this id?
///
/// Every session, not only the listed ones: this answers "is this row ours",
/// which is how the registry routes preview, resume and delete. A child or
/// archived session is still OpenCode's even though it does not appear in the
/// sidebar, and claiming otherwise would send it to whichever backend guessed
/// next.
pub fn has_session(session_id: &str) -> bool {
    if !valid_id(session_id) {
        return false;
    }
    let sql = format!("select id from session where id = '{session_id}' limit 1");
    query(&sql).is_some_and(|json| !json.trim().is_empty())
}

/// The first line of a trash dump: enough to list the entry by name, and to
/// recognize the file for what it is.
const DUMP_KIND: &str = "opencode-session-dump";

/// The id set a delete covers: the session and every descendant, spelled as a
/// subquery. OpenCode mints a child session per subagent run (`parent_id`),
/// and the schema's `ON DELETE CASCADE` does not fire — the CLI leaves
/// `foreign_keys` off — so leaving children behind would orphan rows OpenCode
/// still counts.
fn tree_sql(session_id: &str) -> String {
    format!(
        "with recursive tree(id) as (\
           select id from session where id = '{session_id}' \
           union \
           select s.id from session s join tree on s.parent_id = tree.id\
         ) select id from tree"
    )
}

/// Delete one session — dump first, rows after.
///
/// The dump is what lets this share the trash's promise: everything deleted is
/// readable in `~/.claude/trash/<id>.jsonl` for the keep window. It is a JSON
/// dump of the rows, not a transcript — readable, but not restorable by a
/// rename, which is why `trash_restore` refuses these ids honestly instead of
/// guessing.
///
/// The delete removes exactly the session rows the dump captured, listed by id
/// — never re-derived, so the two can only disagree by rows written between
/// the two steps, and the sidebar only offers this on stopped sessions.
/// A failed delete removes the dump again: a trash entry for a session that
/// still exists would be a lie in both directions.
pub fn delete_to_trash(session_id: &str, trash: &std::path::Path) -> Result<(), String> {
    if !valid_id(session_id) {
        return Err("invalid session id".into());
    }
    #[cfg(unix)]
    {
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(trash)
            .map_err(|e| e.to_string())?;
        delete_to_trash_at(session_id, &directory)
    }
    #[cfg(not(unix))]
    {
        let _ = (session_id, trash);
        Err("verified OpenCode dump operations are unsupported on this platform".into())
    }
}

#[cfg(unix)]
pub(crate) fn delete_to_trash_at(session_id: &str, trash: &std::fs::File) -> Result<(), String> {
    if !valid_id(session_id) {
        return Err("invalid session id".into());
    }
    #[cfg(target_os = "linux")]
    {
        let database = db_path().ok_or("OpenCode's database is not on this machine")?;
        delete_to_trash_from_path_with_hooks(
            session_id,
            &database,
            trash,
            || {},
            || {},
            || Ok(()),
            || Ok(()),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = trash;
        Err("verified OpenCode database operations require Linux openat support".into())
    }
}

/// A database object and its parent directory, both opened without following
/// symlinks. Keeping both descriptors alive makes the destructive target an
/// inode, not a pathname that can be redirected between dump and delete.
#[cfg(target_os = "linux")]
struct PinnedDatabase {
    parent: std::fs::File,
    object: std::fs::File,
    leaf: CString,
    device: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
impl PinnedDatabase {
    fn open(path: &std::path::Path) -> Result<Self, String> {
        use std::path::Component;

        if !path.is_absolute() {
            return Err("OpenCode database path must be absolute".into());
        }
        let parent_path = path.parent().ok_or("OpenCode database has no parent")?;
        let leaf_os = path
            .file_name()
            .ok_or("OpenCode database has no file name")?;
        let leaf = CString::new(leaf_os.as_bytes()).map_err(|_| "invalid database file name")?;
        let mut parent = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/")
            .map_err(|error| format!("could not pin filesystem root: {error}"))?;
        for component in parent_path.components() {
            let Component::Normal(name) = component else {
                if matches!(component, Component::RootDir) {
                    continue;
                }
                return Err("OpenCode database path contains an unsafe component".into());
            };
            let name = CString::new(name.as_bytes()).map_err(|_| "invalid database directory")?;
            let descriptor = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                return Err(format!(
                    "could not pin OpenCode database directory: {}",
                    std::io::Error::last_os_error()
                ));
            }
            parent = unsafe { std::fs::File::from_raw_fd(descriptor) };
        }

        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                leaf.as_ptr(),
                libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(format!(
                "could not pin OpenCode database: {}",
                std::io::Error::last_os_error()
            ));
        }
        let object = unsafe { std::fs::File::from_raw_fd(descriptor) };
        let metadata = object.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("OpenCode database is not a regular file".into());
        }
        Ok(Self {
            parent,
            object,
            leaf,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn named_identity(&self) -> Result<(u64, u64), String> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                self.parent.as_raw_fd(),
                self.leaf.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err("OpenCode database identity changed".into());
        }
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err("OpenCode database identity changed".into());
        }
        Ok((stat.st_dev as u64, stat.st_ino as u64))
    }

    fn connect(
        &self,
        before_open: impl FnOnce(),
        after_open: impl FnOnce(),
    ) -> Result<rusqlite::Connection, String> {
        // `/proc/self/fd/<parent>/<leaf>` walks from the directory descriptor
        // pinned above. If procfs is unavailable we fail closed; reopening the
        // original pathname would reintroduce the root-replacement race.
        let proc_parent = PathBuf::from(format!("/proc/self/fd/{}", self.parent.as_raw_fd()));
        let proc_metadata = std::fs::metadata(&proc_parent)
            .map_err(|_| "pinned OpenCode operations require Linux procfs".to_string())?;
        let parent_metadata = self.parent.metadata().map_err(|error| error.to_string())?;
        if proc_metadata.dev() != parent_metadata.dev()
            || proc_metadata.ino() != parent_metadata.ino()
        {
            return Err("pinned OpenCode directory identity changed".into());
        }
        if self.named_identity()? != (self.device, self.inode) {
            return Err("OpenCode database identity changed".into());
        }
        before_open();
        let connection = rusqlite::Connection::open_with_flags(
            proc_parent.join(std::ffi::OsStr::from_bytes(self.leaf.as_bytes())),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|error| format!("could not open pinned OpenCode database: {error}"))?;
        after_open();
        let identity = self.named_identity()?;
        if identity != (self.device, self.inode) {
            return Err("OpenCode database identity changed".into());
        }
        // Keep the object descriptor observably live through connection setup.
        let pinned = self.object.metadata().map_err(|error| error.to_string())?;
        if (pinned.dev(), pinned.ino()) != (self.device, self.inode) {
            return Err("OpenCode database identity changed".into());
        }
        Ok(connection)
    }
}

#[cfg(target_os = "linux")]
fn transaction_rows(
    transaction: &rusqlite::Transaction<'_>,
    sql: &str,
) -> Result<Vec<serde_json::Value>, String> {
    use base64::Engine as _;
    use rusqlite::types::ValueRef;

    let mut statement = transaction
        .prepare(sql)
        .map_err(|error| error.to_string())?;
    let names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut query = statement.query([]).map_err(|error| error.to_string())?;
    let mut output = Vec::new();
    while let Some(row) = query.next().map_err(|error| error.to_string())? {
        let mut object = serde_json::Map::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            let value = match row.get_ref(index).map_err(|error| error.to_string())? {
                ValueRef::Null => serde_json::Value::Null,
                ValueRef::Integer(value) => serde_json::Value::from(value),
                ValueRef::Real(value) => serde_json::Value::from(value),
                ValueRef::Text(value) => {
                    serde_json::Value::String(String::from_utf8_lossy(value).into_owned())
                }
                ValueRef::Blob(value) => serde_json::Value::String(
                    base64::engine::general_purpose::STANDARD.encode(value),
                ),
            };
            object.insert(name.clone(), value);
        }
        output.push(serde_json::Value::Object(object));
    }
    Ok(output)
}

#[cfg(target_os = "linux")]
fn unlink_named(directory: &std::fs::File, name: &CString) {
    unsafe {
        libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0);
    }
}

#[cfg(target_os = "linux")]
fn rename_noreplace(
    from_directory: &std::fs::File,
    from: &CString,
    to_directory: &std::fs::File,
    to: &CString,
) -> std::io::Result<()> {
    let result = unsafe {
        libc::renameat2(
            from_directory.as_raw_fd(),
            from.as_ptr(),
            to_directory.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn write_dump(
    file: std::fs::File,
    header: &serde_json::Value,
    sessions: &[serde_json::Value],
    messages: &[serde_json::Value],
    parts: &[serde_json::Value],
) -> std::io::Result<()> {
    use std::io::Write;

    let mut output = std::io::BufWriter::new(file);
    writeln!(output, "{header}")?;
    for (table, rows) in [
        ("session", sessions),
        ("message", messages),
        ("part", parts),
    ] {
        for row in rows {
            writeln!(
                output,
                "{}",
                serde_json::json!({ "table": table, "row": row })
            )?;
        }
    }
    output.into_inner()?.sync_all()
}

/// Dump and delete through one pinned database connection. The only pathname
/// used by SQLite is rooted at the pinned parent descriptor, and the leaf inode
/// is rechecked after the connection opens. The dump is fsynced and published
/// before any DELETE runs; consequently a crash can leave an extra dump, but
/// can never commit deleted rows without a durable dump.
#[cfg(target_os = "linux")]
fn delete_to_trash_from_path_with_hooks<AfterPin, DumpGate, SqlGate>(
    session_id: &str,
    database_path: &std::path::Path,
    trash: &std::fs::File,
    after_pin: AfterPin,
    after_connection_open: impl FnOnce(),
    dump_gate: DumpGate,
    sql_gate: SqlGate,
) -> Result<(), String>
where
    AfterPin: FnOnce(),
    DumpGate: FnOnce() -> Result<(), String>,
    SqlGate: FnOnce() -> Result<(), String>,
{
    if !valid_id(session_id) {
        return Err("invalid session id".into());
    }
    let pinned = PinnedDatabase::open(database_path)?;
    let mut connection = pinned.connect(after_pin, after_connection_open)?;
    connection
        .busy_timeout(Duration::from_secs(3))
        .map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("could not lock OpenCode database: {error}"))?;

    let tree = tree_sql(session_id);
    let sessions = transaction_rows(
        &transaction,
        &format!("select * from session where id in ({tree})"),
    )?;
    if sessions.is_empty() {
        return Err("session not found".into());
    }
    let ids = sessions
        .iter()
        .filter_map(|session| session.get("id").and_then(|id| id.as_str()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if ids.len() != sessions.len() || !ids.iter().all(|id| valid_id(id)) {
        return Err("OpenCode's session rows look unfamiliar — refusing to delete".into());
    }
    let messages = transaction_rows(
        &transaction,
        &format!("select * from message where session_id in ({tree})"),
    )?;
    let parts = transaction_rows(
        &transaction,
        &format!("select * from part where session_id in ({tree})"),
    )?;
    let root = sessions
        .iter()
        .find(|session| session.get("id").and_then(|id| id.as_str()) == Some(session_id))
        .ok_or("session not found")?;
    let field = |key: &str| {
        root.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let header = serde_json::json!({
        "kind": DUMP_KIND,
        "version": 1,
        "id": session_id,
        "title": field("title"),
        "directory": field("directory"),
        "sessions": sessions.len(),
        "messages": messages.len(),
        "parts": parts.len(),
    });

    let temporary_name = CString::new(format!(".aiterm-opencode-dump-{}", uuid::Uuid::new_v4()))
        .expect("UUID dump name cannot contain NUL");
    let final_name =
        CString::new(format!("{session_id}.jsonl")).map_err(|_| "invalid session dump name")?;
    let temporary_fd = unsafe {
        libc::openat(
            trash.as_raw_fd(),
            temporary_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if temporary_fd < 0 {
        return Err(format!(
            "could not create the trash dump: {}",
            std::io::Error::last_os_error()
        ));
    }
    let temporary = unsafe { std::fs::File::from_raw_fd(temporary_fd) };
    if let Err(error) = write_dump(temporary, &header, &sessions, &messages, &parts) {
        unlink_named(trash, &temporary_name);
        return Err(format!("could not write the trash dump: {error}"));
    }
    if let Err(error) = dump_gate() {
        unlink_named(trash, &temporary_name);
        return Err(error);
    }
    if let Err(error) = rename_noreplace(trash, &temporary_name, trash, &final_name) {
        unlink_named(trash, &temporary_name);
        return Err(format!("could not publish the trash dump: {error}"));
    }
    trash
        .sync_all()
        .map_err(|error| format!("could not make the trash dump durable: {error}"))?;

    if let Err(error) = sql_gate() {
        drop(transaction);
        unlink_named(trash, &final_name);
        return Err(error);
    }
    let placeholders = (1..=ids.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let parameters = || rusqlite::params_from_iter(ids.iter());
    for (table, column) in [
        ("part", "session_id"),
        ("message", "session_id"),
        ("session", "id"),
    ] {
        if let Err(error) = transaction.execute(
            &format!("delete from {table} where {column} in ({placeholders})"),
            parameters(),
        ) {
            drop(transaction);
            unlink_named(trash, &final_name);
            return Err(format!("could not delete OpenCode rows: {error}"));
        }
    }
    // On a commit error the durable dump remains. SQLite may have committed
    // despite a late I/O report, so removing the only recovery copy would be
    // the unsafe choice.
    transaction
        .commit()
        .map_err(|error| format!("could not commit OpenCode deletion: {error}"))?;
    Ok(())
}

/// Name and project for a trash dump, read off its header line.
///
/// `trash_list` titles claude transcripts by parsing them; a dump is not one,
/// and without this it would list as `session ses_xxxx` from nowhere.
pub fn dump_meta(path: &std::path::Path) -> Option<(String, String)> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let first = std::io::BufReader::new(file).lines().next()?.ok()?;
    let v: serde_json::Value = serde_json::from_str(&first).ok()?;
    if v.get("kind").and_then(|k| k.as_str()) != Some(DUMP_KIND) {
        return None;
    }
    let title = v.get("title").and_then(|t| t.as_str()).unwrap_or_default();
    let dir = v
        .get("directory")
        .and_then(|d| d.as_str())
        .unwrap_or_default();
    Some((title.to_string(), dir.to_string()))
}

/// OpenCode's sessions as a [`SessionProvider`].
///
/// `scan_with_paths` and `find_session_file` owe every caller a `PathBuf`, and
/// what they return is the database — the honest answer, since that genuinely
/// is where the session lives. It must never be *read* as a transcript, and it
/// is not: `messages` answers first everywhere a transcript would be opened.
/// It must never be *renamed into the trash* either, and it is not —
/// `session_delete` routes OpenCode ids to [`delete_to_trash`] before its
/// file-move path can see this database.
pub struct OpencodeSessions;

impl SessionProvider for OpencodeSessions {
    fn scan_with_paths(&self) -> Vec<(Session, PathBuf)> {
        let Some(db) = db_path() else {
            return vec![];
        };
        sessions().into_iter().map(|s| (s, db.clone())).collect()
    }

    fn scan_with_paths_bounded(
        &self,
        budget: &mut crate::sessions::DiscoveryBudget,
    ) -> Vec<(Session, PathBuf)> {
        let Some(db) = db_path() else {
            return vec![];
        };
        let rows = sessions_bounded(budget.remaining());
        rows.into_iter()
            .take_while(|_| budget.claim_file())
            .map(|session| (session, db.clone()))
            .collect()
    }

    fn find_session_file(&self, session_id: &str) -> Option<PathBuf> {
        has_session(session_id).then(db_path)?
    }

    fn messages(&self, session_id: &str) -> Option<Vec<(String, String)>> {
        valid_id(session_id).then(|| messages(session_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ---- id validation -------------------------------------------------- */

    /// The one thing standing between a frontend string and SQL text. A real id
    /// passes; anything carrying SQL punctuation does not, which is what makes
    /// interpolating a validated id safe.
    #[test]
    fn only_a_real_looking_session_id_reaches_the_database() {
        assert!(valid_id("ses_03ee94418ffeDX6l2Xs5hpVzcN"));
        assert!(!valid_id("ses_abc'; DROP TABLE session;--"));
        assert!(!valid_id("ses_abc' or '1'='1"));
        assert!(!valid_id("ses_"), "a bare prefix names nothing");
        assert!(!valid_id(""));
        // Other engines' ids: rejected without spawning sqlite3 at all.
        assert!(!valid_id("00000000-0000-4000-8000-000000000000"));
        assert!(!valid_id("../../etc/passwd"));
        assert!(!valid_id("ses_abc def"));
    }

    /// Refusal is not silent-ish: a rejected id must produce nothing, not an
    /// empty query against the real database.
    #[test]
    fn a_rejected_id_yields_no_conversation_and_no_ownership() {
        assert!(messages("ses_abc'; DROP TABLE session;--").is_empty());
        assert!(!has_session("ses_abc'; DROP TABLE session;--"));
        assert!(OpencodeSessions.messages("not-an-opencode-id").is_none());
    }

    /* ---- session rows --------------------------------------------------- */

    /// sqlite3's `-json` output as it really arrives, including the two rows
    /// that must not become sidebar entries: a subagent child and an archived
    /// session.
    const SESSION_JSON: &str = r#"[
      {"id":"ses_03ee94418ffeDX6l2Xs5hpVzcN","title":"Greeting","directory":"/home/matt/Projects/deepseek-test","time_updated":1785651210026,"parent_id":null,"time_archived":null},
      {"id":"ses_2399155baffe6MQ64zL3p836pn","title":"README content review","directory":"/home/matt/Projects/proxy-test","time_updated":1777151773990,"parent_id":null,"time_archived":null},
      {"id":"ses_child00000000000000000000","title":"subagent run","directory":"/home/matt/Projects/proxy-test","time_updated":1777151774000,"parent_id":"ses_2399155baffe6MQ64zL3p836pn","time_archived":null},
      {"id":"ses_archived0000000000000000","title":"old work","directory":"/home/matt/Projects/proxy-test","time_updated":1777151775000,"parent_id":null,"time_archived":1777151999000}
    ]"#;

    #[test]
    fn a_session_row_carries_everything_the_sidebar_needs() {
        let rows = parse_sessions(SESSION_JSON);
        let s = &rows[0];
        assert_eq!(s.id, "ses_03ee94418ffeDX6l2Xs5hpVzcN");
        assert_eq!(s.agent, "opencode");
        assert_eq!(s.title, "Greeting");
        assert_eq!(s.project_path, "/home/matt/Projects/deepseek-test");
        assert_eq!(
            s.group_path, s.project_path,
            "nothing regroups an opencode row"
        );
        // Unix millis, the same unit `scan_chats` reports — not seconds.
        assert_eq!(s.last_active, 1785651210026);
        assert_eq!(s.branch, None);
        assert!(!s.forked && !s.background && s.fork_parent.is_none());
    }

    /// A subagent run is a step inside a conversation, not one of your
    /// sessions; an archived session has been put away. Neither belongs in the
    /// sidebar, and the parser drops both even when the SQL forgets to.
    #[test]
    fn child_and_archived_sessions_never_become_rows() {
        let rows = parse_sessions(SESSION_JSON);
        assert_eq!(rows.len(), 2, "only the two top-level live sessions");
        let ids: Vec<&str> = rows.iter().map(|s| s.id.as_str()).collect();
        assert!(!ids.contains(&"ses_child00000000000000000000"));
        assert!(!ids.contains(&"ses_archived0000000000000000"));
    }

    /// Nothing to read is an empty list, not a panic: sqlite3 prints nothing at
    /// all for a query that matched no rows.
    #[test]
    fn no_rows_and_no_output_are_both_just_empty() {
        assert!(parse_sessions("").is_empty());
        assert!(parse_sessions("[]").is_empty());
        assert!(parse_sessions("not json").is_empty());
        assert!(parse_messages("").is_empty());
        assert!(parse_messages("[]").is_empty());
        assert!(parse_messages("garbage").is_empty());
    }

    /* ---- conversation --------------------------------------------------- */

    /// A `message.data` / `part.data` pair as sqlite3 hands it over: the blobs
    /// are JSON *strings* inside the JSON row, so everything survives two
    /// layers of escaping or nothing does.
    fn row(message_id: &str, role: &str, part: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "message_id": message_id,
            "message_data": serde_json::json!({"role": role, "agent": "build"}).to_string(),
            "part_data": part.to_string(),
        })
    }

    fn text_part(text: &str) -> serde_json::Value {
        serde_json::json!({"type": "text", "text": text})
    }

    /// The real shape of the "Greeting" session: a user text part, a
    /// `step-start`, the assistant's reply, a `step-finish`. Only the prose
    /// comes out, in the order it was said.
    #[test]
    fn only_text_parts_become_conversation_and_the_machinery_is_skipped() {
        let json = serde_json::json!([
            row("msg_1", "user", text_part("hello")),
            row(
                "msg_2",
                "assistant",
                serde_json::json!({"type": "step-start"})
            ),
            row(
                "msg_2",
                "assistant",
                text_part("Hello! How can I help you today?")
            ),
            row(
                "msg_2",
                "assistant",
                serde_json::json!({
                    "reason": "stop", "type": "step-finish",
                    "tokens": {"total": 8586}, "cost": 0.000239202
                })
            ),
            row(
                "msg_3",
                "user",
                serde_json::json!({
                    "type": "tool", "tool": "write",
                    "state": {"status": "completed", "output": "Wrote file successfully."}
                })
            ),
        ])
        .to_string();
        assert_eq!(
            parse_messages(&json),
            vec![
                ("user".to_string(), "hello".to_string()),
                (
                    "assistant".to_string(),
                    "Hello! How can I help you today?".to_string()
                ),
            ]
        );
    }

    /// One assistant turn split across parts is one turn, not two: OpenCode
    /// starts a new part at every step boundary, which is its storage and not
    /// something that happened in the conversation.
    #[test]
    fn a_turn_split_across_parts_comes_back_whole() {
        let json = serde_json::json!([
            row("msg_1", "assistant", text_part("First half.")),
            row("msg_1", "assistant", text_part("Second half.")),
            row("msg_2", "user", text_part("thanks")),
        ])
        .to_string();
        assert_eq!(
            parse_messages(&json),
            vec![
                (
                    "assistant".to_string(),
                    "First half.\nSecond half.".to_string()
                ),
                ("user".to_string(), "thanks".to_string()),
            ]
        );
    }

    /// Quotes, newlines and non-ASCII go through JSON twice — once as the blob
    /// inside the row, once as the row itself. Losing a layer would either
    /// truncate the text at the first quote or hand back an escaped mess, and
    /// both would be invisible until someone read their own transcript back.
    #[test]
    fn quotes_newlines_and_unicode_survive_the_round_trip() {
        let nasty = "he said \"don't\"\nthen ✦ — 日本語\\ok";
        let json = serde_json::json!([row("msg_1", "user", text_part(nasty))]).to_string();
        assert_eq!(
            parse_messages(&json),
            vec![("user".to_string(), nasty.to_string())]
        );

        let titled = serde_json::json!([{
            "id": "ses_abc123",
            "title": nasty,
            "directory": "/tmp/a b\"c",
            "time_updated": 1,
            "parent_id": null,
            "time_archived": null,
        }])
        .to_string();
        let rows = parse_sessions(&titled);
        assert_eq!(rows[0].title, nasty);
        assert_eq!(rows[0].project_path, "/tmp/a b\"c");
    }

    /// A turn with nothing in it is not a turn. An empty text part is what a
    /// cancelled generation leaves behind, and a blank row in the preview would
    /// be worse than no row.
    #[test]
    fn empty_text_is_not_a_turn() {
        let json = serde_json::json!([
            row("msg_1", "user", text_part("   ")),
            row("msg_2", "assistant", text_part("real")),
        ])
        .to_string();
        assert_eq!(
            parse_messages(&json),
            vec![("assistant".to_string(), "real".to_string())]
        );
    }

    /// A role we do not know is not a turn either — dropped rather than shown
    /// under a guessed name.
    #[test]
    fn an_unknown_role_is_dropped() {
        let json = serde_json::json!([
            row("msg_1", "system", text_part("injected")),
            row("msg_2", "user", text_part("mine")),
        ])
        .to_string();
        assert_eq!(
            parse_messages(&json),
            vec![("user".to_string(), "mine".to_string())]
        );
    }

    /* ---- against the real database, when there is one -------------------- */

    /// Everything above is parsing. This is the part that can only be checked
    /// against OpenCode itself, and it is skipped where OpenCode is not
    /// installed — most machines, including CI.
    #[test]
    fn the_real_store_reads_back_consistently() {
        if db_path().is_none() || crate::agents::which("sqlite3").is_none() {
            return;
        }
        for s in sessions() {
            assert_eq!(s.agent, "opencode");
            assert!(valid_id(&s.id), "{} is not an opencode id", s.id);
            assert!(has_session(&s.id), "{} lists but does not resolve", s.id);
            assert_eq!(
                OpencodeSessions.find_session_file(&s.id),
                db_path(),
                "a listed session must resolve to the database",
            );
            assert!(
                OpencodeSessions.messages(&s.id).is_some(),
                "{} must answer with a conversation, never with a path to read",
                s.id,
            );
        }
    }

    /// The resume-model query's reply, as sqlite3 -json really emits it.
    #[test]
    fn a_sessions_last_reply_names_its_provider_and_model() {
        let json = r#"[{"provider":"openrouter","model":"z-ai/glm-5.2"}]"#;
        assert_eq!(
            parse_session_model(json),
            Some(("openrouter".into(), "z-ai/glm-5.2".into())),
        );
    }

    /// Half an answer is no answer: a missing or null side cannot be spelled
    /// into `provider/model`, and empty output means no assistant reply yet.
    #[test]
    fn a_partial_or_empty_model_row_is_no_answer() {
        assert_eq!(
            parse_session_model(r#"[{"provider":"openrouter","model":null}]"#),
            None
        );
        assert_eq!(
            parse_session_model(r#"[{"provider":null,"model":"m"}]"#),
            None
        );
        assert_eq!(
            parse_session_model(r#"[{"provider":"","model":"m"}]"#),
            None
        );
        assert_eq!(parse_session_model(""), None);
        assert_eq!(parse_session_model("not json"), None);
    }

    /* ---- delete ---------------------------------------------------------- */

    /// The same gate the read queries have, on the one write: an id that is
    /// not an OpenCode id gets a refusal before anything is dumped or touched
    /// — the trash path handed in here does not even exist.
    #[test]
    fn delete_refuses_a_foreign_id_before_touching_anything() {
        let nowhere = std::path::PathBuf::from("/nonexistent/trash");
        assert_eq!(
            delete_to_trash("00000000-0000-4000-8000-000000000000", &nowhere),
            Err("invalid session id".into()),
        );
        assert_eq!(
            delete_to_trash("ses_abc'; drop table session;--", &nowhere),
            Err("invalid session id".into()),
        );
    }

    /// A dump names itself on its first line, and `dump_meta` reads the name
    /// back — that is what lets the trash list a deleted OpenCode session by
    /// title instead of as `session ses_xxxx`, and lets restore recognize what
    /// it must refuse.
    #[test]
    fn a_dumps_header_line_answers_with_its_title_and_directory() {
        let p = std::env::temp_dir().join("aiterm-test-dump-meta.jsonl");
        std::fs::write(
            &p,
            concat!(
                r#"{"kind":"opencode-session-dump","version":1,"id":"ses_a","title":"hi","directory":"/home/x/p"}"#,
                "\n",
                r#"{"table":"session","row":{"id":"ses_a"}}"#,
                "\n",
            ),
        )
        .unwrap();
        assert_eq!(dump_meta(&p), Some(("hi".into(), "/home/x/p".into())));
        std::fs::remove_file(&p).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn fixture_database(path: &std::path::Path, session_id: &str, title: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(
                "create table session (id text primary key, title text, directory text, time_updated integer, parent_id text, time_archived integer);\
                 create table message (id text primary key, session_id text, data text, time_created integer);\
                 create table part (id text primary key, message_id text, session_id text, data text, time_created integer);",
            )
            .unwrap();
        connection
            .execute(
                "insert into session values (?1, ?2, '/fixture/project', 1, null, null)",
                rusqlite::params![session_id, title],
            )
            .unwrap();
        connection
            .execute(
                "insert into message values ('msg_1', ?1, '{\"role\":\"user\"}', 1)",
                [session_id],
            )
            .unwrap();
        connection
            .execute(
                "insert into part values ('part_1', 'msg_1', ?1, '{\"type\":\"text\",\"text\":\"hello\"}', 1)",
                [session_id],
            )
            .unwrap();
    }

    #[cfg(target_os = "linux")]
    fn fixture_has_session(path: &std::path::Path, session_id: &str) -> bool {
        rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap()
        .query_row(
            "select exists(select 1 from session where id = ?1)",
            [session_id],
            |row| row.get::<_, bool>(0),
        )
        .unwrap()
    }

    #[cfg(target_os = "linux")]
    fn fixture_directory(path: &std::path::Path) -> std::fs::File {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sqlite_can_commit_with_normal_journaling_through_a_held_object_fd() {
        let root = std::env::temp_dir().join(format!(
            "aiterm-opencode-object-fd-probe-{}",
            uuid::Uuid::new_v4()
        ));
        let database = root.join("store/opencode.db");
        let session_id = "ses_objectfdprobe";
        fixture_database(&database, session_id, "Before");
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&database)
            .unwrap();
        let held_path = std::path::PathBuf::from(format!(
            "/proc/self/fd/{}",
            held.as_raw_fd()
        ));
        let mut connection = rusqlite::Connection::open_with_flags(
            held_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        transaction
            .execute(
                "update session set title = 'After' where id = ?1",
                [session_id],
            )
            .unwrap();
        transaction.commit().unwrap();
        drop(connection);

        let title = rusqlite::Connection::open(&database)
            .unwrap()
            .query_row(
                "select title from session where id = ?1",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(title, "After");
        assert!(!database.with_extension("db-journal").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn opencode_delete_cannot_open_a_replacement_during_a_leaf_aba() {
        let root = std::env::temp_dir().join(format!(
            "aiterm-opencode-leaf-aba-{}",
            uuid::Uuid::new_v4()
        ));
        let database = root.join("store/opencode.db");
        let held_original = root.join("store/held-original.db");
        let replacement = root.join("store/replacement.db");
        let trash_path = root.join("trash");
        std::fs::create_dir_all(&trash_path).unwrap();
        let session_id = "ses_leafaba";
        fixture_database(&database, session_id, "Pinned database");
        let trash = fixture_directory(&trash_path);

        delete_to_trash_from_path_with_hooks(
            session_id,
            &database,
            &trash,
            || {
                std::fs::rename(&database, &held_original).unwrap();
                fixture_database(&database, session_id, "Replacement sentinel");
            },
            || {
                std::fs::rename(&database, &replacement).unwrap();
                std::fs::rename(&held_original, &database).unwrap();
            },
            || Ok(()),
            || Ok(()),
        )
        .unwrap();

        assert!(!fixture_has_session(&database, session_id));
        assert!(fixture_has_session(&replacement, session_id));
        assert_eq!(
            dump_meta(&trash_path.join(format!("{session_id}.jsonl")))
                .unwrap()
                .0,
            "Pinned database"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn opencode_delete_uses_the_pinned_database_when_its_root_is_replaced() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "aiterm-opencode-root-replacement-{}",
            uuid::Uuid::new_v4()
        ));
        let store = root.join("store");
        let database = store.join("opencode.db");
        let trash_path = root.join("trash");
        std::fs::create_dir_all(&trash_path).unwrap();
        let session_id = "ses_rootreplace";
        fixture_database(&database, session_id, "Pinned database");
        let trash = fixture_directory(&trash_path);
        let pinned_store = root.join("pinned-store");
        let outside_store = root.join("outside-store");

        let error = delete_to_trash_from_path_with_hooks(
            session_id,
            &database,
            &trash,
            || {
                std::fs::rename(&store, &pinned_store).unwrap();
                fixture_database(
                    &outside_store.join("opencode.db"),
                    session_id,
                    "Outside sentinel",
                );
                symlink(&outside_store, &store).unwrap();
            },
            || {},
            || Ok(()),
            || Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("could not open pinned OpenCode database"));
        assert!(fixture_has_session(
            &pinned_store.join("opencode.db"),
            session_id
        ));
        assert!(fixture_has_session(
            &outside_store.join("opencode.db"),
            session_id
        ));
        assert!(!trash_path.join(format!("{session_id}.jsonl")).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn opencode_delete_rejects_a_replaced_database_leaf() {
        let root = std::env::temp_dir().join(format!(
            "aiterm-opencode-leaf-replacement-{}",
            uuid::Uuid::new_v4()
        ));
        let store = root.join("store");
        let database = store.join("opencode.db");
        let original = store.join("original.db");
        let trash_path = root.join("trash");
        std::fs::create_dir_all(&trash_path).unwrap();
        let session_id = "ses_leafreplace";
        fixture_database(&database, session_id, "Pinned database");
        let trash = fixture_directory(&trash_path);

        let error = delete_to_trash_from_path_with_hooks(
            session_id,
            &database,
            &trash,
            || {
                std::fs::rename(&database, &original).unwrap();
                fixture_database(&database, session_id, "Replacement sentinel");
            },
            || {},
            || Ok(()),
            || Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("database identity changed"));
        assert!(fixture_has_session(&original, session_id));
        assert!(fixture_has_session(&database, session_id));
        assert!(!trash_path.join(format!("{session_id}.jsonl")).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn opencode_delete_rolls_back_rows_and_dump_on_dump_or_sql_failure() {
        for stage in ["dump", "sql"] {
            let root = std::env::temp_dir().join(format!(
                "aiterm-opencode-{stage}-failure-{}",
                uuid::Uuid::new_v4()
            ));
            let database = root.join("store/opencode.db");
            let trash_path = root.join("trash");
            std::fs::create_dir_all(&trash_path).unwrap();
            let session_id = if stage == "dump" {
                "ses_dumpfail"
            } else {
                "ses_sqlfail"
            };
            fixture_database(&database, session_id, "Must survive");
            let trash = fixture_directory(&trash_path);

            let result = delete_to_trash_from_path_with_hooks(
                session_id,
                &database,
                &trash,
                || {},
                || {},
                || {
                    if stage == "dump" {
                        Err("injected dump failure".into())
                    } else {
                        Ok(())
                    }
                },
                || {
                    if stage == "sql" {
                        Err("injected SQL failure".into())
                    } else {
                        Ok(())
                    }
                },
            );

            assert!(result.is_err());
            assert!(fixture_has_session(&database, session_id));
            assert!(!trash_path.join(format!("{session_id}.jsonl")).exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// The whole delete, end to end, against a *copy* of this machine's real
    /// store: dump written and readable, the session's rows gone, every other
    /// session untouched.
    ///
    /// Ignored because it points `$HOME` at the copy for the whole process,
    /// which would misdirect any test running beside it. Run it alone:
    /// `cargo test --lib -- --ignored --exact opencode::tests::delete_roundtrip_against_a_copy_of_the_real_store`
    /// It passes trivially on a machine with no OpenCode store.
    #[test]
    #[ignore]
    fn delete_roundtrip_against_a_copy_of_the_real_store() {
        let real = dirs::home_dir().unwrap().join(".local/share/opencode");
        if !real.join("opencode.db").is_file() {
            eprintln!("no OpenCode store on this machine; nothing to check");
            return;
        }
        let fake = std::env::temp_dir().join("aiterm-oc-delete-roundtrip");
        let _ = std::fs::remove_dir_all(&fake);
        let store = fake.join(".local/share/opencode");
        std::fs::create_dir_all(&store).unwrap();
        // The WAL rides along: it holds everything since the last checkpoint,
        // and a copy without it is a copy missing its newest sessions.
        for suffix in ["opencode.db", "opencode.db-wal", "opencode.db-shm"] {
            let src = real.join(suffix);
            if src.is_file() {
                std::fs::copy(&src, store.join(suffix)).unwrap();
            }
        }
        std::env::set_var("HOME", &fake);

        let all = sessions();
        assert!(
            all.len() >= 2,
            "need at least two sessions to prove isolation"
        );
        let victim = all.last().unwrap().clone();
        let keep = all.first().unwrap().clone();
        let trash = fake.join("trash");
        std::fs::create_dir_all(&trash).unwrap();

        delete_to_trash(&victim.id, &trash).expect("delete should succeed");

        let dump = trash.join(format!("{}.jsonl", victim.id));
        assert!(dump.is_file(), "the dump is the trash entry");
        assert_eq!(
            dump_meta(&dump).map(|(t, _)| t),
            Some(victim.title.clone()),
            "the dump header names the session",
        );
        assert!(!has_session(&victim.id), "the session's rows must be gone");
        assert!(has_session(&keep.id), "every other session must survive");
        let orphans = query(&format!(
            "select id from message where session_id = '{}'",
            victim.id
        ))
        .unwrap_or_default();
        assert!(
            orphans.trim().is_empty(),
            "no message rows may be left behind"
        );
    }

    /// A claude transcript's first line is JSON too — recognition rides on the
    /// `kind` field, not on parsing succeeding.
    #[test]
    fn a_transcript_is_not_mistaken_for_a_dump() {
        let p = std::env::temp_dir().join("aiterm-test-not-a-dump.jsonl");
        std::fs::write(&p, "{\"type\":\"user\",\"cwd\":\"/home/x\"}\n").unwrap();
        assert_eq!(dump_meta(&p), None);
        std::fs::remove_file(&p).unwrap();
        assert_eq!(dump_meta(std::path::Path::new("/nonexistent.jsonl")), None);
    }
}
