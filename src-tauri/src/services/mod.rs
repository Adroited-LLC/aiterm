//! Transport-independent application operations.
//!
//! Tauri commands and the remote gateway both call services from this module.
//! The concrete session and agent services arrive with their remote handlers;
//! keeping the boundary explicit now prevents either transport from wrapping
//! the other later.

pub mod agents;
pub mod sessions;
