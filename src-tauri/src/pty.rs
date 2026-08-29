use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State};

pub struct PtyInstance {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// The pty's direct child — the login shell, not the command it runs.
    /// Killing only this leaves the real process orphaned; see `pty_kill`.
    child_pid: Option<u32>,
}

/// The global table protects allocation membership only. Each returned value
/// has its own lock, so a blocked write/resize/kill for one PTY cannot retain
/// the map guard and stall unrelated terminals.
struct PtyTable<T> {
    entries: Arc<Mutex<HashMap<u32, Arc<Mutex<T>>>>>,
}

impl<T> Clone for PtyTable<T> {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
        }
    }
}

impl<T> Default for PtyTable<T> {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl<T> PtyTable<T> {
    fn insert(&self, id: u32, value: T) {
        self.entries
            .lock()
            .unwrap()
            .insert(id, Arc::new(Mutex::new(value)));
    }

    fn get(&self, id: u32) -> Option<Arc<Mutex<T>>> {
        self.entries.lock().ok()?.get(&id).cloned()
    }

    fn remove(&self, id: u32) -> Option<Arc<Mutex<T>>> {
        self.entries.lock().ok()?.remove(&id)
    }

    fn snapshot(&self) -> Vec<(u32, Arc<Mutex<T>>)> {
        self.entries
            .lock()
            .map(|entries| {
                entries
                    .iter()
                    .map(|(id, value)| (*id, value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn with<R>(&self, id: u32, body: impl FnOnce(&mut T) -> R) -> Option<R> {
        let value = self.get(id)?;
        let mut value = value.lock().ok()?;
        Some(body(&mut value))
    }
}

#[derive(Clone, Default)]
pub struct PtyManager {
    ptys: PtyTable<PtyInstance>,
    next_id: Arc<AtomicU32>,
}

#[derive(Clone, Serialize)]
struct PtyExit {
    id: u32,
    /// The child's exit status, or `None` if it could not be reaped.
    ///
    /// This is the whole difference between "you left" and "something killed
    /// it". A shell you typed `exit` into leaves 0; a `claude` killed from
    /// `claude agents` — possibly from another terminal, possibly from a
    /// phone — does not. Without this the UI cannot tell the two apart, and it
    /// treated every death as a deliberate close: the tab vanished with no
    /// explanation and no way back but hunting the session down in the sidebar.
    code: Option<u32>,
    /// The signal that killed the child, named ("Killed", "Terminated"), when
    /// it was killed rather than having exited.
    ///
    /// `code` cannot carry this. portable-pty reports a *fixed* `exit_code()`
    /// of 1 for every signal death, so a SIGKILL and a plain `exit 1` are
    /// indistinguishable there — observed 2026-07-26, when a SIGKILLed shell
    /// told the user "exited with status 1". Reporting a made-up exit code as
    /// though the process chose it sends you looking for a failure that never
    /// happened.
    signal: Option<String>,
}

/// Receives the lifetime of one spawned PTY.
///
/// The sink belongs to the caller that created this process, so output can be
/// consumed from its first byte without giving the PTY manager any knowledge
/// of tabs, screens, or transports.
pub trait PtySink: Send + Sync + 'static {
    fn output(&self, pty_id: u32, bytes: &[u8]);
    fn exited(&self, pty_id: u32, code: Option<u32>, signal: Option<&str>);
}

/// Process-only inputs for one PTY spawn.
pub struct PtySpawnSpec {
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub size: PtySize,
    pub env_provider: Option<String>,
    pub env_model: Option<String>,
}

impl PtySpawnSpec {
    pub fn command(command: impl Into<String>) -> Self {
        Self {
            cwd: None,
            command: Some(command.into()),
            size: PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
            env_provider: None,
            env_model: None,
        }
    }
}

/// Thin adapter that keeps the existing desktop IPC contract while desktop
/// callers still spawn PTYs directly. Task 5 replaces this with tab-owned
/// attachments; process mechanics stay in [`PtyManager`].
struct DesktopPtySink {
    app: AppHandle,
    on_output: Channel<InvokeResponseBody>,
}

impl PtySink for DesktopPtySink {
    fn output(&self, _pty_id: u32, bytes: &[u8]) {
        let _ = self.on_output.send(InvokeResponseBody::Raw(bytes.to_vec()));
    }

    fn exited(&self, pty_id: u32, code: Option<u32>, signal: Option<&str>) {
        let _ = self.app.emit(
            "pty://exit",
            PtyExit {
                id: pty_id,
                code,
                signal: signal.map(str::to_owned),
            },
        );
    }
}

/// A second listener on a pty's life, for anything that is not the desktop
/// window.
///
/// The remote gateway needs the same bytes xterm.js gets, and it cannot get
/// them from the frontend: the renderer is not running when a phone attaches to
/// a tab nobody is looking at, and routing terminal traffic through it would
/// put every remote keystroke behind a frame. So the reader thread calls here
/// as well as sending on its Channel — *as well as*, never instead of. The
/// desktop path below must behave identically whether or not anyone is
/// observing.
pub trait PtyObserver: Send + Sync {
    fn on_output(&self, pty_id: u32, bytes: &[u8]);
    fn on_exit(&self, pty_id: u32, code: Option<u32>, signal: Option<&str>);
}

/// Process-global because pty reader threads have nothing else in common: each
/// one is spawned by a command, owns no state, and would otherwise need the
/// broker threaded through `pty_spawn`'s signature for a feature that is off by
/// default. Unregistered costs one uncontended read lock per 8 KiB chunk.
static OBSERVER: RwLock<Option<Arc<dyn PtyObserver>>> = RwLock::new(None);

pub fn set_observer(observer: Arc<dyn PtyObserver>) {
    if let Ok(mut slot) = OBSERVER.write() {
        *slot = Some(observer);
    }
}

/// Called when Remote Access is switched off: the broker stops seeing traffic
/// the moment the listener closes, rather than at the next app restart.
pub fn clear_observer() {
    if let Ok(mut slot) = OBSERVER.write() {
        *slot = None;
    }
}

fn observer() -> Option<Arc<dyn PtyObserver>> {
    OBSERVER.read().ok()?.clone()
}

/// Environment variables that mean "you are already inside an agent session".
///
/// A CLI agent exports these so anything it launches behaves as a *nested* run
/// rather than as a new conversation. aiterm is not a nested run. It is a
/// terminal, and the sessions it opens are the user's own — so it has to spawn
/// from a clean environment or it inherits someone else's session identity.
///
/// The damage is silent and total. With `CLAUDE_CODE_CHILD_SESSION` set, claude
/// starts with transcript saving off; the sidebar is built entirely from
/// transcripts on disk, so every session started this way is invisible in the
/// list, unresumable, and gone when its tab closes. Nothing in the UI can
/// explain it, because from aiterm's side nothing failed.
///
/// It bites exactly one group of people: those running aiterm from inside a
/// Claude Code session — which is to say, anyone developing aiterm with it.
/// Launched from a desktop entry none of these are set and the whole list is a
/// no-op. *Observed 2026-07-27: `npm run tauri dev` from a Claude Code session
/// produced sessions warning "Transcript saving is off — inherited
/// CLAUDE_CODE_CHILD_SESSION marker", and no row ever appeared.*
///
/// Named one by one rather than matched by prefix, and that is deliberate:
/// `CLAUDE_*` is not ours to claim. `CLAUDE_CONFIG_DIR` points the CLI at a
/// different config root, and dropping it would silently change which account,
/// settings and projects a session sees — trading a visible bug for an
/// invisible one. Only markers a parent session exports about *itself* belong
/// here.
///
/// As other agents are supported, add their equivalents to this list. The
/// property to preserve is the one above: strip what identifies the parent
/// session, never what configures the tool.
const AGENT_SESSION_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_AGENT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_EFFORT",
    "CLAUDE_JOB_DIR",
    "CLAUDE_PID",
];

/// What the terminal on the other end can do.
///
/// xterm.js draws 24-bit colour in both its renderers, but nothing in `TERM`
/// can say so — there is no truecolor terminfo entry to name, which is why
/// `TERM` stays the 256-colour one programs actually look up. `COLORTERM` is
/// the out-of-band signal they test for, and without it claude, delta and bat
/// all quantise to 256 and say so in a startup tip.
fn describe_terminal(cmd: &mut CommandBuilder) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
}

/// Drop the parent session's markers so the child starts as its own session.
///
/// Applies to shells as well as agent commands: a shell that inherits them is
/// one `claude` away from the same problem, and a terminal that does not behave
/// like a terminal is the harder bug to find.
fn scrub_agent_markers(cmd: &mut CommandBuilder) {
    for key in AGENT_SESSION_MARKERS {
        cmd.env_remove(key);
    }
}

/// A child exists once `spawn_command` succeeds, even if the remaining PTY
/// setup cannot produce a reader or writer. Do not leave that child running on
/// an error path: terminate it and reap its status before returning the setup
/// error to the caller.
fn reap_failed_spawn(child: &mut dyn portable_pty::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    state: State<'_, PtyManager>,
    cwd: Option<String>,
    command: Option<String>,
    cols: u16,
    rows: u16,
    on_output: Channel<InvokeResponseBody>,
    env_provider: Option<String>,
    env_model: Option<String>,
) -> Result<u32, String> {
    state.spawn(
        PtySpawnSpec {
            cwd,
            command,
            size: PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            },
            env_provider,
            env_model,
        },
        Arc::new(DesktopPtySink { app, on_output }),
    )
}

impl PtyManager {
    /// Spawn one PTY and deliver its bytes and terminal exit to `sink`.
    ///
    /// The legacy observer remains a temporary additive bridge for the remote
    /// broker. It is deliberately not used as the spawn's primary sink: the
    /// passed sink observes every byte from this PTY's reader thread.
    pub fn spawn(&self, spec: PtySpawnSpec, sink: Arc<dyn PtySink>) -> Result<u32, String> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(spec.size).map_err(|e| e.to_string())?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let mut cmd = match &spec.command {
            // Run through the login shell so PATH/aliases resolve like a normal terminal.
            Some(c) => {
                let mut b = CommandBuilder::new(&shell);
                b.args(["-i", "-c", c]);
                b
            }
            None => CommandBuilder::new(&shell),
        };
        describe_terminal(&mut cmd);
        scrub_agent_markers(&mut cmd);
        // A provider-backed tab (OpenCode on an OpenRouter model) gets the key as
        // process environment, resolved here from the provider store. It never
        // crosses the frontend and never touches a command line — /proc shows
        // argv to everyone, but environ only to the same user, which is the same
        // exposure as the tool's own credential file.
        //
        // That tab's routing rides in the same environment, for the same reason:
        // the block is compiled here from stored state, so no routing decision
        // crosses the frontend and none of it appears in argv.
        // `OPENCODE_CONFIG_CONTENT` merges over the user's own config rather than
        // replacing it, which is how a model's routing reaches OpenCode without
        // aiterm ever writing their config file.
        if let Some(pid) = spec.env_provider {
            if let Some(p) = crate::providers::load_providers()
                .iter()
                .find(|p| p.id == pid)
            {
                if p.is_openrouter() && !p.api_key.is_empty() {
                    cmd.env("OPENROUTER_API_KEY", &p.api_key);
                }
                if let Some(model) = spec.env_model.as_deref() {
                    if let Some(cfg) = crate::providers::opencode_config_content(p, model) {
                        cmd.env("OPENCODE_CONFIG_CONTENT", cfg);
                    }
                }
            }
        }
        if let Some(dir) = spec.cwd.filter(|d| std::path::Path::new(d).is_dir()) {
            cmd.cwd(dir);
        }

