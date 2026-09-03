//! The spine's registry: one bounded event log per session, the adapter
//! driver that fills it, and the broadcast every consumer reads.
//!
//! Lives in Tauri managed state as `Arc<Spine>`. See `docs/spine.md` for the
//! lifecycle this implements.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Manager};
use tokio::sync::broadcast;
use tokio::sync::mpsc::{self, UnboundedSender};

use super::{now_ms, Adapter, Kind, Phase, SpineEvent};

/// Ring bounds, whichever comes first. 5 000 events is a long day's session;
/// 4 MB is where keeping history stops being free.
const MAX_EVENTS: usize = 5_000;
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// A tail with no tab bound and nobody asking dies after this.
const INTEREST_TTL: Duration = Duration::from_secs(15 * 60);
/// How often a driver asks whether it is still wanted.
const REAP_EVERY: Duration = Duration::from_secs(60);
/// Fallback poll when no watched file moved — and the only poll the legacy
/// adapter usually gets, since opencode keeps sessions in a database with no
/// path to watch.
const TICK: Duration = Duration::from_secs(2);
/// A transcript append is several lines and lands as several inotify events;
/// one poll per burst, not one per line.
const COALESCE: Duration = Duration::from_millis(250);
/// Deep enough that bootstrapping a long session cannot lag a phone that is
/// keeping up. Its own channel on purpose: a burst here must not push the
/// coarse `remote_api::Event`s out of theirs.
const BROADCAST_CAPACITY: usize = 1024;
/// How long a `GET …/spine` waits for a just-started tail to finish reading
/// history. Answering the first call empty leaves the phone on a blank
/// screen until something else happens to move.
const BOOTSTRAP_GRACE: Duration = Duration::from_secs(2);

/// One session's log. Created by the first `push` or `ensure_tail` for that
/// id and kept after the tail stops, so a phone that comes back late still
/// gets what it missed.
struct SessionLog {
    agent: String,
    events: VecDeque<SpineEvent>,
    /// Running sum of `weight`, so bounding does not re-walk the ring.
    bytes: usize,
    next_seq: u64,
    last_interest: Instant,
    /// The driver task, while one runs. Kept so `ensure_tail` is idempotent.
    tail: Option<tauri::async_runtime::JoinHandle<()>>,
    /// The adapter opened AND reads the engine's own live source. Reported
    /// to a phone as `live`.
    live: bool,
    /// `bootstrap()` has returned; a waiting GET can stop waiting.
    ready: bool,
    /// The last phase pushed. The terminal publishes cadence four times a
    /// second while output flows; without this the ring would be nothing
    /// but identical `working` events.
    last_phase: Option<Phase>,
}

impl SessionLog {
    fn new(agent: &str) -> Self {
        Self {
            agent: agent.to_string(),
            events: VecDeque::new(),
            bytes: 0,
            next_seq: 1,
            last_interest: Instant::now(),
            tail: None,
            live: false,
            ready: false,
            last_phase: None,
        }
    }

    fn tailing(&self) -> bool {
        self.tail.as_ref().is_some_and(|h| !h.inner().is_finished())
    }
}

pub struct Spine {
    epoch: u64,
    sessions: Mutex<HashMap<String, SessionLog>>,
    /// session id → agent id. Resolving one walks every backend's session
    /// directory; a phone polling one session must not pay that twice.
    agents: Mutex<HashMap<String, String>>,
    tx: broadcast::Sender<SpineEvent>,
}

impl Default for Spine {
    fn default() -> Self {
        Self::new()
    }
}

impl Spine {
    pub fn new() -> Self {
        Self {
            epoch: now_ms(),
            sessions: Mutex::new(HashMap::new()),
            agents: Mutex::new(HashMap::new()),
            tx: broadcast::channel(BROADCAST_CAPACITY).0,
        }
    }

