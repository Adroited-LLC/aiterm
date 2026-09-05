//! Dispatching a task to OpenCode as a headless agent, and reading back its
//! report.
//!
//! This is the "OpenCode as a subagent" engine: hand it a prompt and a working
//! directory, it runs the task on the model aiterm would launch it with, and
//! returns the agent's final answer. The session it runs in is a normal
//! OpenCode session — it lands in `opencode.db` like any other, so it appears
//! in the sidebar, previews, resumes, and deletes through the machinery that
//! already exists (see [`crate::opencode`]).
//!
//! ## Why `opencode run`, not the server
//!
//! OpenCode can also be driven as a headless HTTP server (`opencode serve`), and
//! this engine was first written against it. But its prompt/wait API admits a
//! message without starting a run in this configuration — `/wait` answers
//! "not available yet" and the transcript never advances. `opencode run` is the
//! blessed one-shot path: it "runs opencode with a message," drives the whole
//! turn to completion itself, streams JSON events, and carries `--auto` to
//! auto-approve tools so a dispatched agent can actually *do work* rather than
//! stall on a permission prompt. Proven live against GLM 5.2 before this was
//! written.
//!
//! ## Same variables as a session, on purpose
//!
//! The one rule that makes this trustworthy: a dispatched agent launches with
//! the *same environment* aiterm gives an interactive OpenCode tab — nothing
//! special, nothing guessed. [`crate::pty::pty_spawn`] compiles two variables
//! from the provider store when it opens an OpenCode tab: `OPENROUTER_API_KEY`
//! (the key) and `OPENCODE_CONFIG_CONTENT` (the model's routing, merged over
//! the user's own config). [`apply_agent_env`] sets those exact two the exact
//! same way. Without them a bare `opencode` falls back to whatever default sits
//! in `opencode.json` — on this machine a local llama-server that may not be
//! running, which is a task that hangs forever instead of doing the work.

use std::io::Read;
use std::time::{Duration, Instant};

/// The ceiling on a single dispatched run. Generous — an agent task is minutes,
/// not seconds — but bounded, so a wedged run cannot hang a caller forever.
const RUN_CEILING: Duration = Duration::from_secs(1800);

/// A finished dispatch: the session it ran in, and the agent's final answer.
#[derive(serde::Serialize)]
pub struct Report {
    pub session_id: String,
    pub text: String,
}

