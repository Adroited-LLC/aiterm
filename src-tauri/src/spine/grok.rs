//! Grok adapter: `~/.grok/sessions/<cwd>/<id>/updates.jsonl` (an on-disk ACP
//! `session/update` stream) plus `events.jsonl` for turns and permissions.
//!
//! Owned by the grok-adapter task. See `docs/spine.md`.

use super::{Adapter, Kind};
use std::path::PathBuf;

pub struct GrokAdapter {
    session_id: String,
}

/// The adapter for a Grok session, or `None` when no session dir exists.
pub fn open(session_id: &str) -> Option<GrokAdapter> {
    let _ = session_id;
    None
}

impl Adapter for GrokAdapter {
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
