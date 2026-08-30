//! Transport-independent application operations.
//!
//! Tauri commands and the remote gateway both call services from this module.
//! The concrete session and agent services arrive with their remote handlers;
//! keeping the boundary explicit now prevents either transport from wrapping
//! the other later.

pub mod agents;
pub mod sessions;

/// One long-lived transport-independent service graph. Tauri commands and the
/// remote gateway receive clones of this value, which preserve the same Arc-
/// backed catalogs/resolvers rather than reconstructing desktop services per
/// adapter call.
#[derive(Clone)]
pub struct ApplicationServices {
    pub sessions: sessions::SessionService,
    pub agents: agents::AgentService,
}

impl Default for ApplicationServices {
    fn default() -> Self {
        Self::desktop()
    }
}

impl ApplicationServices {
    pub fn desktop() -> Self {
        static SERVICES: std::sync::OnceLock<ApplicationServices> = std::sync::OnceLock::new();
        SERVICES
            .get_or_init(|| Self {
                sessions: sessions::SessionService::desktop(),
                agents: agents::AgentService::desktop(),
            })
            .clone()
    }
}