        let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        let child_pid = child.process_id();
        let killer = child.clone_killer();
        drop(pair.slave);

        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                reap_failed_spawn(&mut *child);
                return Err(error.to_string());
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                reap_failed_spawn(&mut *child);
                return Err(error.to_string());
            }
        };

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.ptys.insert(
            id,
            PtyInstance {
                master: pair.master,
                writer,
                killer,
                child_pid,
            },
        );

        let ptys = self.ptys.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // A single raw byte stream preserves multibyte UTF-8 across
                        // 8 KiB reads; the sink decides how its transport consumes it.
                        sink.output(id, &buf[..n]);
                        // Compatibility-only fan-out for the existing remote broker.
                        // Task 6 removes this after every consumer takes a per-spawn sink.
                        if let Some(observer) = observer() {
                            observer.on_output(id, &buf[..n]);
                        }
                    }
                }
            }
            // Reap the child before announcing the exit. The read loop ends when
            // the pty closes, which says nothing about *why* — so wait for the
            // real status. This blocks only this pty's reader thread, and the
            // child is already gone by the time we get here in every path but a
            // detaching one, so the wait returns immediately.
            let status = child.wait().ok();
            let code = status.as_ref().map(|s| s.exit_code());
            let signal = status.as_ref().and_then(|s| s.signal().map(String::from));
            // A concurrent `kill` may already have removed this entry. In
            // either order, remove only by this PTY's allocation id; never act
            // on a possibly reused OS pid after the child has been reaped.
            ptys.remove(id);
            sink.exited(id, code, signal.as_deref());
            if let Some(observer) = observer() {
                observer.on_exit(id, code, signal.as_deref());
            }
        });

        Ok(id)
    }
}

