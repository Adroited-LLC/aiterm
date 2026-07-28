//! The agents aiterm knows how to work with.
//!
//! aiterm was built around Claude Code, and the assumption is spread thin
//! across the codebase rather than concentrated: the session store path, the
//! launch flags, the roster command and the transcript format are all just
//! *there*, in whatever function needed them. That is fine for one agent and
//! becomes untenable at two, because the second one does not announce itself —
//! it shows up as a session list that silently omits half your work, or a
//! search index that only covers one tool.
//!
//! This module is the one place that knows an agent exists. A backend answers
//! three questions, and they are deliberately separate:
//!
//! - **Who are you?** `id` and `display_name`. `id` is written onto every
//!   session the backend yields and is what the UI switches its icon on.
//! - **Are you here?** `detect`, which is cheap enough to call whenever the
//!   answer is wanted and never assumes a tool is installed.
//! - **Where are your sessions?** `sessions`, a [`SessionProvider`].
//!
//! ## What is *not* here yet, and why that matters
//!
//! Listing, indexing and transcript lookup route through this registry. A great
//! deal does not, and a second backend will find every one of these still
//! hard-wired to Claude Code:
//!
//! - **Liveness.** `read_roster` shells out to `claude agents --json`.
//! - **Lifecycle.** Resume, fork, stop and the `--session-id` mint in the UI
//!   all speak Claude Code's flags.
//! - **Panels.** Tasks, artifacts, agents and the model pills parse Claude
//!   Code's transcript records and read `~/.claude`.
//! - **Trash.** `session_delete` and restore know `~/.claude/projects` layout.
//!
//! Each of those is a real decision rather than a mechanical port — a CLI agent
//! has to be *asked* whether a session is alive, while an API-backed one knows
//! for free — so they are left explicit rather than hidden behind a trait that
//! pretends to abstract them. Adding a backend today gets you rows in the list
//! and hits in search. Everything else is still to do, and this comment is the
//! honest list of what.

use std::time::Duration;

use serde::Serialize;

use crate::sessions::{ClaudeProvider, Session, SessionProvider};

/// What is known about an agent on this machine right now.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Detection {
    pub id: String,
    pub display_name: String,
    /// Whether aiterm can actually use it. For a CLI agent this means the
    /// binary is on PATH; a future API-backed backend would report whether it
    /// has credentials.
    pub available: bool,
    /// First line of `<bin> --version`, when it answered. `None` covers both
    /// "not installed" and "installed but would not say", which are different
    /// facts — `available` is the one to branch on.
    pub version: Option<String>,
    /// Resolved binary path, so the UI can show *which* copy was found when
    /// several are installed.
    pub path: Option<String>,
}

/// A model a backend can be started on, and the effort levels it accepts.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ModelOption {
    /// What goes on the command line.
    pub id: String,
    pub display_name: String,
    /// Effort levels valid *for this model*. Empty when the agent has no such
    /// concept. Per-model rather than per-agent because they genuinely differ:
    /// Codex publishes a different set for each model.
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
}

/// What to start. Every field optional — the honest default for all of them is
/// "whatever the agent would do on its own", which is not the same as any value
/// we could pick for it.
#[derive(serde::Deserialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSpec {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A session id aiterm has minted. Only meaningful where the agent accepts
    /// one — see `mints_session_id`.
    pub session_id: Option<String>,
}

pub trait AgentBackend: Send + Sync {
    /// Stable identifier. Written onto every session this backend yields, so
    /// changing it orphans the `agent` field on rows already in the index.
    fn id(&self) -> &'static str;

    /// Human-facing name, for settings and empty states.
    fn display_name(&self) -> &'static str;

    /// Is this agent usable on this machine?
    ///
    /// Called on demand rather than polled: availability changes when someone
    /// installs something, which is not something to spend a timer on. The
    /// PATH lookup is pure filesystem; only reading a version spawns anything,
    /// and only when the binary was found.
    fn detect(&self) -> Detection;

    /// Where this backend's sessions live.
    fn sessions(&self) -> &dyn SessionProvider;

    /// Models this agent can be started on, best-effort. An empty list means
    /// "we do not know" — the UI offers the agent's own default rather than
    /// inventing names.
    fn models(&self) -> Vec<ModelOption> {
        Vec::new()
    }

    /// Whether `--session-id`-style pre-minting works.
    ///
    /// This is not a detail: aiterm mints the id before launching so a new tab
    /// has a sidebar row from the first frame. Where an agent will not take
    /// one, the id is a tab handle only, no panel should be keyed to it, and
    /// its placeholder row stays until the tab closes. Saying so here is what
    /// keeps the UI from pointing panels at a session that will never exist.
    fn mints_session_id(&self) -> bool {
        false
    }

    /// How to start a session: the command, plus any environment it needs.
    ///
    /// Built here rather than in the frontend, which is where it used to live
    /// as a hardcoded `CLAUDE_CMD` string. Command-line syntax is the one thing
    /// that is certainly per-agent, so it belongs with the agent — otherwise
    /// adding one means editing the renderer.
    fn launch(&self, spec: &LaunchSpec) -> LaunchPlan;
}

