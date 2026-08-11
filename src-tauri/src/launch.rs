//! Which engine answers an intent, and what it should be told to run.
//!
//! The frontend holds intent — "start this model here", "reopen this session"
//! — and until now it also held the answer: `App.tsx` decided OpenCode versus
//! `aiterm chat` from a boolean pair, concatenated claude's flags by hand, and
//! minted session ids in the renderer. That is engine knowledge in the one
//! place that should not have any, and none of it was testable.
//!
//! This module is the single place that decision lives. It takes a
//! [`LaunchRequest`] and returns a [`LaunchPlan`]: a command, whether a session
//! id is real, and what the resulting tab supports. The caller never learns
//! which backend answered.

use serde::{Deserialize, Serialize};

use crate::agents::{AgentBackend, Caps, LaunchSpec};
use crate::providers::Provider;

/// What the user asked for, in the terms the UI actually has.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LaunchRequest {
    /// A named engine from the ＋ menu, with whatever the picker chose.
    Agent {
        agent_id: String,
        model: Option<String>,
        effort: Option<String>,
    },
    /// A model from Model access. Which engine runs it is this module's answer,
    /// not the caller's.
    ApiModel {
        provider_id: String,
        model_id: String,
    },
    /// Reopen an existing session where it left off.
    Resume { session_id: String },
    /// The ended-tab restart. Resolves like a resume today; kept separate
    /// because the two diverge the moment an engine wants a different command
    /// for "continue" than for "start again".
    Restart { session_id: String },
    /// Start a fresh conversation under an id aiterm already knows.
    Clear { session_id: String },
}

/// Everything a tab needs to open, and nothing about who produced it.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct LaunchPlan {
    pub command: String,
    /// Provider id whose key `pty_spawn` injects into the tab environment.
    /// `None` means no key is needed — an engine that reads the provider store
    /// itself must never be handed one.
    pub env_provider: Option<String>,
    /// `Some` = a real session id panels may key to. `None` = tab handle only.
    pub session_id: Option<String>,
    pub agent_id: String,
    pub caps: Caps,
}

/// Resolve against the real registry and the saved providers.
pub fn resolve(request: LaunchRequest) -> Option<LaunchPlan> {
    resolve_in(
        &crate::agents::backends(),
        &crate::providers::load_providers(),
        request,
    )
}

/// The body of [`resolve`], over an explicit registry.
///
/// Split out for the same reason `scan_backends` is: through the real registry
/// the answers depend on which agents happen to be installed, so the routing
/// rules — the part worth pinning down — would be untested on every machine
/// and vacuously green on some.
pub fn resolve_in(
    list: &[Box<dyn AgentBackend>],
    providers: &[Provider],
    request: LaunchRequest,
) -> Option<LaunchPlan> {
    match request {
        LaunchRequest::Agent { agent_id, model, effort } => {
            let backend = list.iter().find(|b| b.id() == agent_id)?;
            let spec = LaunchSpec {
                model,
                effort,
                session_id: mint_for(&**backend),
                provider: None,
            };
            Some(LaunchPlan {
                command: backend.launch(&spec),
                env_provider: None,
                session_id: spec.session_id.clone(),
                agent_id: backend.id().to_string(),
                caps: backend.caps(),
            })
        }

        LaunchRequest::ApiModel { provider_id, model_id } => {
            let provider = providers.iter().find(|p| p.id == provider_id)?;
            // First willing engine in registry order. `ChatBackend` is last and
            // accepts everything, so this only fails if the registry is empty.
            // Willingness is asked first because it is free; detection may
            // spawn a login shell per backend, and an engine that would decline
            // anyway is not worth probing for.
            let backend = list
                .iter()
                .find(|b| b.accepts_api(provider) && b.detect().available)?;
            let spec = LaunchSpec {
                model: Some(backend.api_model_slug(&model_id)),
                effort: None,
                session_id: mint_for(&**backend),
                provider: Some(provider_id.clone()),
            };
            Some(LaunchPlan {
                command: backend.launch(&spec),
                // The security property the provider design rests on: a key
                // reaches a tab's environment only for an engine that has said
                // it has no other way to authenticate.
                env_provider: backend.needs_provider_key_env().then_some(provider_id),
                session_id: spec.session_id.clone(),
                agent_id: backend.id().to_string(),
                caps: backend.caps(),
            })
        }

        // Restart is a resume: the tab ended, and continuing the conversation
        // is what "start it again" means for every engine here today.
        LaunchRequest::Resume { session_id } | LaunchRequest::Restart { session_id } => {
            reopen(list, &session_id, |b, id| b.resume(id))
        }

        // Clear is not a reopen, and the difference is the whole feature: the
        // id asked about names the conversation being *parked*, never the one
        // being started. Handing it back would relaunch the engine onto a
        // transcript that already exists and key the new tab to the very row
        // the sidebar was told to keep — the old conversation would be
        // un-parked by the thing that parked it.
        LaunchRequest::Clear { session_id } => {
            let backend = owner(list, &session_id)?;
            let fresh = mint_for(backend)?;
            Some(LaunchPlan {
                command: backend.clear(&fresh)?,
                env_provider: None,
                session_id: Some(fresh),
                agent_id: backend.id().to_string(),
                caps: backend.caps(),
            })
        }
    }
}