/// Send input to a pty.
///
/// `async`, and that is the whole point of it: Tauri runs a *sync* command on
/// the GTK main thread, which is also the thread that has to be free for the
/// window to draw. Every keystroke was therefore a write syscall scheduled
/// against the frame loop, and under load — several agents streaming, the
/// compositor busy — typing went soft and laggy exactly when the machine could
/// least afford it. An async command runs on the runtime instead, so a
/// keystroke never queues behind a frame.
#[tauri::command]
pub async fn pty_write(state: State<'_, PtyManager>, id: u32, data: String) -> Result<(), String> {
    write_to_pty(&state, id, data.as_bytes())
}

/// The write itself, with no transport attached, so a remote client reaches a
/// pty through the same code the window does instead of a parallel copy.
///
/// The lock is held for the write and nothing else, with no await inside it, so
/// this cannot block the runtime the command above runs on.
pub fn write_to_pty(manager: &PtyManager, id: u32, data: &[u8]) -> Result<(), String> {
    manager.write(id, data)
}

/// Off the main thread for the same reason as [`pty_write`]: a resize arrives
/// on every window drag frame, and the ioctl has no business competing with the
/// draw it was triggered by.
#[tauri::command]
pub async fn pty_resize(
    state: State<'_, PtyManager>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    resize_pty(&state, id, cols, rows)
}

