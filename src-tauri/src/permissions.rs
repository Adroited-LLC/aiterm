//! Which permission mode each engine starts in.
//!
//! Every engine aiterm launches has a switch for how much it asks before
//! acting, and every one spells it differently: claude has `--permission-mode`
//! and `--dangerously-skip-permissions`, codex an approval policy and a
//! sandbox, grok `--permission-mode` and `--always-approve`, opencode
//! `--auto`. Until now claude got one fixed answer on every launch and the
//! others got nothing — which suits one person's habits and nobody else's.
//! Someone who wants every action approved and someone who never wants to be
//! asked are both right, so the mode is a setting, one per engine.
//!
//! ## Where the answer applies
//!
//! To every session of that engine aiterm opens: a fresh start, a ▶ resume, a
//! restart of an ended tab, and a `/clear` re-key. The flags are appended by
//! the launch resolver (`launch.rs`), not by each backend's `launch()`, because
//! three of the four engines build their resume command on a separate path
//! from their launch, and a default that reached starts but not resumes would
//! look like the engine's own behaviour rather than a bug.
//!
//! ## The tables are the engines' own
//!
//! Each backend declares its modes ([`AgentBackend::permission_modes`]) with
//! the exact flags each one is spelled as, taken from that engine's `--help`.
//! The first entry is what aiterm uses when nothing is stored, and for claude
//! it is exactly the flag pair every launch carried before this existed, so
//! nobody's sessions change until they choose. An id the engine does not list
//! is refused at save time: `claude --permission-mode yolo` exits on open, and
//! a terminal that dies on open is worse than a permission prompt.
//!
//! ## Storage
//!
//! `~/.config/aiterm/agents.json`, beside the provider store:
//! `{"permission": {"claude": "bypassPermissions"}}`. Read on every launch —
//! it is one small file, and a setting that took effect only after a restart
//! would be reported as not working. Nothing secret is in it.

use serde::Serialize;

use crate::agents::{AgentBackend, PermissionMode};

fn config_path() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".config/aiterm/agents.json"))
}

fn load() -> serde_json::Value {
    let Some(path) = config_path() else {
        return serde_json::json!({});
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return serde_json::json!({});
    };
    // A corrupt or hand-edited file reads as "nothing stored" — every engine
    // falls back to its first mode — rather than blocking every launch.
    serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({}))
}

fn save(store: &serde_json::Value) -> Result<(), String> {
    let path = config_path().ok_or("no home directory")?;
    let dir = path.parent().ok_or("bad config path")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The stored mode id for an engine, if the file names one.
fn stored_mode(store: &serde_json::Value, agent_id: &str) -> Option<String> {
    store
        .get("permission")?
        .get(agent_id)?
        .as_str()
        .map(|s| s.to_string())
}

/// The mode an engine starts in, given a store: the stored one when the
/// engine still lists it, else the engine's first. `None` only for an engine
/// with no permission switch at all.
pub fn mode_for_in(
    store: &serde_json::Value,
    backend: &dyn AgentBackend,
) -> Option<&'static PermissionMode> {
    let modes = backend.permission_modes();
    let stored = stored_mode(store, backend.id());
    modes
        .iter()
        .find(|m| Some(m.id) == stored.as_deref())
        .or_else(|| modes.first())
}

/// [`mode_for_in`] against the saved store.
pub fn mode_for(backend: &dyn AgentBackend) -> Option<&'static PermissionMode> {
    mode_for_in(&load(), backend)
}

/// The flags to append to any command that opens a session of this engine.
/// Empty for the engine's own default, and for an engine with no switch.
pub fn flags_for(backend: &dyn AgentBackend) -> String {
    mode_for(backend)
        .map(|m| m.flags.join(" "))
        .unwrap_or_default()
}

/// One engine's modes and its current choice, for the settings panel.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AgentPermissions {
    pub agent_id: String,
    pub display_name: String,
    pub modes: Vec<PermissionMode>,
    /// The id of the mode in force — stored, or the first when nothing is.
    pub selected: String,
}

fn view_in(store: &serde_json::Value, list: &[Box<dyn AgentBackend>]) -> Vec<AgentPermissions> {
    list.iter()
        .filter_map(|b| {
            let selected = mode_for_in(store, &**b)?;
            Some(AgentPermissions {
                agent_id: b.id().to_string(),
                display_name: b.display_name().to_string(),
                modes: b.permission_modes().to_vec(),
                selected: selected.id.to_string(),
            })
        })
        .collect()
}