    /// When this registry started, in ms. A phone that sees a new epoch
    /// throws away everything it holds — the seq numbering started over.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SpineEvent> {
        self.tx.subscribe()
    }

    /// Stamp, store and broadcast one event. The only way anything enters
    /// the spine.
    pub fn push(&self, session_id: &str, agent: &str, ts: u64, kind: Kind) -> SpineEvent {
        let ev = {
            let mut sessions = self.sessions.lock().unwrap();
            let log = sessions
                .entry(session_id.to_string())
                .or_insert_with(|| SessionLog::new(agent));
            let ev = SpineEvent {
                seq: log.next_seq,
                epoch: self.epoch,
                session_id: session_id.to_string(),
                agent: agent.to_string(),
                ts,
                kind,
            };
            log.next_seq += 1;
            // A reset says everything before it is gone. Keeping the old
            // events would only hand a reconnecting phone history it is
            // about to throw away — and they are the events most likely to
            // be the bulk of the ring.
            if matches!(ev.kind, Kind::Reset) {
                log.events.clear();
                log.bytes = 0;
            }
            log.bytes += weight(&ev);
            log.events.push_back(ev.clone());
            while log.events.len() > MAX_EVENTS
                || (log.bytes > MAX_BYTES && log.events.len() > 1)
            {
                if let Some(old) = log.events.pop_front() {
                    log.bytes = log.bytes.saturating_sub(weight(&old));
                }
            }
            ev
        };
        // Outside the lock: a subscriber's wake must never be able to
        // re-enter the registry while it is held.
        let _ = self.tx.send(ev.clone());
        ev
    }

    /// Everything after `after_seq` that is still in the ring. `after_seq`
    /// of 0 means all of it.
    pub fn after(&self, session_id: &str, after_seq: u64) -> Vec<SpineEvent> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|log| log.events.iter().filter(|e| e.seq > after_seq).cloned().collect())
            .unwrap_or_default()
    }

    /// Whether this session's adapter reads the engine's own source, as
    /// opposed to the legacy re-derivation (or nothing at all).
    pub fn is_live(&self, session_id: &str) -> bool {
        self.sessions.lock().unwrap().get(session_id).is_some_and(|l| l.live)
    }

    /// Whether the tail has finished reading history.
    pub fn is_ready(&self, session_id: &str) -> bool {
        self.sessions.lock().unwrap().get(session_id).is_some_and(|l| l.ready)
    }

    /// Start the adapter driver for a session if one is not already
    /// running, and mark the session as wanted either way.
    pub fn ensure_tail(self: &Arc<Self>, app: &AppHandle, session_id: &str, agent: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        let log = sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionLog::new(agent));
        log.last_interest = Instant::now();
        if log.tailing() {
            return;
        }
        log.ready = false;
        log.tail = Some(tauri::async_runtime::spawn(drive(
            self.clone(),
            app.clone(),
            session_id.to_string(),
            agent.to_string(),
        )));
    }

    /// Bridge a terminal activity verdict onto the spine, for a session
    /// that already has a tail. Deliberately does not start one: a phase
    /// with no content behind it is not worth opening a transcript for.
    pub fn push_phase_if_tailed(&self, session_id: &str, phase: Phase) {
        let agent = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(log) = sessions.get_mut(session_id) else { return };
            if !log.tailing() || log.last_phase == Some(phase) {
                return;
            }
            log.last_phase = Some(phase);
            log.agent.clone()
        };
        self.push(session_id, &agent, now_ms(), Kind::Phase { phase, detail: String::new() });
    }

    fn agent_of(&self, session_id: &str) -> Option<String> {
        self.agents.lock().unwrap().get(session_id).cloned()
    }

    fn remember_agent(&self, session_id: &str, agent: &str) {
        self.agents.lock().unwrap().insert(session_id.to_string(), agent.to_string());
    }

    fn set_flags(&self, session_id: &str, live: Option<bool>, ready: Option<bool>) {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(log) = sessions.get_mut(session_id) else { return };
        if let Some(v) = live {
            log.live = v;
        }
        if let Some(v) = ready {
            log.ready = v;
        }
    }

    fn has_events(&self, session_id: &str) -> bool {
        self.sessions.lock().unwrap().get(session_id).is_some_and(|l| !l.events.is_empty())
    }

    /// A tail is wanted while a tab is bound to the session, or while
    /// somebody asked about it recently.
    fn still_wanted(&self, app: &AppHandle, session_id: &str) -> bool {
        if let Some(tabs) = app.try_state::<Arc<crate::tabs::TabRegistry>>() {
            if tabs.bound_sessions().iter().any(|s| s == session_id) {
                return true;
            }
        }
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .is_some_and(|l| l.last_interest.elapsed() < INTEREST_TTL)
    }
}