/// Split from the command for the same reason as [`write_to_pty`].
pub fn resize_pty(manager: &PtyManager, id: u32, cols: u16, rows: u16) -> Result<(), String> {
    manager.resize(id, cols, rows)
}

/// Reaches the pty table through the Tauri state the app already manages, so
/// the remote broker can stay free of Tauri types: it holds this as a
/// `PtyControl` and never learns where the ptys live.
pub struct AppPtyControl {
    app: AppHandle,
}

impl AppPtyControl {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl crate::remote::terminal::PtyControl for AppPtyControl {
    fn write(&self, pty_id: u32, data: &[u8]) -> Result<(), String> {
        write_to_pty(&self.app.state::<PtyManager>(), pty_id, data)
    }

    fn resize(&self, pty_id: u32, cols: u16, rows: u16) -> Result<(), String> {
        resize_pty(&self.app.state::<PtyManager>(), pty_id, cols, rows)
    }
}

/// True while `pid` names a *running* process. A killed child keeps its
/// `/proc/<pid>` entry until its parent reaps it, so presence alone would call
/// every zombie alive — and a "did the kill work?" check built on that never
/// succeeds. State `Z` counts as dead.
pub fn pid_alive(pid: u32) -> bool {
    let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
        return false;
    };
    !status
        .lines()
        .any(|l| l.starts_with("State:") && l.contains('Z'))
}

/// Every descendant of `root` (not including `root`), deepest last — read from
/// /proc rather than from process groups. `zsh -i -c <cmd>` runs with job
/// control on, so the command lands in its **own** process group; signalling
/// the shell's group misses it entirely.
fn descendants(root: u32) -> Vec<u32> {
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in procs.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        if let Some(ppid) = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .and_then(|v| v.trim().parse::<u32>().ok())
        {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut out = Vec::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for &kid in children.get(&pid).into_iter().flatten() {
            out.push(kid);
            queue.push(kid);
        }
    }
    out
}