/// Every engine with a permission switch, its modes, and the one in force.
#[tauri::command]
pub fn agent_permissions() -> Vec<AgentPermissions> {
    view_in(&load(), &crate::agents::backends())
}

/// Store the mode an engine starts in. Refused for an id the engine does not
/// list — see the module note on why.
#[tauri::command]
pub fn agent_permission_set(agent_id: String, mode: String) -> Result<Vec<AgentPermissions>, String> {
    let list = crate::agents::backends();
    let backend = list
        .iter()
        .find(|b| b.id() == agent_id)
        .ok_or_else(|| format!("No engine called {agent_id}."))?;
    if !backend.permission_modes().iter().any(|m| m.id == mode) {
        return Err(format!("{} has no permission mode called {mode}.", backend.display_name()));
    }
    let mut store = load();
    if !store.is_object() {
        store = serde_json::json!({});
    }
    let permission = store
        .as_object_mut()
        .expect("checked above")
        .entry("permission")
        .or_insert_with(|| serde_json::json!({}));
    if !permission.is_object() {
        *permission = serde_json::json!({});
    }
    permission
        .as_object_mut()
        .expect("just made it one")
        .insert(agent_id, serde_json::Value::String(mode));
    save(&store)?;
    Ok(view_in(&store, &list))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{ClaudeBackend, CodexBackend, OpenCodeBackend};
    use crate::grok::GrokBackend;
    use crate::antigravity::AntigravityBackend;

    #[test]
    fn nothing_stored_means_the_engines_first_mode() {
        let store = serde_json::json!({});
        let m = mode_for_in(&store, &ClaudeBackend).expect("claude has modes");
        assert_eq!(m.id, "auto");
        // Which is the flag pair every claude launch carried before this
        // setting existed — nobody's sessions change until they choose.
        assert_eq!(
            m.flags,
            &["--permission-mode auto", "--allow-dangerously-skip-permissions"]
        );
        assert_eq!(mode_for_in(&store, &CodexBackend).unwrap().flags, &[] as &[&str]);
        assert_eq!(mode_for_in(&store, &GrokBackend).unwrap().flags, &[] as &[&str]);
        assert_eq!(mode_for_in(&store, &OpenCodeBackend).unwrap().flags, &[] as &[&str]);
        assert_eq!(mode_for_in(&store, &AntigravityBackend).unwrap().flags, &[] as &[&str]);
    }

    #[test]
    fn a_stored_mode_wins_and_junk_falls_back() {
        let store = serde_json::json!({"permission": {"claude": "bypassPermissions", "codex": "yolo"}});
        assert_eq!(
            mode_for_in(&store, &ClaudeBackend).unwrap().flags,
            &["--dangerously-skip-permissions"]
        );
        // "yolo" is not a mode codex lists; the file may have been hand-edited.
        assert_eq!(mode_for_in(&store, &CodexBackend).unwrap().id, "default");
        // A file that is not even an object is "nothing stored".
        assert_eq!(mode_for_in(&serde_json::json!([1, 2]), &ClaudeBackend).unwrap().id, "auto");
    }

    #[test]
    fn every_mode_is_spelled_the_way_its_engine_documents() {
        let store = serde_json::json!({"permission": {
            "claude": "acceptEdits", "codex": "never", "grok": "bypassPermissions", "opencode": "auto",
            "antigravity": "accept-edits"
        }});
        let flags = |b: &dyn AgentBackend| mode_for_in(&store, b).unwrap().flags.join(" ");
        assert_eq!(flags(&ClaudeBackend), "--permission-mode acceptEdits --allow-dangerously-skip-permissions");
        assert_eq!(flags(&CodexBackend), "-a never -s workspace-write");
        assert_eq!(flags(&GrokBackend), "--always-approve");
        assert_eq!(flags(&OpenCodeBackend), "--auto");
        assert_eq!(flags(&AntigravityBackend), "--mode accept-edits");
    }

    #[test]
    fn the_view_lists_only_engines_with_a_switch() {
        let store = serde_json::json!({"permission": {"grok": "auto"}});
        let view = view_in(&store, &crate::agents::backends());
        let ids: Vec<&str> = view.iter().map(|v| v.agent_id.as_str()).collect();
        assert_eq!(ids, ["claude", "codex", "grok", "opencode", "antigravity"], "the API chat engine has no switch");
        let grok = view.iter().find(|v| v.agent_id == "grok").unwrap();
        assert_eq!(grok.selected, "auto");
        assert!(grok.modes.iter().any(|m| m.id == "bypassPermissions"));
    }
}