/// Roughly what one event costs to hold, for the byte bound. Exact JSON
/// length would mean serializing every event twice; the texts are all of
/// the size and the constant covers the envelope.
fn weight(ev: &SpineEvent) -> usize {
    let body = match &ev.kind {
        Kind::UserMessage { id, text }
        | Kind::AgentText { id, text, .. }
        | Kind::AgentThought { id, text, .. } => id.len() + text.len(),
        Kind::ToolCall { id, tool, title, input, .. } => {
            id.len() + tool.len() + title.len() + input.len()
        }
        Kind::ToolCallUpdate { id, output, .. } => {
            id.len() + output.as_ref().map_or(0, |o| o.len())
        }
        Kind::TurnStarted { turn } => turn.len(),
        Kind::TurnEnded { turn, reason } => turn.len() + reason.len(),
        Kind::Phase { detail, .. } => detail.len(),
        Kind::Reset => 0,
    };
    body + ev.session_id.len() + ev.agent.len() + 64
}

/// The agent id that owns a session, cached. `None` when no backend claims
/// it — a session id the phone made up, or a transcript that has gone.
pub async fn resolve_agent(spine: &Arc<Spine>, session_id: &str) -> Option<String> {
    if let Some(agent) = spine.agent_of(session_id) {
        return Some(agent);
    }
    let sid = session_id.to_string();
    let found = crate::run_blocking(move || {
        let list = crate::agents::backends();
        crate::agents::owner_in(&list, &sid).map(|(b, _)| b.id().to_string())
    })
    .await?;
    spine.remember_agent(session_id, &found);
    Some(found)
}

/// Start (or refresh) a tail for a session, resolving its agent first.
/// Callable from a plain thread — the tabs registry bridge runs on one and
/// has no async context of its own.
pub fn ensure_tail_for(app: &AppHandle, session_id: &str) {
    let Some(spine) = app.try_state::<Arc<Spine>>().map(|s| s.inner().clone()) else { return };
    let app = app.clone();
    let sid = session_id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Some(agent) = resolve_agent(&spine, &sid).await {
            spine.ensure_tail(&app, &sid, &agent);
        }
    });
}

/// Bridge the terminal's activity verdict onto the spine.
pub fn push_phase(app: &AppHandle, session_id: &str, activity: &str) {
    let phase = match activity {
        // The tabs bridge publishes raw terminal cadence as "output"; the
        // sessions endpoint upgrades it to working/attention/idle before a
        // phone sees it. Both spellings reach here, so both are accepted.
        "working" | "output" => Phase::Working,
        "attention" => Phase::NeedsYou,
        "idle" => Phase::Idle,
        _ => return,
    };
    let Some(spine) = app.try_state::<Arc<Spine>>() else { return };
    spine.push_phase_if_tailed(session_id, phase);
}

/// Answer `GET /v1/sessions/{id}/spine`: register interest, wait out a
/// first bootstrap, and hand back everything after `after_seq`.
pub async fn read_after(
    app: &AppHandle,
    session_id: &str,
    after_seq: u64,
) -> Option<(u64, bool, Vec<SpineEvent>)> {
    let spine = app.try_state::<Arc<Spine>>().map(|s| s.inner().clone())?;
    let agent = resolve_agent(&spine, session_id).await?;
    spine.ensure_tail(app, session_id, &agent);
    let deadline = Instant::now() + BOOTSTRAP_GRACE;
    while !spine.is_ready(session_id) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Some((spine.epoch(), spine.is_live(session_id), spine.after(session_id, after_seq)))
}