/// The backend that owns a session, decided by asking each one to find the
/// transcript rather than by inspecting the id: ids are opaque, and a rule for
/// telling one engine's from another's would be a guess that breaks the first
/// time a format changes.
///
/// The lookup itself lives in `agents.rs` beside the registry, so the resolver,
/// the preview panel and `session_delete` cannot come to different answers
/// about who owns a row.
fn owner<'a>(list: &'a [Box<dyn AgentBackend>], session_id: &str) -> Option<&'a dyn AgentBackend> {
    crate::agents::owner_in(list, session_id).map(|(b, _)| b)
}

/// A plan built by the backend that owns `session_id`, reopening it under that
/// same id.
///
/// `None` when nobody owns the id or the owner declines the operation. The
/// frontend keeps its own fallback — the command the tab was opened with — for
/// exactly that case, so a declining backend costs a feature rather than the
/// action.
fn reopen(
    list: &[Box<dyn AgentBackend>],
    session_id: &str,
    build: impl Fn(&dyn AgentBackend, &str) -> Option<String>,
) -> Option<LaunchPlan> {
    let backend = owner(list, session_id)?;
    Some(LaunchPlan {
        command: build(backend, session_id)?,
        env_provider: None,
        session_id: Some(session_id.to_string()),
        agent_id: backend.id().to_string(),
        caps: backend.caps(),
    })
}

/// A session id, but only where one will mean anything.
///
/// Minted here rather than in the renderer because only the backend knows
/// whether the id will ever name a real session — which is what retires the
/// "there is a session id, so this must be claude" inference the panels were
/// built on.
fn mint_for(backend: &dyn AgentBackend) -> Option<String> {
    backend
        .mints_session_id()
        .then(|| uuid::Uuid::new_v4().to_string())
}

/// Why nothing could be started, in terms of what the person did.
///
/// This string is the toast they see, so it names the engine rather than the
/// request: "Codex sessions can't be reopened from aiterm yet" is something to
/// act on, where the id of a session they just clicked is not. A declining
/// owner and a session that is simply gone are different facts and read
/// differently.
fn explain(list: &[Box<dyn AgentBackend>], request: &LaunchRequest) -> String {
    let gone = "That conversation isn't on disk any more.";
    match request {
        LaunchRequest::Agent { agent_id, .. } => {
            format!("{agent_id} isn't installed on this machine.")
        }
        LaunchRequest::ApiModel { model_id, .. } => {
            format!("No engine here can run {model_id}. Check Settings → Model access.")
        }
        LaunchRequest::Resume { session_id } | LaunchRequest::Restart { session_id } => {
            match owner(list, session_id) {
                Some(b) => format!("{} sessions can't be reopened from aiterm yet.", b.display_name()),
                None => gone.into(),
            }
        }
        LaunchRequest::Clear { session_id } => match owner(list, session_id) {
            Some(b) => format!("{} sessions can't be cleared.", b.display_name()),
            None => gone.into(),
        },
    }
}