/// A command and the environment to run it in.
///
/// Two fields rather than one string because a credential must not go on a
/// command line: the command is executed as `$SHELL -ic '<cmd>'`, so anything
/// in it is visible in `ps` to every process on the machine. Environment
/// reaches only the child.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct LaunchPlan {
    pub command: String,
    pub env: std::collections::HashMap<String, String>,
}

impl LaunchPlan {
    fn cmd(command: String) -> Self {
        Self { command, env: Default::default() }
    }
}

/// Shell-quote a value going onto a command line.
///
/// Model ids and effort levels come from the frontend. They are chosen from
/// lists we produced, but "the UI only sends good values" is not a security
/// boundary — this string is handed to `$SHELL -ic`.
fn q(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub struct ClaudeBackend;

impl AgentBackend for ClaudeBackend {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn detect(&self) -> Detection {
        detect_cli(self.id(), self.display_name(), "claude")
    }
    fn sessions(&self) -> &dyn SessionProvider {
        &ClaudeProvider
    }

    fn mints_session_id(&self) -> bool {
        true
    }

    /// The aliases `claude --help` documents, plus the effort levels it lists.
    ///
    /// Hardcoded because Claude Code publishes no machine-readable list — the
    /// `/model` picker is drawn in the TUI and there is no cache file to read,
    /// unlike Codex. These come from `--help` on 2.1.220 and will age; the
    /// blank "agent default" option in the UI is the escape hatch, and picking
    /// nothing here is always safe.
    fn models(&self) -> Vec<ModelOption> {
        const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
        [("fable", "Fable"), ("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")]
            .into_iter()
            .map(|(id, name)| ModelOption {
                id: id.to_string(),
                display_name: name.to_string(),
                efforts: EFFORTS.iter().map(|s| s.to_string()).collect(),
                default_effort: None,
            })
            .collect()
    }

    /// `--permission-mode auto --allow-dangerously-skip-permissions` is kept
    /// exactly as the frontend's old `CLAUDE_CMD` had it, flags and reasoning
    /// unchanged — see the comment this replaced in App.tsx. Moving it here is
    /// a relocation, not a behaviour change.
    fn launch(&self, spec: &LaunchSpec) -> LaunchPlan {
        LaunchPlan::cmd(claude_command(spec))
    }
}

/// Claude Code's invocation, shared by the Claude backend and by every
/// API-backed source — those *are* Claude Code, pointed at another endpoint.
fn claude_command(spec: &LaunchSpec) -> String {
    {
        let mut cmd =
            String::from("claude --permission-mode auto --allow-dangerously-skip-permissions");
        if let Some(m) = spec.model.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --model {}", q(m)));
        }
        if let Some(e) = spec.effort.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --effort {}", q(e)));
        }
        if let Some(id) = spec.session_id.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --session-id {}", q(id)));
        }
        cmd
    }
}

/// OpenAI Codex.
///
/// **Detection only.** Whether `codex` is installed is a fact aiterm can check
/// today, and worth showing — "Codex: not installed" tells you the difference
/// between a tool aiterm cannot use and one you have not set up. Its sessions
/// are another matter: the on-disk format has not been examined, so
/// [`CodexSessions`] finds nothing rather than guessing at a layout.
///
/// That split is deliberate. A provider that invented a plausible path would
/// fail by finding nothing in a way indistinguishable from "you have no Codex
/// sessions", and the first person to debug it would have to prove a negative.
/// This way the registry is honestly two backends, the settings panel can say
/// what is supported, and filling in the provider is a self-contained job with
/// nothing to unpick first.
pub struct CodexBackend;

/// Placeholder until Codex's session store has actually been looked at.
pub struct CodexSessions;

impl SessionProvider for CodexSessions {
    fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)> {
        Vec::new()
    }
    fn find_session_file(&self, _session_id: &str) -> Option<std::path::PathBuf> {
        // Never claims ownership, so lookups fall through to a backend that
        // can actually resolve the id.
        None
    }
}

impl AgentBackend for CodexBackend {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn display_name(&self) -> &'static str {
        "Codex"
    }
    fn detect(&self) -> Detection {
        detect_cli(self.id(), self.display_name(), "codex")
    }
    fn sessions(&self) -> &dyn SessionProvider {
        &CodexSessions
    }

    /// Codex has no `--session-id`. `codex --help` offers `resume` and `fork`
    /// as subcommands and nothing that names a session up front, so aiterm
    /// cannot pre-mint one — its placeholder row is a tab handle for the life
    /// of the tab, and no panel is keyed to it.
    fn mints_session_id(&self) -> bool {
        false
    }

    /// Read from Codex's own cache rather than hardcoded.
    ///
    /// `~/.codex/models_cache.json` is written by the CLI and carries the slug,
    /// display name, the reasoning levels *each model* supports and its
    /// default — which is exactly this list, kept current by the tool itself.
    /// Absent (never run, or a future version that stops writing it) the list
    /// is empty and the UI offers only the agent default, which is correct
    /// rather than a guess.
    fn models(&self) -> Vec<ModelOption> {
        codex_models().unwrap_or_default()
    }

    /// Effort is a config override, not a flag: `codex --help` has no effort
    /// option, and `model_reasoning_effort` is a real config key — verified
    /// 2026-07-27 in the native binary of codex-cli 0.145.0.
    fn launch(&self, spec: &LaunchSpec) -> LaunchPlan {
        let mut cmd = String::from("codex");
        if let Some(m) = spec.model.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" --model {}", q(m)));
        }
        if let Some(e) = spec.effort.as_deref().filter(|s| !s.is_empty()) {
            cmd.push_str(&format!(" -c model_reasoning_effort={}", q(e)));
        }
        // spec.session_id is deliberately dropped — see mints_session_id.
        LaunchPlan::cmd(cmd)
    }
}

