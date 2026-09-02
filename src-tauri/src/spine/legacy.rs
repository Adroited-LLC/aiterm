//! Legacy adapter: any engine without a native feed yet. Re-derives the
//! conversation from `detail::conversation_rich` and diffs it into events.
//!
//! Owned by the spine-core task. See `docs/spine.md`.

use super::{Adapter, Kind};
use std::path::PathBuf;

pub struct LegacyAdapter {
    agent: String,
    session_id: String,
}

pub fn open(agent: &str, session_id: &str) -> Option<LegacyAdapter> {
    Some(LegacyAdapter { agent: agent.to_string(), session_id: session_id.to_string() })
}

impl Adapter for LegacyAdapter {
    fn bootstrap(&mut self) -> Vec<(u64, Kind)> {
        let _ = (&self.agent, &self.session_id);
        vec![]
    }
    fn poll(&mut self) -> Vec<(u64, Kind)> {
        vec![]
    }
    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![]
    }
}
