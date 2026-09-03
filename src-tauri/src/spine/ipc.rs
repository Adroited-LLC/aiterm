//! The spine, read by the desktop's own renderer.
//!
//! The phone reads the spine as a stream: `GET …/spine?after=N`, apply every
//! event, keep the whole conversation in memory. The home screen wants the
//! opposite shape — not one session in full but every session in one line
//! each, so a person arriving can see which of eight running agents is stuck
//! on a permission prompt without opening any of them.
//!
//! So this is a *snapshot*, not a feed: what each session's log already knows,
//! folded down to the handful of fields a board row draws. It reads only the
//! registry's in-memory ring — no transcript, no `stat`, no process walk — and
//! it never starts a tail. A session the spine is not following simply is not
//! in the answer, and the renderer falls back to the sessions list for it.

use std::sync::Arc;

use serde::Serialize;

use super::Spine;

/// The tool card a session is on, reduced to what a one-line row can show.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverviewTool {
    /// The adapter's human title for the call — "Edit foo.rs", "Bash".
    pub title: String,
    /// `pending` | `running` | `completed` | `failed` | `cancelled`.
    pub status: String,
}

/// One session, as the fleet board draws it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionOverview {
    pub session_id: String,
    pub agent: String,
    /// `working` | `needs_you` | `idle` — the last phase the registry pushed,
    /// which is the same verdict the phone and the sessions list see.
    pub phase: String,
    /// The phase's human half: "running Bash", "permission: Edit foo.rs".
    pub detail: String,
    /// Whether a turn is open right now. False also when no adapter has ever
    /// reported a turn boundary — the legacy one never does.
    pub turn_open: bool,
    /// When the open turn started, in ms. `None` when no turn is open, so a
    /// row's elapsed timer has nothing to count from and shows nothing.
    pub turn_started_ts: Option<u64>,
    /// The last line of the most recent assistant block, clipped. The last
    /// LINE rather than the head of the block: a block is a snapshot that
    /// grows, and its first sentence is what the session was saying a minute
    /// ago while its last is what it is saying now.
    pub last_text: Option<String>,
    pub last_tool: Option<OverviewTool>,
}

/// Every session the spine holds a log for, one row each.
///
/// Cheap by construction: one lock on the registry map, and per session a
/// backwards walk of its ring that stops as soon as it has found the last
/// text and the last tool call — in a live session, within a handful of
/// events. Nothing here touches the disk, so the renderer's 2 s poll while
/// the home screen is up costs about what a `Vec` clone costs.
#[tauri::command]
pub fn spine_overview(spine: tauri::State<'_, Arc<Spine>>) -> Vec<SessionOverview> {
    spine.overview()
}
