//! Claude Code adapter: the transcript at `~/.claude/projects/<proj>/<id>.jsonl`,
//! read from a byte offset, one line per content block.
//!
//! Owned by the claude-adapter task. See `docs/spine.md`.

use super::{Adapter, Kind};
use std::path::PathBuf;

pub struct ClaudeAdapter {
    session_id: String,
}

/// The adapter for a Claude session, or `None` when no transcript exists
/// for that id.
pub fn open(session_id: &str) -> Option<ClaudeAdapter> {
    let _ = session_id;
    None
}

impl Adapter for ClaudeAdapter {
    fn bootstrap(&mut self) -> Vec<(u64, Kind)> {
        let _ = &self.session_id;
        vec![]
    }
    fn poll(&mut self) -> Vec<(u64, Kind)> {
        vec![]
    }
    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![]
    }
}