// ------------------------------------------------------------- the driver

/// One tokio task per tailed session: open the adapter, replay history,
/// then poll on every change of a watched file and on the fallback tick
/// until nobody wants this session any more.
async fn drive(spine: Arc<Spine>, app: AppHandle, session_id: String, agent: String) {
    let short = short(&session_id);
    let opened = {
        let (a, s) = (agent.clone(), session_id.clone());
        crate::run_blocking(move || super::open_adapter(&a, &s)).await
    };
    let Some(adapter) = opened else {
        crate::diag!("spine", "{agent} has no adapter for {short}; nothing to tail");
        // Ready, so a waiting GET answers at once; not live, so the phone
        // knows to keep using the conversation poll.
        spine.set_flags(&session_id, Some(false), Some(true));
        return;
    };
    spine.set_flags(&session_id, Some(super::is_native(&agent)), None);

    // A tail that ran before and was reaped left history a phone may still
    // hold; bootstrapping again would append every user message twice. Say
    // the history was rebuilt and let it start over.
    if spine.has_events(&session_id) {
        spine.push(&session_id, &agent, now_ms(), Kind::Reset);
    }

    let (mut adapter, history) = step(adapter, |a| a.bootstrap()).await;
    let count = history.len();
    for (ts, kind) in history {
        spine.push(&session_id, &agent, ts, kind);
    }
    spine.set_flags(&session_id, None, Some(true));
    crate::diag!("spine", "tail up for {agent} {short}: {count} events from history");

    // Kept alive alongside the watcher so `fs.recv()` pends forever rather
    // than resolving immediately when there is nothing to watch.
    let (tx, mut fs) = mpsc::unbounded_channel::<()>();
    let _keepalive = tx.clone();
    let _watcher = spawn_watch(&adapter.watch_paths(), tx);

    let mut tick = tokio::time::interval(TICK);
    tick.tick().await; // the first tick is immediate; skip it
    let mut reap = tokio::time::interval(REAP_EVERY);
    reap.tick().await;

    loop {
        tokio::select! {
            _ = fs.recv() => {
                // Fold the rest of the burst into this one poll.
                while tokio::time::timeout(COALESCE, fs.recv()).await.is_ok() {}
            }
            _ = tick.tick() => {}
            _ = reap.tick() => {
                if !spine.still_wanted(&app, &session_id) {
                    break;
                }
                continue;
            }
        }
        let (a, events) = step(adapter, |a| a.poll()).await;
        adapter = a;
        for (ts, kind) in events {
            // A Reset from the adapter is pushed like anything else; the
            // ring drops what came before it and the phone re-fetches.
            spine.push(&session_id, &agent, ts, kind);
        }
    }
    crate::diag!("spine", "tail stopped for {short}: no tab bound and no interest");
}

/// Run one adapter call on the blocking pool. Adapters are synchronous and
/// read files, so none of it belongs on an async worker; the adapter is
/// moved in and handed back because it is `Send` but not `Sync`.
async fn step<F>(mut adapter: Box<dyn Adapter>, f: F) -> (Box<dyn Adapter>, Vec<(u64, Kind)>)
where
    F: FnOnce(&mut Box<dyn Adapter>) -> Vec<(u64, Kind)> + Send + 'static,
{
    crate::run_blocking(move || {
        let out = f(&mut adapter);
        (adapter, out)
    })
    .await
}

/// Watch the directories holding `paths` and ping `tx` when one of those
/// files moves. Directories rather than the files themselves because a
/// transcript is often replaced rather than appended (a `/clear` writes a
/// new file), and an inotify watch on the old inode would go quiet.
fn spawn_watch(paths: &[PathBuf], tx: UnboundedSender<()>) -> Option<RecommendedWatcher> {
    if paths.is_empty() {
        return None;
    }
    let wanted: HashSet<PathBuf> = paths.iter().cloned().collect();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if ev.paths.iter().any(|p| wanted.contains(p)) {
                let _ = tx.send(());
            }
        }
    })
    .ok()?;
    let dirs: HashSet<PathBuf> =
        paths.iter().filter_map(|p| p.parent().map(PathBuf::from)).collect();
    let mut armed = false;
    for dir in dirs {
        if watcher.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
            armed = true;
        }
    }
    armed.then_some(watcher)
}