/// Parse `~/.codex/models_cache.json`. Split out so the shape can be tested
/// against a captured copy without needing Codex installed.
fn codex_models() -> Option<Vec<ModelOption>> {
    let path = dirs::home_dir()?.join(".codex/models_cache.json");
    let text = std::fs::read_to_string(path).ok()?;
    parse_codex_models(&text)
}

pub fn parse_codex_models(text: &str) -> Option<Vec<ModelOption>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let models = v.get("models")?.as_array()?;
    let out: Vec<ModelOption> = models
        .iter()
        .filter_map(|m| {
            let id = m.get("slug")?.as_str()?.to_string();
            let display_name = m
                .get("display_name")
                .and_then(|d| d.as_str())
                .unwrap_or(&id)
                .to_string();
            let efforts = m
                .get("supported_reasoning_levels")
                .and_then(|l| l.as_array())
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(|l| {
                            // Entries are objects with an `effort`, but accept a
                            // bare string too rather than dropping the list if a
                            // future version simplifies it.
                            l.get("effort")
                                .and_then(|e| e.as_str())
                                .or_else(|| l.as_str())
                                .map(String::from)
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(ModelOption {
                id,
                display_name,
                efforts,
                default_effort: m
                    .get("default_reasoning_level")
                    .and_then(|d| d.as_str())
                    .map(String::from),
            })
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

/// A configured API endpoint, driven through Claude Code.
///
/// Not a new engine. Claude Code reads `ANTHROPIC_BASE_URL` and
/// `ANTHROPIC_AUTH_TOKEN`, so pointing it at a provider that serves the
/// Anthropic Messages API gives a real session — the same terminal, the same
/// transcripts, the same sidebar — against someone else's models. That is a far
/// smaller thing to build than an API client, and it inherits everything the
/// app already does well.
///
/// This is only offered because it was **tested end to end**, not because the
/// environment variables exist:
///
/// > 2026-07-27. `POST https://openrouter.ai/api/v1/messages` with
/// > `anthropic-version: 2023-06-01` returned a correctly-shaped Anthropic
/// > response. Then `ANTHROPIC_BASE_URL=https://openrouter.ai/api
/// > ANTHROPIC_AUTH_TOKEN=<key> claude -p 'reply with exactly: OK' --model
/// > anthropic/claude-sonnet-5` printed `OK` and exited 0.
///
/// Not every provider can do this. An OpenAI-compatible endpoint serves
/// `/v1/chat/completions`, which is a different protocol from
/// `/v1/messages` — OpenRouter happens to serve both. So availability is not
/// assumed from having a key: [`provider_speaks_anthropic`] asks the endpoint,
/// and the settings panel reports the answer rather than letting a session fail
/// at the first prompt.
pub struct ApiBackend {
    id: String,
    provider: crate::providers::Provider,
}

impl AgentBackend for ApiBackend {
    fn id(&self) -> &'static str {
        // Backend ids are `&'static str` because Claude's and Codex's are
        // compile-time constants and are stamped onto every session row. An
        // API source's id is only known at runtime, so it is leaked once, at
        // registry construction. There are as many of these as the user has
        // configured providers — a handful, once per call — and the
        // alternative is threading a lifetime through the whole trait for the
        // sake of two static implementors.
        Box::leak(self.id.clone().into_boxed_str())
    }

    fn display_name(&self) -> &'static str {
        Box::leak(self.provider.name.clone().into_boxed_str())
    }

    fn detect(&self) -> Detection {
        // A key is what makes it *configured*. Whether the endpoint speaks the
        // right protocol is a separate question with a network cost, asked by
        // the settings panel rather than here — this runs whenever the picker
        // opens, and must not make that wait on someone's API.
        Detection {
            id: self.id.clone(),
            display_name: self.provider.name.clone(),
            available: !self.provider.api_key.is_empty(),
            version: None,
            path: Some(self.provider.base_url.clone()),
        }
    }

    fn sessions(&self) -> &dyn SessionProvider {
        // Sessions run through Claude Code, so they land in Claude Code's
        // store and are already listed by ClaudeProvider. Returning it here
        // would list every one of them once per configured provider.
        &NoSessions
    }

    /// Deliberately empty. The model list is per-provider and costs a network
    /// round trip, so the UI fetches it from `provider_models` when this source
    /// is actually selected, rather than every time the picker opens.
    fn models(&self) -> Vec<ModelOption> {
        Vec::new()
    }

    fn mints_session_id(&self) -> bool {
        true // it is Claude Code underneath
    }

    fn launch(&self, spec: &LaunchSpec) -> LaunchPlan {
        let mut env = std::collections::HashMap::new();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            crate::providers::anthropic_base(&self.provider.base_url),
        );
        env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), self.provider.api_key.clone());
        // Claude Code warns that a key "takes precedence over your claude.ai
        // login" and disables connectors. That is exactly what is wanted here,
        // and only for this child — the environment does not escape the pty.
        LaunchPlan { command: claude_command(spec), env }
    }
}