/// Stop `root` and everything under it: SIGTERM, then SIGKILL whatever is left
/// after `grace`. Children go first so a shell can't respawn or reparent one
/// mid-teardown. Returns false if anything is still alive at the end.
/// The pid of whichever process we descend from, or `None` at the top.
fn parent_of(pid: u32) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("PPid:"))
        .and_then(|v| v.trim().parse::<u32>().ok())
}

impl PtyManager {
    /// Write input to one live PTY.
    pub fn write(&self, id: u32, data: &[u8]) -> Result<(), String> {
        self.ptys
            .with(id, |pty| {
                pty.writer.write_all(data).map_err(|e| e.to_string())
            })
            .ok_or_else(|| "no such pty".to_string())?
    }

    /// Resize one live PTY without coupling the process manager to a caller's
    /// attachment or focus policy.
    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        self.ptys
            .with(id, |pty| {
                pty.master
                    .resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(|e| e.to_string())
            })
            .ok_or_else(|| "no such pty".to_string())?
    }

    /// Stop a PTY's complete process tree and release its process resources.
    pub fn kill(&self, id: u32) {
        // Take the instance out under the lock, then release it: the kill below
        // can block for over a second, and holding the map would stall every
        // other PTY's writes and resizes for that whole time.
        let taken = self.ptys.remove(id);
        if let Some(pty) = taken {
            let mut pty = pty.lock().unwrap();
            // `killer.kill()` only reaches the pty's direct child — the login
            // shell. zsh forks the command rather than exec'ing it, so killing the
            // shell orphaned every `claude` aiterm ever launched: they stayed in
            // `claude agents` forever, which made their rows permanently "running"
            // and left fork-a-copy as the only action the UI would offer.
            if let Some(pid) = pty.child_pid {
                kill_tree(pid, std::time::Duration::from_millis(1500));
            }
            let _ = pty.killer.kill();
            // Closing a tab is one of the few things that changes the roster from
            // inside aiterm. Say so, rather than letting the sidebar keep showing
            // the session as running for the rest of the cache window.
            crate::sessions::invalidate_roster();
        }
    }

    /// The pty whose child tree contains `pid`, found by walking `pid`'s
    /// ancestor chain up to some pty's direct child (the login shell).
    ///
    /// Walked upward rather than enumerating every pty's descendants because
    /// the caller has one pid and there may be many ptys: one bounded /proc
    /// walk answers for all of them. This is how a `SessionStart` hook's
    /// claude pid becomes a tab — see `hooklink.rs`.
    pub fn pty_for_descendant(&self, pid: u32) -> Option<u32> {
        let roots: HashMap<u32, u32> = self
            .ptys
            .snapshot()
            .into_iter()
            .filter_map(|(id, pty)| pty.lock().ok()?.child_pid.map(|child| (child, id)))
            .collect();
        let mut cur = pid;
        for _ in 0..64 {
            if let Some(&id) = roots.get(&cur) {
                return Some(id);
            }
            match parent_of(cur) {
                Some(p) if p > 1 => cur = p,
                _ => return None,
            }
        }
        None
    }
}

/// Our own pid and every pid we descend from.
///
/// The walk is bounded rather than trusting /proc to terminate: a pid that
/// reports itself as its own parent would otherwise spin here forever, and a
/// hang inside a kill path is worse than a short chain.
fn self_and_ancestors() -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    let mut pid = std::process::id();
    for _ in 0..64 {
        if !out.insert(pid) {
            break; // already seen — a cycle, stop rather than loop
        }
        match parent_of(pid) {
            Some(p) if p > 1 => pid = p,
            _ => break,
        }
    }
    out
}