/// `async` and off the main thread: resolution scans every backend's session
/// store and probes PATH for the API case, and a plain `#[tauri::command]`
/// would do all of that on the GTK main loop.
#[tauri::command]
pub async fn resolve_launch(request: LaunchRequest) -> Result<LaunchPlan, String> {
    crate::run_blocking(move || {
        let list = crate::agents::backends();
        let providers = crate::providers::load_providers();
        // The registry is built once and used for both the answer and, when
        // there is none, the explanation — otherwise the two could disagree
        // about which engines exist.
        resolve_in(&list, &providers, request.clone())
            .ok_or_else(|| explain(&list, &request))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Detection, ModelOption};
    use crate::sessions::{Session, SessionProvider};

    /* ---- fakes, in the shape agents.rs already uses ---------------------- */

    struct FakeSessions {
        ids: Vec<&'static str>,
    }

    impl SessionProvider for FakeSessions {
        fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)> {
            vec![]
        }
        fn find_session_file(&self, session_id: &str) -> Option<std::path::PathBuf> {
            self.ids
                .contains(&session_id)
                .then(|| std::path::PathBuf::from(format!("/fake/{session_id}")))
        }
    }

    struct FakeBackend {
        id: &'static str,
        available: bool,
        mints: bool,
        accepts_openrouter_only: bool,
        accepts_anything: bool,
        needs_key: bool,
        can_resume: bool,
        can_clear: bool,
        prefix_models: bool,
        sessions: FakeSessions,
    }

    impl Default for FakeBackend {
        fn default() -> Self {
            FakeBackend {
                id: "fake",
                available: true,
                mints: false,
                accepts_openrouter_only: false,
                accepts_anything: false,
                needs_key: false,
                can_resume: false,
                can_clear: false,
                prefix_models: false,
                sessions: FakeSessions { ids: vec![] },
            }
        }
    }

    impl AgentBackend for FakeBackend {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &'static str {
            self.id
        }
        fn detect(&self) -> Detection {
            Detection {
                id: self.id.to_string(),
                display_name: self.id.to_string(),
                available: self.available,
                version: None,
                path: None,
                caps: self.caps(),
            }
        }
        fn sessions(&self) -> &dyn SessionProvider {
            &self.sessions
        }
        fn models(&self) -> Vec<ModelOption> {
            vec![]
        }
        fn mints_session_id(&self) -> bool {
            self.mints
        }
        fn caps(&self) -> Caps {
            Caps { resume: self.can_resume, ..Default::default() }
        }
        fn launch(&self, spec: &LaunchSpec) -> String {
            let mut cmd = self.id.to_string();
            if let Some(m) = spec.model.as_deref() {
                cmd.push_str(&format!(" --model {m}"));
            }
            if let Some(e) = spec.effort.as_deref() {
                cmd.push_str(&format!(" --effort {e}"));
            }
            if let Some(p) = spec.provider.as_deref() {
                cmd.push_str(&format!(" --provider {p}"));
            }
            if let Some(id) = spec.session_id.as_deref() {
                cmd.push_str(&format!(" --session-id {id}"));
            }
            cmd
        }
        fn resume(&self, session_id: &str) -> Option<String> {
            self.can_resume
                .then(|| format!("{} --resume {session_id}", self.id))
        }
        fn clear(&self, session_id: &str) -> Option<String> {
            self.can_clear
                .then(|| format!("{} --session-id {session_id}", self.id))
        }
        fn accepts_api(&self, provider: &Provider) -> bool {
            self.accepts_anything || (self.accepts_openrouter_only && provider.is_openrouter())
        }
        fn api_model_slug(&self, model_id: &str) -> String {
            if self.prefix_models {
                format!("openrouter/{model_id}")
            } else {
                model_id.to_string()
            }
        }
        fn needs_provider_key_env(&self) -> bool {
            self.needs_key
        }
    }

    /// Stand-ins for the two engines whose routing is the whole question.
    fn opencode_like(available: bool) -> Box<dyn AgentBackend> {
        Box::new(FakeBackend {
            id: "opencode",
            available,
            accepts_openrouter_only: true,
            needs_key: true,
            prefix_models: true,
            ..Default::default()
        })
    }

    fn chat_like(owns: Vec<&'static str>) -> Box<dyn AgentBackend> {
        Box::new(FakeBackend {
            id: "api",
            mints: true,
            accepts_anything: true,
            can_resume: true,
            sessions: FakeSessions { ids: owns },
            ..Default::default()
        })
    }

    fn claude_like(owns: Vec<&'static str>) -> Box<dyn AgentBackend> {
        Box::new(FakeBackend {
            id: "claude",
            mints: true,
            can_resume: true,
            sessions: FakeSessions { ids: owns },
            ..Default::default()
        })
    }

    fn provider(id: &str, base_url: &str) -> Provider {
        Provider {
            id: id.into(),
            name: id.into(),
            base_url: base_url.into(),
            api_key: "k".into(),
            startup_models: vec![],
            policy: Default::default(),
            routes: Default::default(),
        }
    }

    fn openrouter() -> Provider {
        provider("openrouter", "https://openrouter.ai/api/v1")
    }

    fn local() -> Provider {
        provider("local", "http://localhost:8080/v1")
    }

    /* ---- routing -------------------------------------------------------- */

    #[test]
    fn an_agent_request_reaches_the_named_backend() {
        let list = vec![claude_like(vec![]), chat_like(vec![])];
        let plan = resolve_in(
            &list,
            &[],
            LaunchRequest::Agent {
                agent_id: "claude".into(),
                model: Some("opus".into()),
                effort: Some("high".into()),
            },
        )
        .expect("no plan");
        assert_eq!(plan.agent_id, "claude");
        assert!(plan.command.starts_with("claude --model opus --effort high"), "{}", plan.command);
        assert_eq!(plan.env_provider, None, "a plain agent must never be handed a key");
    }

    #[test]
    fn an_unknown_agent_yields_no_plan() {
        let list = vec![claude_like(vec![])];
        assert!(resolve_in(
            &list,
            &[],
            LaunchRequest::Agent { agent_id: "ghost".into(), model: None, effort: None },
        )
        .is_none());
    }

    /// The decision that used to be `choice.api.openrouter && opencodeAvail`.
    #[test]
    fn an_openrouter_model_goes_to_opencode_when_it_is_installed() {
        let list = vec![opencode_like(true), chat_like(vec![])];
        let plan = resolve_in(
            &list,
            &[openrouter()],
            LaunchRequest::ApiModel {
                provider_id: "openrouter".into(),
                model_id: "anthropic/claude-sonnet-5".into(),
            },
        )
        .expect("no plan");
        assert_eq!(plan.agent_id, "opencode");
        // The catalog prefix is the engine's spelling, applied by the engine.
        assert!(
            plan.command.contains("--model openrouter/anthropic/claude-sonnet-5"),
            "{}",
            plan.command
        );
    }

    #[test]
    fn an_openrouter_model_falls_back_to_chat_when_opencode_is_absent() {
        let list = vec![opencode_like(false), chat_like(vec![])];
        let plan = resolve_in(
            &list,
            &[openrouter()],
            LaunchRequest::ApiModel {
                provider_id: "openrouter".into(),
                model_id: "anthropic/claude-sonnet-5".into(),
            },
        )
        .expect("no plan");
        assert_eq!(plan.agent_id, "api");
        // No prefix: that spelling belongs to OpenCode's catalog, not to ours.
        assert!(plan.command.contains("--model anthropic/claude-sonnet-5"), "{}", plan.command);
    }

    /// A custom OpenAI-compatible endpoint is not in OpenCode's catalog, so it
    /// must go to the harness even with OpenCode installed and willing-looking.
    #[test]
    fn a_non_openrouter_provider_always_goes_to_chat() {
        let list = vec![opencode_like(true), chat_like(vec![])];
        let plan = resolve_in(
            &list,
            &[local()],
            LaunchRequest::ApiModel {
                provider_id: "local".into(),
                model_id: "some-local-model".into(),
            },
        )
        .expect("no plan");
        assert_eq!(plan.agent_id, "api");
    }

    #[test]
    fn an_unknown_provider_yields_no_plan() {
        let list = vec![chat_like(vec![])];
        assert!(resolve_in(
            &list,
            &[openrouter()],
            LaunchRequest::ApiModel {
                provider_id: "deleted".into(),
                model_id: "m".into(),
            },
        )
        .is_none());
    }

    /* ---- the key ------------------------------------------------------- */

    /// The security property: only the engine that authenticates from its
    /// environment gets a provider named on its plan. The harness reads the
    /// store itself and must never be handed one.
    #[test]
    fn only_an_engine_that_needs_the_key_is_given_a_provider() {
        let with_opencode = vec![opencode_like(true), chat_like(vec![])];
        let plan = resolve_in(
            &with_opencode,
            &[openrouter()],
            LaunchRequest::ApiModel {
                provider_id: "openrouter".into(),
                model_id: "m".into(),
            },
        )
        .unwrap();
        assert_eq!(plan.env_provider.as_deref(), Some("openrouter"));

        let without = vec![opencode_like(false), chat_like(vec![])];
        let plan = resolve_in(
            &without,
            &[openrouter()],
            LaunchRequest::ApiModel {
                provider_id: "openrouter".into(),
                model_id: "m".into(),
            },
        )
        .unwrap();
        assert_eq!(plan.env_provider, None, "the chat harness was handed a key");
    }

    /* ---- minting -------------------------------------------------------- */

    #[test]
    fn a_session_id_is_minted_only_where_it_will_mean_something() {
        let list = vec![claude_like(vec![]), opencode_like(true), chat_like(vec![])];
        let minting = resolve_in(
            &list,
            &[],
            LaunchRequest::Agent { agent_id: "claude".into(), model: None, effort: None },
        )
        .unwrap();
        let id = minting.session_id.expect("claude mints an id");
        assert!(minting.command.contains(&id), "the minted id never reached the command line");

        let not_minting = resolve_in(
            &list,
            &[openrouter()],
            LaunchRequest::ApiModel {
                provider_id: "openrouter".into(),
                model_id: "m".into(),
            },
        )
        .unwrap();
        assert_eq!(not_minting.agent_id, "opencode");
        assert_eq!(not_minting.session_id, None, "an id was minted for an engine that drops it");
    }

    #[test]
    fn two_launches_do_not_share_a_session_id() {
        let list = vec![claude_like(vec![])];
        let req = || LaunchRequest::Agent {
            agent_id: "claude".into(),
            model: None,
            effort: None,
        };
        let a = resolve_in(&list, &[], req()).unwrap().session_id;
        let b = resolve_in(&list, &[], req()).unwrap().session_id;
        assert!(a.is_some() && a != b, "the same id twice: {a:?}");
    }

    /* ---- reopening ------------------------------------------------------ */

    #[test]
    fn resume_routes_to_the_backend_that_owns_the_session() {
        let list = vec![claude_like(vec!["c1"]), chat_like(vec!["chat-1"])];
        let plan = resolve_in(
            &list,
            &[],
            LaunchRequest::Resume { session_id: "chat-1".into() },
        )
        .expect("no plan");
        assert_eq!(plan.agent_id, "api");
        assert_eq!(plan.command, "api --resume chat-1");
        assert_eq!(plan.session_id.as_deref(), Some("chat-1"));
        assert!(plan.caps.resume);
    }

    /// The real harness, not a fake: `▶` on an `api` row has to produce the
    /// command `aiterm chat` actually parses.
    #[test]
    fn resuming_a_chat_produces_the_chat_harness_command() {
        let cmd = crate::agents::ChatBackend.resume("chat-1").expect("no resume command");
        assert!(cmd.contains(" chat --resume 'chat-1'"), "{cmd}");
    }

    #[test]
    fn restart_resolves_the_same_way_as_resume() {
        let list = vec![chat_like(vec!["chat-1"])];
        let resumed = resolve_in(
            &list,
            &[],
            LaunchRequest::Resume { session_id: "chat-1".into() },
        );
        let restarted = resolve_in(
            &list,
            &[],
            LaunchRequest::Restart { session_id: "chat-1".into() },
        );
        assert_eq!(resumed, restarted);
    }

    /// A backend that cannot reopen must say so rather than have the resolver
    /// invent a command for it — the caller then keeps its own fallback.
    #[test]
    fn a_backend_that_cannot_resume_yields_no_plan() {
        let list = vec![Box::new(FakeBackend {
            id: "codex",
            can_resume: false,
            sessions: FakeSessions { ids: vec!["x1"] },
            ..Default::default()
        }) as Box<dyn AgentBackend>];
        assert!(resolve_in(
            &list,
            &[],
            LaunchRequest::Resume { session_id: "x1".into() },
        )
        .is_none());
    }

    #[test]
    fn an_unowned_session_yields_no_plan() {
        let list = vec![claude_like(vec!["c1"]), chat_like(vec!["chat-1"])];
        assert!(resolve_in(
            &list,
            &[],
            LaunchRequest::Resume { session_id: "nobody".into() },
        )
        .is_none());
    }

    /// Clear is claude's alone today; a backend without it declines rather than
    /// re-keying something onto a command it does not understand.
    #[test]
    fn clear_asks_the_owner_and_takes_no_for_an_answer() {
        let list = vec![chat_like(vec!["chat-1"])];
        assert!(resolve_in(
            &list,
            &[],
            LaunchRequest::Clear { session_id: "chat-1".into() },
        )
        .is_none());

        // The real backend: `/clear` re-keys onto an id aiterm chose, which is
        // the same command as a fresh start.
        let cmd = crate::agents::ClaudeBackend.clear("abc-123").expect("no clear command");
        assert!(cmd.contains("--session-id 'abc-123'"), "{cmd}");
    }

    /// The id asked about names the conversation being parked; the plan must
    /// name a different one. An earlier version handed the same id back, which
    /// would have relaunched the engine onto an existing transcript and keyed
    /// the new tab to the row it had just parked — un-parking it.
    #[test]
    fn a_clear_starts_a_conversation_that_is_not_the_one_it_parked() {
        let list: Vec<Box<dyn AgentBackend>> = vec![Box::new(FakeBackend {
            id: "claude-like",
            mints: true,
            can_clear: true,
            sessions: FakeSessions { ids: vec!["old-1"] },
            ..Default::default()
        })];

        let plan = resolve_in(&list, &[], LaunchRequest::Clear { session_id: "old-1".into() })
            .expect("no plan");

        let fresh = plan.session_id.expect("a clear names its new conversation");
        assert_ne!(fresh, "old-1");
        assert!(plan.command.contains(&fresh), "{}", plan.command);
        assert!(!plan.command.contains("old-1"), "{}", plan.command);
    }

    /// The refusal is the toast a person reads, so it has to name the engine
    /// that refused — not the request. "resume of session <uuid>" told them
    /// nothing they did not already know and leaked an id at them.
    #[test]
    fn a_refusal_names_the_engine_that_refused() {
        let list: Vec<Box<dyn AgentBackend>> = vec![Box::new(FakeBackend {
            id: "codex-like",
            can_resume: false,
            sessions: FakeSessions { ids: vec!["c-1"] },
            ..Default::default()
        })];

        let refused = explain(&list, &LaunchRequest::Resume { session_id: "c-1".into() });
        assert!(refused.contains("codex-like"), "{refused}");
        assert!(!refused.contains("c-1"), "the id is not news to whoever clicked it: {refused}");

        // Nobody owns it: a different fact, and it reads differently.
        let gone = explain(&list, &LaunchRequest::Resume { session_id: "never".into() });
        assert!(gone.contains("isn't on disk"), "{gone}");
    }

    /// A backend that names no session cannot clear: there would be nothing for
    /// the new tab to be keyed to.
    #[test]
    fn an_engine_that_mints_nothing_cannot_clear() {
        let list: Vec<Box<dyn AgentBackend>> = vec![Box::new(FakeBackend {
            id: "no-mint",
            mints: false,
            can_clear: true,
            sessions: FakeSessions { ids: vec!["old-1"] },
            ..Default::default()
        })];

        assert!(
            resolve_in(&list, &[], LaunchRequest::Clear { session_id: "old-1".into() }).is_none()
        );
    }

    /* ---- the wire ------------------------------------------------------- */

    /// The frontend sends camelCase. Asserted rather than assumed: container
    /// `rename_all` renames variants, not their fields, so the field casing
    /// comes from a separate attribute and would fail silently at runtime.
    #[test]
    fn every_variant_deserializes_from_the_camel_case_the_ui_sends() {
        let agent: LaunchRequest = serde_json::from_str(
            r#"{"kind":"agent","agentId":"claude","model":"opus","effort":"high"}"#,
        )
        .expect("agent");
        assert!(
            matches!(agent, LaunchRequest::Agent { agent_id, model, effort }
                if agent_id == "claude" && model.as_deref() == Some("opus")
                    && effort.as_deref() == Some("high")),
        );

        let api: LaunchRequest = serde_json::from_str(
            r#"{"kind":"apiModel","providerId":"openrouter","modelId":"a/b"}"#,
        )
        .expect("apiModel");
        assert!(matches!(api, LaunchRequest::ApiModel { provider_id, model_id }
            if provider_id == "openrouter" && model_id == "a/b"));

        for (json, ok) in [
            (r#"{"kind":"resume","sessionId":"s1"}"#, "resume"),
            (r#"{"kind":"restart","sessionId":"s1"}"#, "restart"),
            (r#"{"kind":"clear","sessionId":"s1"}"#, "clear"),
        ] {
            let parsed: LaunchRequest = serde_json::from_str(json).expect(ok);
            let id = match parsed {
                LaunchRequest::Resume { session_id }
                | LaunchRequest::Restart { session_id }
                | LaunchRequest::Clear { session_id } => session_id,
                other => panic!("{json} parsed as {other:?}"),
            };
            assert_eq!(id, "s1");
        }
    }

    /// Optional fields are optional: the ＋ menu's "agent default" sends
    /// neither a model nor an effort.
    #[test]
    fn an_agent_request_may_name_no_model_at_all() {
        let parsed: LaunchRequest =
            serde_json::from_str(r#"{"kind":"agent","agentId":"claude"}"#).expect("agent");
        assert!(matches!(parsed, LaunchRequest::Agent { model: None, effort: None, .. }));
    }

    /// `caps` is what the UI gates on, so it has to survive serialization in
    /// the snake_case the rest of the payload uses.
    #[test]
    fn a_plan_serializes_the_fields_the_ui_reads() {
        let list = vec![claude_like(vec![])];
        let plan = resolve_in(
            &list,
            &[],
            LaunchRequest::Agent { agent_id: "claude".into(), model: None, effort: None },
        )
        .unwrap();
        let v: serde_json::Value = serde_json::to_value(&plan).unwrap();
        assert!(v.get("command").is_some() && v.get("env_provider").is_some());
        assert_eq!(v.pointer("/caps/tui_drive"), Some(&serde_json::Value::Bool(false)));
        assert_eq!(v.pointer("/caps/resume"), Some(&serde_json::Value::Bool(true)));
    }

    /* ---- the real registry ---------------------------------------------- */

    /// Order is the tie-break for both routing and lookup, so the fallback
    /// must sit last: anything after a backend that accepts everything would
    /// never be reached.
    #[test]
    fn the_all_accepting_fallback_is_last_in_the_registry() {
        let list = crate::agents::backends();
        let first = list.iter().position(|b| b.accepts_api(&local()));
        assert_eq!(first, Some(list.len() - 1), "a backend sits behind the fallback");
    }

    /// The chat harness is always available, so an API model resolves on a
    /// machine with no agents installed at all.
    #[test]
    fn an_api_model_always_resolves_through_the_real_registry() {
        let plan = resolve_in(
            &crate::agents::backends(),
            &[local()],
            LaunchRequest::ApiModel { provider_id: "local".into(), model_id: "m".into() },
        )
        .expect("no engine would run an API model");
        assert_eq!(plan.agent_id, "api");
        assert_eq!(plan.env_provider, None);
        assert!(plan.session_id.is_some(), "a chat's id is real from the first frame");
    }
}