/// The provider and model a delegated agent runs under when the caller names
/// neither.
#[derive(serde::Serialize)]
pub struct AgentTarget {
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// The provider and model OpenCode is configured to launch with: the first
/// OpenRouter provider, and the model it would actually open on.
///
/// A `~` prefix on a startup entry marks it parked, so the launch model is the
/// first entry *without* one — the same model an interactive OpenCode tab opens
/// on. Resolved from the launch settings, never hardcoded: re-pin in Model
/// access and both the MCP tool and the one-tap kick follow it.
pub fn default_target() -> (Option<String>, Option<String>) {
    let providers = crate::providers::load_providers();
    let Some(p) = providers.iter().find(|p| p.is_openrouter()) else {
        return (None, None);
    };
    let model = p
        .startup_models
        .iter()
        .find(|m| !m.starts_with('~'))
        .or_else(|| p.startup_models.first())
        .map(|m| m.trim_start_matches('~').trim().to_string())
        .filter(|m| !m.is_empty());
    (Some(p.id.clone()), model)
}

/// The default OpenCode target, for a UI that needs to name it on a dispatch
/// (the downgrade one-tap kick).
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub fn opencode_default_target() -> AgentTarget {
    let (provider, model) = default_target();
    AgentTarget { provider, model }
}

/// Set the same two variables `pty_spawn` injects for an OpenCode tab, resolved
/// the same way from the same store.
///
/// If the provider is not OpenRouter or has no key, neither is set and OpenCode
/// runs on its configured default — the honest behaviour, not a silent
/// substitution.
fn apply_agent_env(cmd: &mut std::process::Command, provider: Option<&str>, model: Option<&str>) {
    let Some(pid) = provider else { return };
    let providers = crate::providers::load_providers();
    let Some(p) = providers.iter().find(|p| p.id == pid) else {
        return;
    };
    if p.is_openrouter() && !p.api_key.is_empty() {
        cmd.env("OPENROUTER_API_KEY", &p.api_key);
    }
    if let Some(m) = model {
        if let Some(cfg) = crate::providers::opencode_config_content(p, m) {
            cmd.env("OPENCODE_CONFIG_CONTENT", cfg);
        }
    }
}

/// The `-m provider/model` value OpenCode wants. OpenRouter is the only provider
/// `opencode_config_content` routes, so its id is the one paired with the config
/// content set above.
fn model_flag(model: &str) -> String {
    format!("openrouter/{model}")
}

/// The session id an `opencode run --format json` stream reports.
///
/// Every event carries the same top-level `sessionID`; the first one is enough.
/// Kept separate from the report text because the report comes from the
/// database (the same source the sidebar reads), not from re-assembling the
/// event stream — the id is the only thing the stream is the authority on.
fn session_id_from_events(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(id) = v.get("sessionID").and_then(|s| s.as_str()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// The agent's text, re-assembled from the event stream. The fallback for when
/// the database read comes back empty — for a session that genuinely produced
/// only text (no tools), the `text` parts in order are the whole answer.
fn text_from_events(stdout: &str) -> String {
    let mut out = String::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(t) = v
                .get("part")
                .and_then(|p| p.get("text"))
                .and_then(|t| t.as_str())
            {
                out.push_str(t);
            }
        }
    }
    out
}

/// Run a child to completion, killing it if it outlives `ceiling`.
///
/// `std::process::Command` has no timeout. A reader thread drains stdout while
/// the main thread polls for exit against a deadline, so a wedged run is killed
/// rather than blocking the caller forever; whatever it printed before the kill
/// still comes back.
fn run_bounded_output(mut child: std::process::Child, ceiling: Duration) -> Result<String, String> {
    let mut stdout = child.stdout.take().ok_or("child has no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + ceiling;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    let _ = child.wait();
    reader
        .join()
        .map_err(|_| "stdout reader panicked".to_string())
}

/// Dispatch one task and return the agent's final answer.
///
/// Spawns `opencode run --format json` with the session env, lets it drive the
/// turn to completion, then reads the answer from `opencode.db` — the same
/// transcript the sidebar shows, so there is one definition of "what the agent
/// said," not two. The event stream is used only for the session id (its one
/// authority) and as a fallback when the database read is empty.
pub fn dispatch(
    prompt: &str,
    cwd: &str,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Report, String> {
    if prompt.trim().is_empty() {
        return Err("nothing to dispatch — the prompt is empty".into());
    }

    // Resolve opencode's absolute path rather than trusting PATH. The desktop
    // launcher starts aiterm with a minimal environment — `~/.opencode/bin` and
    // node-manager shims are not on it — so a bare `Command::new("opencode")`
    // fails with "No such file or directory". This is the same resolver
    // detection uses: PATH first, then the user's login shell.
    let bin = crate::agents::which("opencode")
        .or_else(|| crate::agents::which_via_login_shell("opencode"))
        .ok_or("opencode is not installed, or not on aiterm's PATH")?;
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("run").args(["--format", "json"]);
    // Auto-approve tools: a dispatched agent runs unattended, so a permission
    // prompt would wedge it. This is the same `--auto` the CLI documents as
    // "dangerous" — appropriate here because the whole point is autonomous work.
    cmd.arg("--auto");
    // opencode's own tool spawns need a usable PATH too; the minimal one the GUI
    // hands us may lack the user's bin dirs. Prepend opencode's own directory and
    // ~/.local/bin so `--auto` work can find its tools.
    if let Ok(existing) = std::env::var("PATH") {
        let mut parts: Vec<String> = Vec::new();
        if let Some(dir) = bin.parent() {
            parts.push(dir.to_string_lossy().into_owned());
        }
        if let Some(home) = dirs::home_dir() {
            parts.push(format!("{}/.local/bin", home.display()));
        }
        parts.push(existing);
        cmd.env("PATH", parts.join(":"));
    }
    if let Some(m) = model {
        cmd.args(["-m", &model_flag(m)]);
    }
    if std::path::Path::new(cwd).is_dir() {
        cmd.args(["--dir", cwd]);
    }
    // The prompt is a positional arg, which puts it in argv (visible in `ps` to
    // the same user). Acceptable for now — same exposure as any CLI argument —
    // but a candidate to move onto stdin if OpenCode grows a way to read it.
    cmd.arg(prompt);
    apply_agent_env(&mut cmd, provider, model);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    // Null stdin, or the child inherits the parent's. That is harmless from the
    // GUI, but the MCP server's stdin is the JSON-RPC channel Claude Code speaks
    // over — an inherited open pipe makes `opencode run` block reading it
    // forever (and would corrupt the protocol). Headless runs read no input.
    cmd.stdin(std::process::Stdio::null());

    let child = cmd
        .spawn()
        .map_err(|e| format!("could not start opencode run: {e}"))?;
    let stdout = run_bounded_output(child, RUN_CEILING)?;

    let session_id = session_id_from_events(&stdout)
        .ok_or("opencode run produced no session — it may have failed to start")?;

    // The answer, read from the store the sidebar reads. Fall back to the event
    // stream only if the database has nothing yet.
    let text = crate::opencode::messages(&session_id)
        .into_iter()
        .rev()
        .find(|(role, _)| role == "assistant")
        .map(|(_, t)| t)
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| text_from_events(&stdout));

    Ok(Report { session_id, text })
}

/// Dispatch a task to OpenCode and return its report.
///
/// `provider`/`model` are the same pair a session launch resolves (see
/// `launch.rs`): the provider whose key and routing to run under, and the model
/// id within it. Omit them to let OpenCode use its configured default.
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn opencode_dispatch(
    prompt: String,
    cwd: String,
    provider: Option<String>,
    model: Option<String>,
) -> Result<Report, String> {
    crate::run_blocking(move || dispatch(&prompt, &cwd, provider.as_deref(), model.as_deref()))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The event parser, on a captured `opencode run --format json` stream (the
    /// real shape, from the live probe): the session id is the first event's,
    /// and the text fallback concatenates `text` parts in order.
    #[test]
    fn parses_session_id_and_text_from_the_event_stream() {
        let stream = concat!(
            r#"{"type":"step_start","sessionID":"ses_abc","part":{"type":"step-start"}}"#,
            "\n",
            r#"{"type":"text","sessionID":"ses_abc","part":{"type":"text","text":"PO"}}"#,
            "\n",
            r#"{"type":"text","sessionID":"ses_abc","part":{"type":"text","text":"NG"}}"#,
            "\n",
            r#"{"type":"step_finish","sessionID":"ses_abc","part":{"type":"step-finish"}}"#,
            "\n",
        );
        assert_eq!(session_id_from_events(stream).as_deref(), Some("ses_abc"));
        assert_eq!(text_from_events(stream), "PONG");
    }

    /// An empty or non-JSON stream names no session and yields no text, rather
    /// than panicking.
    #[test]
    fn a_silent_stream_is_no_session_and_no_text() {
        assert_eq!(session_id_from_events(""), None);
        assert_eq!(session_id_from_events("not json\n"), None);
        assert_eq!(text_from_events(""), "");
    }

    #[test]
    fn model_flag_is_openrouter_prefixed() {
        assert_eq!(model_flag("z-ai/glm-5.2"), "openrouter/z-ai/glm-5.2");
    }

    /// Live end-to-end: dispatch a no-tool prompt to OpenCode on the model
    /// aiterm is configured to launch it with, and confirm the answer comes
    /// back through the report. Ignored and run alone — it spawns a real run,
    /// spends a token or two on the real provider, and needs opencode installed
    /// with a configured OpenRouter key.
    ///   cargo test --lib live_dispatch_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_dispatch_smoke() {
        if crate::agents::which("opencode").is_none() {
            eprintln!("no opencode on PATH; skipping");
            return;
        }
        let cwd = std::env::temp_dir();
        let report = dispatch(
            "Reply with exactly this one word and nothing else: PONG",
            &cwd.to_string_lossy(),
            Some("openrouter"),
            Some("z-ai/glm-5.2"),
        )
        .expect("dispatch should return a report");
        eprintln!("session: {}", report.session_id);
        eprintln!("report : {:?}", report.text);
        assert!(
            report.text.to_uppercase().contains("PONG"),
            "expected PONG, got {:?}",
            report.text
        );
        // Don't litter the real db: dump+delete the probe session, then remove
        // the dump the trash step wrote into the temp dir.
        let _ = crate::opencode::delete_to_trash(&report.session_id, &cwd);
        let _ = std::fs::remove_file(cwd.join(format!("{}.jsonl", report.session_id)));
    }

    /// The GUI-PATH regression: with `~/.opencode/bin` stripped from PATH (as the
    /// desktop launcher hands it), dispatch must still find opencode via the
    /// login shell instead of failing "No such file or directory".
    ///   cargo test --lib dispatch_resolves_without_path -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dispatch_resolves_without_path() {
        if crate::agents::which_via_login_shell("opencode").is_none() {
            eprintln!("no opencode via login shell; skipping");
            return;
        }
        std::env::set_var("PATH", "/usr/bin:/bin"); // the minimal launcher PATH
        let cwd = std::env::temp_dir();
        let report = dispatch(
            "Reply with exactly this one word and nothing else: PONG",
            &cwd.to_string_lossy(),
            Some("openrouter"),
            Some("z-ai/glm-5.2"),
        )
        .expect("dispatch must resolve opencode via the login shell");
        eprintln!("report: {:?}", report.text);
        assert!(report.text.to_uppercase().contains("PONG"));
        let _ = crate::opencode::delete_to_trash(&report.session_id, &cwd);
        let _ = std::fs::remove_file(cwd.join(format!("{}.jsonl", report.session_id)));
    }
}