/// Remove every pid in `ours` from a kill set, returning what was taken out.
///
/// Split from [`kill_tree`] so the rule can be tested without signalling
/// anything. The first version of that test called `kill_tree` on our own pid,
/// which reaps *all* descendants of the process — and since cargo runs tests as
/// threads in one binary, it killed a sibling test's `fc-list` child and failed
/// that test instead. A destructive test of a guard against destruction.
fn strip_own_chain(tree: &mut Vec<u32>, ours: &std::collections::HashSet<u32>) -> Vec<u32> {
    let skipped: Vec<u32> = tree.iter().copied().filter(|p| ours.contains(p)).collect();
    tree.retain(|p| !ours.contains(p));
    skipped
}

pub fn kill_tree(root: u32, grace: std::time::Duration) -> bool {
    let mut tree = descendants(root);
    tree.reverse(); // deepest first
    tree.push(root);
    // Never signal ourselves, or anything we descend from.
    //
    // aiterm is normally nowhere near this tree. But launch a second instance
    // from a shell that descends from a session, then stop that session, and
    // the walk comes straight back around into the app doing the stopping. It
    // dies by SIGTERM, so there is no panic, no coredump and nothing in the
    // journal — the window simply vanishes, which is unfalsifiable from the
    // outside. (Observed 2026-07-27: resume with two instances running, one
    // launched from inside the other.)
    //
    // The offending pids are dropped rather than the whole call refused: the
    // session still gets stopped, minus the part that would have taken us with
    // it. When `root` itself is one of ours it cannot be killed at all, and
    // that is fine — `stop_session` defines success by the roster, not by this
    // return value, so it will report an honest failure instead of a stop that
    // never happened.
    let skipped = strip_own_chain(&mut tree, &self_and_ancestors());
    if !skipped.is_empty() {
        // Loud on purpose, and to the log file rather than stderr: the bug this
        // guards against kills aiterm outright, so the evidence has to already
        // be on disk by the time anyone thinks to look for it.
        crate::diag!(
            "pty",
            "kill_tree({root}): refusing to signal {skipped:?} — \
             aiterm's own process chain is inside this tree"
        );
    }
    crate::diag!("pty", "kill_tree({root}): signalling {} pid(s)", tree.len());
    for &pid in &tree {
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if !tree.iter().any(|&p| pid_alive(p)) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for &pid in &tree {
        if pid_alive(pid) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    !tree.iter().any(|&p| pid_alive(p))
}

#[tauri::command]
pub async fn pty_kill(state: State<'_, PtyManager>, id: u32) -> Result<(), String> {
    let manager = state.inner().clone();
    crate::run_blocking(move || manager.kill(id)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_blocked_instance_does_not_hold_the_pty_table_lock() {
        let table = PtyTable::default();
        table.insert(1, "first");
        table.insert(2, "second");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocked = table.clone();
        let worker = std::thread::spawn(move || {
            blocked
                .with(1, |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                })
                .unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let independent = table.clone();
        let independent_worker = std::thread::spawn(move || {
            let value = independent.with(2, |value| *value).unwrap();
            done_tx.send(value).unwrap();
        });
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "second",
            "instance 1 held the global PTY map while blocked"
        );

        release_tx.send(()).unwrap();
        worker.join().unwrap();
        independent_worker.join().unwrap();
    }

    /// The regression that started all of this: killing a shell does not kill
    /// what the shell forked. `zsh -i -c claude …` forks rather than execs, so
    /// every `claude` aiterm launched outlived its tab and stayed in the
    /// roster forever, which made its row permanently "running".
    #[test]
    fn kill_tree_reaps_grandchildren() {
        let mut sh = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .spawn()
            .expect("spawn sh");
        let root = sh.id();
        // Give the shell a moment to fork the `sleep`.
        let mut kids = Vec::new();
        for _ in 0..40 {
            kids = descendants(root);
            if !kids.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!kids.is_empty(), "sh never forked a child to test against");

        assert!(
            kill_tree(root, Duration::from_millis(1500)),
            "tree survived"
        );
        assert!(!pid_alive(root), "shell still alive");
        for kid in kids {
            assert!(!pid_alive(kid), "grandchild {kid} outlived the kill");
        }
        let _ = sh.wait();
    }

    /// The whole "keep the tab when something killed it" behaviour rests on one
    /// assumption: that waiting on the pty's child after the read loop ends
    /// yields a status that tells clean exits apart from everything else. If
    /// this ever reports 0 for a killed process, the UI silently goes back to
    /// dropping tabs with no explanation — which is the bug it was built for.
    #[test]
    fn exit_status_separates_leaving_from_dying() {
        for (script, want) in [("exit 0", 0u32), ("exit 7", 7)] {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("openpty");
            let mut cmd = CommandBuilder::new("sh");
            cmd.args(["-c", script]);
            let mut child = pair.slave.spawn_command(cmd).expect("spawn");
            drop(pair.slave);
            // Drain, exactly as the reader thread does: the wait can block
            // forever behind a pty whose output nobody is reading.
            let mut reader = pair.master.try_clone_reader().expect("reader");
            std::thread::spawn(move || {
                let mut sink = Vec::new();
                let _ = reader.read_to_end(&mut sink);
            });
            let status = child.wait().expect("wait");
            assert_eq!(
                status.exit_code(),
                want,
                "`{script}` reported the wrong status"
            );
        }
    }

    /// A process killed by a signal must not look like a clean exit — that is
    /// the case a session stopped from `claude agents` actually takes.
    #[test]
    fn a_signalled_child_is_not_reported_as_clean() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);
        let pid = child.process_id().expect("pid");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        std::thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = reader.read_to_end(&mut sink);
        });
        assert!(kill_tree(pid, Duration::from_millis(1500)), "tree survived");
        let status = child.wait().expect("wait");
        assert_ne!(
            status.exit_code(),
            0,
            "a killed child looked like a clean exit"
        );
        // And the status must say *how* it died. exit_code() alone is 1 for
        // every signal, which is also what `exit 1` reports — the ambiguity
        // that had a SIGKILLed shell claiming it "exited with status 1".
        assert!(
            status.signal().is_some(),
            "a signalled child carried no signal name, so the UI can only guess",
        );
    }

    /// Both halves of the contract, in one test on purpose: `set_var` mutates
    /// process-wide state, and two tests doing it concurrently under cargo's
    /// thread pool is a race waiting to be debugged by someone else.
    ///
    /// The marker half is the bug this exists for — a session inheriting
    /// `CLAUDE_CODE_CHILD_SESSION` runs with transcript saving off, so it never
    /// gets a row and cannot be resumed. `CommandBuilder::new` snapshots the
    /// real environment, so this asserts against a genuinely inherited value
    /// rather than one the test also set on the builder.
    ///
    /// The config half guards the fix against being "simplified" into a
    /// `CLAUDE_*` prefix match later. `CLAUDE_CONFIG_DIR` points the CLI at a
    /// different config root; dropping it would silently change which account
    /// and projects a session sees — a quieter bug than the one being fixed.
    #[test]
    fn scrub_strips_session_markers_but_not_configuration() {
        std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1");
        std::env::set_var("CLAUDE_CONFIG_DIR", "/tmp/some-other-config");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");

        let mut cmd = CommandBuilder::new("true");
        assert!(
            cmd.get_env("CLAUDE_CODE_CHILD_SESSION").is_some(),
            "precondition: the builder should inherit the process environment",
        );
        scrub_agent_markers(&mut cmd);

        assert!(
            cmd.get_env("CLAUDE_CODE_CHILD_SESSION").is_none(),
            "the marker survived the scrub, so spawned sessions save no transcript",
        );
        assert_eq!(
            cmd.get_env("CLAUDE_CONFIG_DIR"),
            Some(std::ffi::OsStr::new("/tmp/some-other-config")),
            "config, not a session marker — stripping it silently repoints the CLI",
        );
        assert!(
            cmd.get_env("ANTHROPIC_API_KEY").is_some(),
            "credentials must survive"
        );

        std::env::remove_var("CLAUDE_CODE_CHILD_SESSION");
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    /// TERM names a terminfo entry programs look up; COLORTERM is the separate
    /// signal for 24-bit colour, which no TERM value can carry. Both are
    /// needed, and dropping either quietly costs colour depth rather than
    /// failing.
    #[test]
    fn a_spawned_terminal_advertises_truecolour_as_well_as_256() {
        let mut cmd = CommandBuilder::new("true");
        describe_terminal(&mut cmd);
        assert_eq!(cmd.get_env("TERM"), Some("xterm-256color".as_ref()));
        assert_eq!(cmd.get_env("COLORTERM"), Some("truecolor".as_ref()));
    }

    /// A scrub after the description must not take the terminal's own
    /// capabilities with it — they are not a parent session's markers.
    #[test]
    fn scrubbing_agent_markers_leaves_the_colour_signal_alone() {
        let mut cmd = CommandBuilder::new("true");
        describe_terminal(&mut cmd);
        scrub_agent_markers(&mut cmd);
        assert_eq!(cmd.get_env("COLORTERM"), Some("truecolor".as_ref()));
    }

    /// A shell tab is scrubbed too — it is one `claude` away from the same
    /// problem, and PATH/HOME must still be intact for it to be a usable shell.
    #[test]
    fn scrub_keeps_the_ordinary_environment() {
        let mut cmd = CommandBuilder::new("true");
        scrub_agent_markers(&mut cmd);
        for key in ["PATH", "HOME"] {
            assert!(cmd.get_env(key).is_some(), "{key} was lost");
        }
    }

    /// The observer is strictly additive, so the property that matters is that
    /// an unregistered one is genuinely absent — the reader thread must do
    /// nothing extra for a desktop that never enabled Remote Access. The
    /// fan-out itself is covered by `tests/remote_terminal.rs`.
    #[test]
    fn the_pty_observer_is_absent_until_something_registers() {
        struct Counting(AtomicU32);
        impl PtyObserver for Counting {
            fn on_output(&self, _pty_id: u32, _bytes: &[u8]) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
            fn on_exit(&self, _pty_id: u32, _code: Option<u32>, _signal: Option<&str>) {}
        }

        assert!(
            observer().is_none(),
            "nothing should be observing by default"
        );
        let counting = Arc::new(Counting(AtomicU32::new(0)));
        set_observer(counting.clone());
        observer()
            .expect("a registered observer should be handed back")
            .on_output(1, b"x");
        assert_eq!(counting.0.load(Ordering::SeqCst), 1);

        clear_observer();
        assert!(observer().is_none(), "clearing must really unregister");
    }

    #[test]
    fn pid_alive_tracks_reality() {
        assert!(pid_alive(std::process::id()));
        // Not a pid Linux will hand out (default pid_max is 4194304).
        assert!(!pid_alive(u32::MAX));
    }

    #[test]
    fn descendants_excludes_the_root_itself() {
        let me = std::process::id();
        assert!(!descendants(me).contains(&me));
    }

    #[test]
    fn our_own_chain_is_us_and_our_parent() {
        let chain = self_and_ancestors();
        let me = std::process::id();
        assert!(chain.contains(&me), "our own pid is missing from the chain");
        if let Some(parent) = parent_of(me).filter(|&p| p > 1) {
            assert!(
                chain.contains(&parent),
                "the walk stopped before our parent"
            );
        }
    }

    /// The regression guard. Tested through `strip_own_chain` rather than by
    /// calling `kill_tree` on ourselves — see that function's comment for why
    /// the obvious version of this test was worse than no test.
    #[test]
    fn a_kill_set_holding_our_own_chain_is_stripped() {
        let me = std::process::id();
        // Above default pid_max, so these can never name a real process.
        let (unrelated_a, unrelated_b) = (4_194_305, 4_194_306);
        let mut tree = vec![unrelated_a, me, unrelated_b];
        let skipped = strip_own_chain(&mut tree, &self_and_ancestors());

        assert!(
            skipped.contains(&me),
            "our own pid was left in the kill set"
        );
        assert!(!tree.contains(&me), "our own pid survived the strip");
        assert_eq!(
            tree,
            vec![unrelated_a, unrelated_b],
            "pids that are not ours must still be signalled",
        );
    }

    #[test]
    fn a_kill_set_of_strangers_is_left_alone() {
        let mut tree = vec![4_194_305, 4_194_306];
        let skipped = strip_own_chain(&mut tree, &self_and_ancestors());
        assert!(skipped.is_empty(), "stripped a pid that was not ours");
        assert_eq!(tree, vec![4_194_305, 4_194_306]);
    }
}