/// Session ids are uuids; the first eight characters identify one in a log
/// line without making the line unreadable.
fn short(session_id: &str) -> &str {
    &session_id[..8.min(session_id.len())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spine::{ToolCategory, ToolStatus};

    fn text(id: &str, body: &str) -> Kind {
        Kind::AgentText { id: id.into(), text: body.into(), done: true }
    }

    #[test]
    fn seq_starts_at_one_and_after_returns_only_what_follows() {
        let spine = Spine::new();
        for i in 0..5 {
            spine.push("s1", "claude", 100 + i, text(&format!("b{i}"), "hi"));
        }
        let all = spine.after("s1", 0);
        assert_eq!(all.len(), 5);
        assert_eq!(all.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        assert!(all.iter().all(|e| e.epoch == spine.epoch()));
        assert_eq!(all[0].ts, 100);

        let tail = spine.after("s1", 3);
        assert_eq!(tail.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![4, 5]);
        assert!(spine.after("s1", 5).is_empty());
        // Sessions do not share a sequence.
        assert_eq!(spine.push("s2", "codex", 1, text("x", "y")).seq, 1);
        assert!(spine.after("nobody", 0).is_empty());
    }

    #[test]
    fn the_ring_drops_the_oldest_past_the_event_bound() {
        let spine = Spine::new();
        for i in 0..(MAX_EVENTS + 20) {
            spine.push("s", "claude", 0, text(&format!("b{i}"), "x"));
        }
        let held = spine.after("s", 0);
        assert_eq!(held.len(), MAX_EVENTS);
        // Seq keeps counting; only the storage is bounded.
        assert_eq!(held.first().unwrap().seq, 21);
        assert_eq!(held.last().unwrap().seq, (MAX_EVENTS + 20) as u64);
    }

    #[test]
    fn the_ring_drops_the_oldest_past_the_byte_bound() {
        let spine = Spine::new();
        let big = "x".repeat(512 * 1024);
        for i in 0..12 {
            spine.push("s", "claude", 0, text(&format!("b{i}"), &big));
        }
        let held = spine.after("s", 0);
        assert!(held.len() < 12, "byte bound never fired: {} events held", held.len());
        assert!(held.iter().map(weight).sum::<usize>() <= MAX_BYTES + big.len());
        assert_eq!(held.last().unwrap().seq, 12);
    }

    #[test]
    fn a_reset_clears_the_history_before_it() {
        let spine = Spine::new();
        spine.push("s", "codex", 1, text("a", "one"));
        spine.push("s", "codex", 2, text("b", "two"));
        spine.push("s", "codex", 3, Kind::Reset);
        spine.push("s", "codex", 4, text("a", "one again"));
        let held = spine.after("s", 0);
        // The reset survives — a phone catching up on `after=1` has to see
        // it to know to drop what it holds.
        assert_eq!(held.len(), 2);
        assert_eq!(held[0].kind, Kind::Reset);
        assert_eq!(held[0].seq, 3);
    }

    #[test]
    fn subscribers_see_every_push_in_order() {
        let spine = Spine::new();
        let mut rx = spine.subscribe();
        spine.push("s", "grok", 7, text("a", "one"));
        spine.push(
            "s",
            "grok",
            8,
            Kind::ToolCall {
                id: "t1".into(),
                tool: "Bash".into(),
                title: "Bash".into(),
                category: ToolCategory::Execute,
                input: "ls".into(),
                status: ToolStatus::Completed,
            },
        );
        assert_eq!(rx.try_recv().unwrap().seq, 1);
        let second = rx.try_recv().unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.agent, "grok");
    }
}