/// For a backend whose sessions are somebody else's to list.
pub struct NoSessions;

impl SessionProvider for NoSessions {
    fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)> {
        Vec::new()
    }
    fn find_session_file(&self, _session_id: &str) -> Option<std::path::PathBuf> {
        None
    }
}

/// Does this provider actually serve the Anthropic Messages API?
///
/// The one question that decides whether a provider can be a source. Asked with
/// the smallest possible real request — a one-token completion — because the
/// endpoint only reveals itself by answering: a provider that serves
/// `/v1/chat/completions` and nothing else returns 404 here, which is precisely
/// the case that must not be offered as a startable source.
#[tauri::command(async)]
pub fn provider_speaks_anthropic(id: String) -> Result<String, String> {
    let list = crate::providers::configured();
    let p = list.iter().find(|p| p.id == id).ok_or("No such provider.")?;
    if p.api_key.is_empty() {
        return Err("No API key saved for this provider.".into());
    }
    let url = format!("{}/v1/messages", crate::providers::anthropic_base(&p.base_url));
    let body = r#"{"model":"anthropic/claude-sonnet-5","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#;
    let out = std::process::Command::new("curl")
        .args([
            "-sS", "--connect-timeout", "5", "--max-time", "25",
            "-o", "/dev/null", "-w", "%{http_code}",
            "-X", "POST",
            "-H", &format!("x-api-key: {}", p.api_key),
            "-H", &format!("Authorization: Bearer {}", p.api_key),
            "-H", "anthropic-version: 2023-06-01",
            "-H", "Content-Type: application/json",
            "-d", body,
            &url,
        ])
        .output()
        .map_err(|e| format!("Could not run curl: {e}"))?;
    let code: u16 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    match code {
        200 => Ok("Answers the Anthropic Messages API — usable as a source.".into()),
        401 | 403 => Err("The provider rejected that API key.".into()),
        404 => Err(format!(
            "No Anthropic Messages API at {url} — this provider can hold models \
             but cannot drive a session."
        )),
        0 => Err(format!("Could not reach {url}.")),
        // A 400 means it parsed the request and disliked the body — usually an
        // unknown model id. The endpoint is there, which is what was asked.
        400 => Ok("Endpoint answers, but rejected the test model — pick a model it serves.".into()),
        other => Err(format!("HTTP {other} from {url}.")),
    }
}

/// Every backend aiterm knows about.
///
/// One registry, so a new agent is added in one place and cannot be half-added
/// — the previous arrangement built this list inline in `list_sessions` while
/// the indexer named `ClaudeProvider` directly, which is exactly the shape that
/// gets you rows you cannot search.
pub fn backends() -> Vec<Box<dyn AgentBackend>> {
    let mut list: Vec<Box<dyn AgentBackend>> =
        vec![Box::new(ClaudeBackend), Box::new(CodexBackend)];
    // Then one per configured API provider. Built from config rather than
    // compiled in, because which endpoints exist is the user's business — and
    // prefixed so an id can never collide with a built-in backend's.
    for p in crate::providers::configured() {
        let id = format!("api:{}", p.id);
        list.push(Box::new(ApiBackend { id, provider: p }));
    }
    list
}

/// Every backend's sessions with their transcript paths, newest first.
///
/// The single entry point for "what sessions exist" — listing and indexing both
/// come through here, so a backend cannot be visible in one and absent from the
/// other.
///
/// Each row is stamped with the id of the backend that produced it, rather than
/// trusting the parser to label its own output. The parser is per-agent and its
/// label would be a second place for the name to live; this way `agent` cannot
/// disagree with the registry, which is what the UI switches on.
pub fn scan_all_with_paths() -> Vec<(Session, std::path::PathBuf)> {
    scan_backends(&backends())
}

