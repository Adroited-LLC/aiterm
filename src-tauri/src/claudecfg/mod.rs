//! Everything Claude Code reads that decides how a session behaves, gathered
//! for display. Read-only by design: these are files every session on the
//! machine depends on, and Phase 1 shows them without touching them.

pub mod settings;
pub mod concern;
pub mod instructions;
pub mod mcp;
pub mod skills;