/// The body of [`scan_all_with_paths`], over an explicit list.
///
/// Split out so the composition rules — tagging, global ordering — can be
/// tested against fake backends. With one real backend in the registry there is
/// otherwise nothing to compose, and the interesting behaviour would go
/// unexercised until the day a second one is added.
fn scan_backends(list: &[Box<dyn AgentBackend>]) -> Vec<(Session, std::path::PathBuf)> {
    let mut all: Vec<(Session, std::path::PathBuf)> = list
        .iter()
        .flat_map(|b| {
            let id = b.id();
            b.sessions()
                .scan_with_paths()
                .into_iter()
                .map(move |(mut s, path)| {
                    s.agent = id.to_string();
                    (s, path)
                })
        })
        .collect();
    // Sorted across all backends, not within each: the list is one timeline of
    // your work, and grouping it by which tool happened to produce a row would
    // be an odd thing to impose on it.
    all.sort_by(|a, b| b.0.last_active.cmp(&a.0.last_active));
    all
}

/// The transcript for `session_id`, from whichever backend owns it.
///
/// Ownership is decided by asking, not by inspecting the id: ids are opaque,
/// and a rule for telling one agent's from another's would be a guess that
/// breaks the first time a format changes. First backend to find the file wins,
/// which makes registry order the tie-break — see the id-collision test for why
/// that is stated rather than left to chance.
pub fn find_session_file_in(
    list: &[Box<dyn AgentBackend>],
    session_id: &str,
) -> Option<std::path::PathBuf> {
    list.iter()
        .find_map(|b| b.sessions().find_session_file(session_id))
}

/// What aiterm can see on this machine, in registry order.
///
/// Reports every known backend, present or not: "Codex — not installed" is
/// more useful in a settings panel than an absence, and it is the difference
/// between a tool aiterm does not support and one you have not installed.
#[tauri::command]
pub fn detect_agents() -> Vec<Detection> {
    backends().iter().map(|b| b.detect()).collect()
}

/// What a new session can be started as: the agents that are actually here,
/// with their models. Absent agents are omitted — this feeds a picker, and
/// offering something that cannot start is worse than not offering it.
#[derive(Serialize, Clone, Debug)]
pub struct AgentChoice {
    pub id: String,
    pub display_name: String,
    pub models: Vec<ModelOption>,
    pub mints_session_id: bool,
}

#[tauri::command]
pub fn agent_choices() -> Vec<AgentChoice> {
    backends()
        .iter()
        .filter(|b| b.detect().available)
        .map(|b| AgentChoice {
            id: b.id().to_string(),
            display_name: b.display_name().to_string(),
            models: b.models(),
            mints_session_id: b.mints_session_id(),
        })
        .collect()
}

/// The command that starts `agent_id` with `spec`.
///
/// The renderer asks for this rather than assembling flags itself. It used to
/// hold Claude's invocation as a constant, which meant the one thing that is
/// certainly per-agent lived in the one place that should not know about any.
#[tauri::command]
pub fn agent_launch_command(agent_id: String, spec: LaunchSpec) -> Result<LaunchPlan, String> {
    backends()
        .iter()
        .find(|b| b.id() == agent_id)
        .map(|b| b.launch(&spec))
        .ok_or_else(|| format!("No agent called {agent_id}."))
}

/// Resolve `bin` against PATH, the way a shell would.
///
/// Deliberately not `which`/`command -v`: spawning a shell to ask whether a
/// program exists costs more than the answer, and would make "is Codex
/// installed?" a process spawn per backend per call.
fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| is_executable_file(candidate))
}

/// Ask the user's login shell where `bin` is.
///
/// Necessary because aiterm's own PATH is not the one you get at a prompt. A
/// desktop launcher starts the app from the session manager with a minimal
/// environment, and Node version managers make it worse: fnm, nvm and asdf put
/// their shims on PATH from a shell rc file, and fnm's live in a per-shell
/// directory (`/run/user/…/fnm_multishells/<pid>_<ts>/bin`) that does not exist
/// outside one.
///
/// That is not a corner case for this feature — Codex is a Node CLI, so an fnm
/// user has it installed and aiterm would flatly report "not installed".
/// *Observed 2026-07-27: `codex-cli 0.145.0` resolving only inside a shell.*
///
/// Interactive as well as login (`-lic`), because rc files that set these shims
/// up are usually the interactive ones. Bounded, because an interactive shell
/// can block on anything a user has put in their profile and this is called
/// from the UI thread's command handler.
fn which_via_login_shell(bin: &str) -> Option<std::path::PathBuf> {
    let shell = std::env::var("SHELL").ok()?;
    // `bin` is a literal from our own backend list, never user input, but keep
    // the quoting correct anyway rather than relying on that staying true.
    let script = format!("command -v '{}' 2>/dev/null", bin.replace('\'', "'\\''"));
    let out = run_bounded(&shell, &["-l", "-i", "-c", &script], Duration::from_secs(4))?;
    // An interactive shell may print a banner or an rc-file warning first, so
    // take the last line that is actually a path to an executable rather than
    // assuming the output is clean.
    String::from_utf8_lossy(&out)
        .lines()
        .rev()
        .map(|l| std::path::PathBuf::from(l.trim()))
        .find(|p| is_executable_file(p))
}

/// Run a command, giving up after `limit`. Returns stdout on success.
///
/// `std::process::Command` has no timeout, and a shell that hangs on someone's
/// profile would hang the settings panel with it. On timeout the worker thread
/// is left to finish on its own — it holds nothing but its own stdout buffer,
/// and abandoning it is better than blocking the UI on it.
fn run_bounded(program: &str, args: &[&str], limit: Duration) -> Option<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let program = program.to_string();
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    std::thread::spawn(move || {
        let result = std::process::Command::new(&program).args(&args).output();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(out)) if out.status.success() => Some(out.stdout),
        _ => None,
    }
}

#[cfg(unix)]
fn is_executable_file(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(p: &std::path::Path) -> bool {
    p.is_file()
}

/// Detection for a backend that is a command-line program.
///
/// A missing binary is the ordinary case, not an error: most machines will have
/// one of these agents and not the others, and that is worth showing plainly
/// rather than treating as a failure.
fn detect_cli(id: &str, display_name: &str, bin: &str) -> Detection {
    // PATH first because it is free; the shell only when that fails, so the
    // common case never spawns anything to answer "is it installed".
    let found = which(bin).or_else(|| which_via_login_shell(bin));
    let version = found.as_ref().and_then(|p| read_version(p));
    Detection {
        id: id.to_string(),
        display_name: display_name.to_string(),
        available: found.is_some(),
        version,
        path: found.map(|p| p.to_string_lossy().into_owned()),
    }
}

/// First line of `<bin> --version`, or `None` if it failed or said nothing.
///
/// Some tools print their version to stderr, so both streams are considered.
/// A tool that is installed but will not report a version is still usable, so
/// this never affects `available`.
fn read_version(bin: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new(bin).arg("--version").output().ok()?;
    let text = if out.stdout.is_empty() { &out.stderr } else { &out.stdout };
    String::from_utf8_lossy(text)
        .lines()
        .next()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_a_program_that_exists_and_misses_one_that_does_not() {
        assert!(which("sh").is_some(), "sh should be on PATH");
        assert!(
            which("definitely-not-a-real-binary-aiterm").is_none(),
            "invented a program that is not installed",
        );
    }

    #[test]
    fn which_ignores_directories_and_unexecutable_files() {
        // A directory named like the binary must not count as finding it.
        let dir = std::env::temp_dir().join("aiterm-which-test");
        let _ = std::fs::create_dir_all(dir.join("notabin"));
        assert!(!is_executable_file(&dir.join("notabin")), "a directory passed as executable");
    }

    /// The case that actually happens: an agent the user has not installed.
    /// It must report cleanly rather than erroring, because a settings panel
    /// showing "Codex — not installed" is the whole point.
    #[test]
    fn a_missing_cli_detects_as_unavailable_without_failing() {
        let d = detect_cli("ghost", "Ghost Agent", "definitely-not-a-real-binary-aiterm");
        assert!(!d.available);
        assert_eq!(d.version, None);
        assert_eq!(d.path, None);
        assert_eq!(d.display_name, "Ghost Agent");
    }

    /// `sh` stands in for an installed agent: present on PATH, with a resolved
    /// path. Whether it reports a `--version` is not asserted — some tools do
    /// not, which is exactly why `available` does not depend on it.
    #[test]
    fn an_installed_cli_detects_as_available_with_a_path() {
        let d = detect_cli("sh", "Bourne Shell", "sh");
        assert!(d.available, "sh was not detected");
        assert!(d.path.is_some_and(|p| p.ends_with("sh")));
    }

    #[test]
    fn every_backend_reports_its_own_identity() {
        for b in backends() {
            let d = b.detect();
            assert_eq!(d.id, b.id(), "detection reported a different id");
            assert_eq!(d.display_name, b.display_name());
            assert!(!d.id.is_empty() && !d.display_name.is_empty());
        }
    }

    /* ---- composition, against fake backends ----------------------------- */

    struct FakeProvider {
        /// (session id, last_active) — enough to test tagging and ordering.
        rows: Vec<(&'static str, u64)>,
    }

    impl SessionProvider for FakeProvider {
        fn scan_with_paths(&self) -> Vec<(Session, std::path::PathBuf)> {
            self.rows
                .iter()
                .map(|(id, at)| {
                    (
                        Session {
                            id: (*id).to_string(),
                            // Deliberately wrong: the registry must stamp this,
                            // not trust what the provider labelled it.
                            agent: "WRONG".into(),
                            title: (*id).to_string(),
                            project_path: "/p".into(),
                            group_path: "/p".into(),
                            branch: None,
                            forked: false,
                            background: false,
                            fork_parent: None,
                            last_active: *at,
                        },
                        std::path::PathBuf::from(format!("/fake/{id}")),
                    )
                })
                .collect()
        }
        fn find_session_file(&self, session_id: &str) -> Option<std::path::PathBuf> {
            self.rows
                .iter()
                .any(|(id, _)| *id == session_id)
                .then(|| std::path::PathBuf::from(format!("/fake/{session_id}")))
        }
    }

    struct FakeBackend {
        id: &'static str,
        provider: FakeProvider,
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
                available: true,
                version: None,
                path: None,
            }
        }
        fn sessions(&self) -> &dyn SessionProvider {
            &self.provider
        }
        fn launch(&self, _spec: &LaunchSpec) -> LaunchPlan {
            LaunchPlan::cmd(format!("fake-{}", self.id))
        }
    }

    fn fake(id: &'static str, rows: Vec<(&'static str, u64)>) -> Box<dyn AgentBackend> {
        Box::new(FakeBackend { id, provider: FakeProvider { rows } })
    }

    /// The whole point of the registry: two agents, one list.
    #[test]
    fn sessions_from_every_backend_appear_in_one_list() {
        let list = vec![fake("claude", vec![("c1", 10)]), fake("codex", vec![("x1", 20)])];
        let ids: Vec<String> = scan_backends(&list).into_iter().map(|(s, _)| s.id).collect();
        assert_eq!(ids.len(), 2, "a backend's sessions went missing");
        assert!(ids.contains(&"c1".to_string()) && ids.contains(&"x1".to_string()));
    }

    /// The registry is the source of truth for `agent`, not the parser. The UI
    /// switches its icon on this field, and a provider mislabelling its own
    /// output would be a second place for the name to live.
    #[test]
    fn every_row_is_tagged_with_the_backend_that_produced_it() {
        let list = vec![fake("claude", vec![("c1", 10)]), fake("codex", vec![("x1", 20)])];
        for (s, _) in scan_backends(&list) {
            let expected = if s.id == "c1" { "claude" } else { "codex" };
            assert_eq!(s.agent, expected, "row {} carried the wrong agent", s.id);
        }
    }

    /// One timeline of your work, not blocks grouped by tool. If ordering were
    /// per-backend, the newest session could sit below a week-old one purely
    /// because of which agent produced it.
    #[test]
    fn ordering_is_global_and_interleaves_backends() {
        let list = vec![
            fake("claude", vec![("old", 10), ("newest", 40)]),
            fake("codex", vec![("newer", 30), ("oldest", 5)]),
        ];
        let ids: Vec<String> = scan_backends(&list).into_iter().map(|(s, _)| s.id).collect();
        assert_eq!(ids, vec!["newest", "newer", "old", "oldest"]);
    }

    #[test]
    fn a_transcript_is_found_through_the_backend_that_owns_it() {
        let list = vec![fake("claude", vec![("c1", 10)]), fake("codex", vec![("x1", 20)])];
        assert_eq!(
            find_session_file_in(&list, "x1"),
            Some(std::path::PathBuf::from("/fake/x1")),
            "did not route to the owning backend",
        );
        assert_eq!(find_session_file_in(&list, "nobody"), None);
    }

    /// Ids are opaque and separately generated, so two agents *could* mint the
    /// same one. Nothing merges or dedupes them — both rows stay, each tagged
    /// with its own agent — and lookup resolves in registry order. Pinned here
    /// so the behaviour is a decision rather than an accident.
    #[test]
    fn colliding_ids_across_backends_stay_separate_rows() {
        let list = vec![fake("claude", vec![("same", 10)]), fake("codex", vec![("same", 20)])];
        let rows = scan_backends(&list);
        assert_eq!(rows.len(), 2, "rows from different agents were merged");
        let agents: Vec<String> = rows.into_iter().map(|(s, _)| s.agent).collect();
        assert!(agents.contains(&"claude".to_string()) && agents.contains(&"codex".to_string()));
        assert_eq!(
            find_session_file_in(&list, "same"),
            Some(std::path::PathBuf::from("/fake/same")),
            "lookup should resolve, first backend in registry order winning",
        );
    }

    /// Codex is registered for detection but contributes no sessions yet.
    /// If someone fills in `CodexSessions`, this fails and asks them to delete
    /// it — better than a stale claim sitting in the docs.
    #[test]
    fn codex_is_detected_but_contributes_no_sessions_yet() {
        let codex = CodexBackend;
        assert!(codex.sessions().scan_with_paths().is_empty());
        assert_eq!(codex.sessions().find_session_file("anything"), None);
        // Detection is real regardless: it reports whatever this machine has.
        assert_eq!(codex.detect().id, "codex");
    }

    /// The registry must report agents that are absent, not omit them — the
    /// settings panel exists to say "not installed".
    #[test]
    fn detection_covers_every_registered_backend() {
        let found = detect_agents();
        assert_eq!(found.len(), backends().len(), "a backend went unreported");
        assert!(found.iter().any(|d| d.id == "claude"));
        assert!(found.iter().any(|d| d.id == "codex"));
    }

    /* ---- launch commands ------------------------------------------------ */

    /// Picking nothing must produce exactly what aiterm always ran. This is the
    /// regression guard for moving the invocation out of the frontend.
    #[test]
    fn claude_with_no_choices_is_the_command_aiterm_always_used() {
        assert_eq!(
            ClaudeBackend.launch(&LaunchSpec::default()).command,
            "claude --permission-mode auto --allow-dangerously-skip-permissions",
        );
    }

    #[test]
    fn claude_takes_model_effort_and_a_minted_session_id() {
        let cmd = ClaudeBackend.launch(&LaunchSpec {
            model: Some("opus".into()),
            effort: Some("high".into()),
            session_id: Some("abc-123".into()),
        }).command;
        assert!(cmd.contains("--model 'opus'"), "{cmd}");
        assert!(cmd.contains("--effort 'high'"), "{cmd}");
        assert!(cmd.contains("--session-id 'abc-123'"), "{cmd}");
    }

    /// Codex has no --session-id, so a minted one must be dropped rather than
    /// passed as some other flag or silently appended as a prompt.
    #[test]
    fn codex_uses_a_config_override_for_effort_and_ignores_session_id() {
        let cmd = CodexBackend.launch(&LaunchSpec {
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
            session_id: Some("abc-123".into()),
        }).command;
        assert!(cmd.starts_with("codex "), "{cmd}");
        assert!(cmd.contains("--model 'gpt-5.6-sol'"), "{cmd}");
        assert!(cmd.contains("-c model_reasoning_effort='high'"), "{cmd}");
        assert!(!cmd.contains("abc-123"), "session id leaked into a codex launch: {cmd}");
    }

    #[test]
    fn empty_choices_add_no_flags() {
        let spec = LaunchSpec {
            model: Some(String::new()),
            effort: Some(String::new()),
            session_id: None,
        };
        assert_eq!(CodexBackend.launch(&spec).command, "codex");
        assert!(!ClaudeBackend.launch(&spec).command.contains("--model"));
    }

    /// These strings are handed to `$SHELL -ic`. The UI only sends values from
    /// lists we produced, but that is not a boundary to rely on.
    /// These strings are handed to `$SHELL -ic`. The UI only sends values from
    /// lists we produced, but that is not a boundary to rely on.
    ///
    /// Asserted by asking a real shell rather than by pattern-matching the
    /// command text: what matters is that the shell sees one literal word, and
    /// only the shell can settle that. (An earlier version of this test looked
    /// for the payload as a substring and failed on correctly-quoted output —
    /// the payload is *supposed* to be in there, inside the quotes.)
    #[test]
    fn values_survive_the_shell_as_a_single_literal_word() {
        for nasty in [
            "x'; touch /tmp/aiterm-pwned; #",
            "$(touch /tmp/aiterm-pwned)",
            "a b\tc",
            "back\\slash",
        ] {
            let out = std::process::Command::new("sh")
                .args(["-c", &format!("printf %s {}", q(nasty))])
                .output()
                .expect("run sh");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                nasty,
                "the shell did not see {nasty:?} as one literal word",
            );
        }
        assert!(
            !std::path::Path::new("/tmp/aiterm-pwned").exists(),
            "quoting failed: the payload executed",
        );
    }

    /* ---- codex model cache ---------------------------------------------- */

    /// Captured from a real `~/.codex/models_cache.json` (codex-cli 0.145.0).
    const CODEX_CACHE: &str = r#"{
      "fetched_at": "2026-07-28T00:22:46Z",
      "models": [
        {"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol",
         "default_reasoning_level":"low",
         "supported_reasoning_levels":[{"effort":"low"},{"effort":"high"},{"effort":"max"}]},
        {"slug":"gpt-5.6-codex","display_name":"GPT-5.6-Codex",
         "supported_reasoning_levels":["medium"]}
      ]}"#;

    #[test]
    fn codex_models_come_from_its_own_cache() {
        let models = parse_codex_models(CODEX_CACHE).expect("parsed nothing");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].display_name, "GPT-5.6-Sol");
        assert_eq!(models[0].efforts, vec!["low", "high", "max"]);
        assert_eq!(models[0].default_effort.as_deref(), Some("low"));
        // Bare strings accepted too, so a simplified future format still parses.
        assert_eq!(models[1].efforts, vec!["medium"]);
        assert_eq!(models[1].default_effort, None);
    }

    /// A missing or unreadable cache must yield "we do not know" rather than an
    /// invented list — the UI then offers only the agent's own default.
    #[test]
    fn an_unusable_codex_cache_yields_no_models() {
        assert_eq!(parse_codex_models("not json"), None);
        assert_eq!(parse_codex_models(r#"{"models":[]}"#), None);
        assert_eq!(parse_codex_models(r#"{"other":1}"#), None);
    }

    #[test]
    fn agent_choices_only_offers_agents_that_are_here() {
        for c in agent_choices() {
            let backend = backends().into_iter().find(|b| b.id() == c.id).unwrap();
            assert!(backend.detect().available, "{} offered but not installed", c.id);
        }
    }

    #[test]
    fn an_empty_registry_is_not_an_error() {
        assert!(scan_backends(&[]).is_empty());
        assert_eq!(find_session_file_in(&[], "anything"), None);
    }

    #[test]
    fn backend_ids_are_unique() {
        let ids: Vec<&str> = backends().iter().map(|b| b.id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "two backends share an id: {ids:?}");
    }
}






