//! Everything Claude Code reads that decides how a session behaves, gathered
//! for display. Read-only by design: these are files every session on the
//! machine depends on, and Phase 1 shows them without touching them.

pub mod settings;
pub mod concern;
pub mod instructions;
pub mod mcp;
pub mod skills;

use serde::Serialize;
use settings::{Layer, LayerId, Setting};

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".into())
}

/// Every settings file, lowest precedence first. Paths only — reading happens
/// in the command, so this is testable without a filesystem. The injected
/// path is passed in rather than built here, so this stays pure while still
/// reporting the path `hooklink::settings_path` actually uses.
fn layer_paths(home: &str, project: Option<&str>, injected: &str) -> Vec<(LayerId, String)> {
    let mut out = vec![(LayerId::User, format!("{home}/.claude/settings.json"))];
    if let Some(p) = project {
        out.push((LayerId::Project, format!("{p}/.claude/settings.json")));
        out.push((LayerId::ProjectLocal, format!("{p}/.claude/settings.local.json")));
    }
    out.push((LayerId::Injected, injected.to_string()));
    out
}

/// The flags aiterm adds to every claude launch — the launcher's own list.
fn injected_flags() -> &'static [&'static str] {
    crate::agents::CLAUDE_LAUNCH_FLAGS
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub layers: Vec<Layer>,
    pub settings: Vec<Setting>,
    /// Parse failures, one per unusable layer.
    pub errors: Vec<String>,
    /// Group order for the panel.
    pub order: Vec<String>,
    pub injected_flags: Vec<String>,
}

/// Attach each layer's parse error to its own row.
///
/// `resolve` reports failures as "<label>: <serde message>", which is the only
/// link back to the file that failed — the settings list itself has already
/// dropped that layer. Kept separate from the command so the join is testable
/// without a filesystem.
fn attach_errors(layers: &mut [Layer], errors: &[String]) {
    for e in errors {
        if let Some((label, msg)) = e.split_once(": ") {
            if let Some(l) = layers.iter_mut().find(|l| l.id.label() == label) {
                l.error = Some(msg.to_string());
            }
        }
    }
}

/// A layer we could not read at all.
///
/// Absence is not an error — the spec is explicit that a missing file reads as
/// "not present". Anything else is: a settings file that exists and governs
/// every session in this project, but is unreadable, must not look identical to
/// one that was never written.
fn unreadable(id: LayerId, path: &str, e: &std::io::Error) -> Layer {
    let error = match e.kind() {
        std::io::ErrorKind::NotFound => None,
        _ => Some(e.to_string()),
    };
    Layer { id, path: path.to_string(), present: false, error }
}

/// Read every settings layer once: a row per file for display, plus the text of
/// the ones that were readable.
///
/// Shared with `claude_skills`, which needs `enabledPlugins` out of the same
/// layers — a plugin can be switched off in a project file, not only in
/// `~/.claude/settings.json`.
fn read_layers(home: &str, project: Option<&str>) -> (Vec<Layer>, Vec<(LayerId, String)>) {
    // The real path the hook writer uses — falling back to the historical
    // default only when dirs::data_dir() can't resolve at all (no HOME),
    // where the whole panel is already guesswork.
    let injected = crate::hooklink::settings_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{home}/.local/share/aiterm/claude-hook-settings.json"));
    let mut layers = Vec::new();
    let mut texts: Vec<(LayerId, String)> = Vec::new();
    for (id, path) in layer_paths(home, project, &injected) {
        match std::fs::read_to_string(&path) {
            Ok(t) => {
                layers.push(Layer { id, path, present: true, error: None });
                texts.push((id, t));
            }
            Err(e) => layers.push(unreadable(id, &path, &e)),
        }
    }
    (layers, texts)
}

#[tauri::command]
pub fn claude_settings(project: Option<String>) -> SettingsView {
    let (mut layers, texts) = read_layers(&home(), project.as_deref());
    let borrowed: Vec<(LayerId, &str)> = texts.iter().map(|(i, t)| (*i, t.as_str())).collect();
    let (settings, errors) = settings::resolve(&borrowed);
    attach_errors(&mut layers, &errors);
    SettingsView {
        layers,
        settings,
        errors,
        order: concern::ORDER.iter().map(|s| s.to_string()).collect(),
        injected_flags: injected_flags().iter().map(|s| s.to_string()).collect(),
    }
}

#[tauri::command]
pub fn claude_instructions(project: Option<String>) -> Vec<instructions::Doc> {
    let h = home();
    let mut roots = vec![("user".to_string(), format!("{h}/.claude/CLAUDE.md"))];
    if let Some(p) = &project {
        roots.push(("project".to_string(), format!("{p}/CLAUDE.md")));
    }
    let mut read = |path: &str| std::fs::read_to_string(path).ok();
    instructions::chain(&roots, &mut read)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpView {
    pub servers: Vec<mcp::Server>,
    /// False when no local config could be read at all, which is a different
    /// answer from "none configured".
    pub local_config_read: bool,
    /// Parse failures, one per source that exists but is malformed.
    pub errors: Vec<String>,
}

#[tauri::command]
pub fn claude_mcp(project: Option<String>) -> McpView {
    let h = home();
    let claude_json = std::fs::read_to_string(format!("{h}/.claude.json")).ok();
    let mcp_json = project
        .as_ref()
        .and_then(|p| std::fs::read_to_string(format!("{p}/.mcp.json")).ok());
    let (servers, local_config_read, errors) = mcp::read(
        claude_json.as_deref(),
        mcp_json.as_deref(),
        project.as_deref().unwrap_or(""),
    );
    McpView { servers, local_config_read, errors }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsView {
    pub skills: Vec<skills::Skill>,
    /// Installed plugins that settings switch off. Their skills are on disk but
    /// out of a session's reach, so the panel says how many it left out rather
    /// than presenting a short list as the whole truth.
    pub disabled_plugins: usize,
    /// Why a source could not be used — a malformed plugin record otherwise
    /// reads as "no skills found".
    pub errors: Vec<String>,
}

#[tauri::command]
pub fn claude_skills(project: Option<String>) -> SkillsView {
    let h = home();
    let mut roots = vec![("user".to_string(), format!("{h}/.claude/skills"))];
    if let Some(p) = &project {
        roots.push(("project".to_string(), format!("{p}/.claude/skills")));
    }

    let (_, texts) = read_layers(&h, project.as_deref());
    let layer_texts: Vec<&str> = texts.iter().map(|(_, t)| t.as_str()).collect();
    let enabled = skills::enabled_plugins(&layer_texts);

    let mut errors = Vec::new();
    let record = format!("{h}/.claude/plugins/installed_plugins.json");
    let plugins = match std::fs::read_to_string(&record) {
        Ok(t) => skills::plugin_roots(&t, &enabled),
        // No plugin record is the ordinary state of an install with no plugins,
        // not a failure. Any other read error is one.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => skills::PluginRoots::default(),
        Err(e) => skills::PluginRoots {
            errors: vec![format!("installed_plugins.json: {e}")],
            ..Default::default()
        },
    };
    errors.extend(plugins.errors);
    roots.extend(plugins.roots);

    let mut out = Vec::new();
    for (source, dir) in roots {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path().join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let (name, description) = skills::frontmatter(&text);
            let dir_name = e.file_name().to_string_lossy().to_string();
            out.push(skills::Skill {
                name: if name.is_empty() { dir_name } else { name },
                description,
                source: source.clone(),
                path: path.to_string_lossy().to_string(),
            });
        }
    }
    out.sort_by(|a, b| (a.source.clone(), a.name.clone()).cmp(&(b.source.clone(), b.name.clone())));
    SkillsView { skills: out, disabled_plugins: plugins.disabled, errors }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_paths_are_built_from_home_and_project() {
        let l = layer_paths("/h", Some("/p"), "/h/.local/share/aiterm/claude-hook-settings.json");
        assert_eq!(l[0].1, "/h/.claude/settings.json");
        assert_eq!(l[1].1, "/p/.claude/settings.json");
        assert_eq!(l[2].1, "/p/.claude/settings.local.json");
        assert!(l[3].1.contains("claude-hook-settings.json"), "{:?}", l[3].1);
    }

    #[test]
    fn with_no_project_open_only_the_user_and_injected_layers_are_looked_for() {
        let l = layer_paths("/h", None, "/h/.local/share/aiterm/claude-hook-settings.json");
        assert!(l.iter().all(|(id, _)| *id != settings::LayerId::Project));
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn the_injected_layer_is_the_path_the_hook_writer_uses_not_a_guess() {
        // Hardcoding ~/.local/share here would report aiterm's own settings
        // file as absent under a non-default XDG_DATA_HOME, while sessions
        // were still being launched with it.
        let l = layer_paths("/h", None, "/custom/aiterm/claude-hook-settings.json");
        assert_eq!(l.last().unwrap().1, "/custom/aiterm/claude-hook-settings.json");
    }

    #[test]
    fn the_flags_reported_are_the_launchers_own() {
        // Not a second copy that can drift from what is run.
        assert_eq!(injected_flags(), crate::agents::CLAUDE_LAUNCH_FLAGS);
    }

    #[test]
    fn a_layer_that_failed_to_parse_wears_its_own_error() {
        let mut layers = vec![
            Layer { id: LayerId::User, path: "/h/s.json".into(), present: true, error: None },
            Layer { id: LayerId::Project, path: "/p/s.json".into(), present: true, error: None },
        ];
        attach_errors(&mut layers, &["project: expected value at line 1".to_string()]);
        assert_eq!(layers[1].error.as_deref(), Some("expected value at line 1"));
        assert!(layers[0].error.is_none(), "a healthy layer must not inherit a sibling's error");
    }

    #[test]
    fn a_two_word_layer_label_is_still_matched() {
        // "project local" contains a space; a split on the wrong separator, or a
        // match on the first word, would silently attach nothing.
        let mut layers = vec![Layer {
            id: LayerId::ProjectLocal, path: "/p/l.json".into(), present: true, error: None,
        }];
        attach_errors(&mut layers, &["project local: trailing comma".to_string()]);
        assert_eq!(layers[0].error.as_deref(), Some("trailing comma"));
    }

    #[test]
    fn a_serde_message_containing_a_colon_keeps_all_of_itself() {
        // split_once, not split — otherwise the message is truncated at its own
        // punctuation and the row explains less than it could.
        let mut layers = vec![Layer {
            id: LayerId::User, path: "/h/s.json".into(), present: true, error: None,
        }];
        attach_errors(&mut layers, &["user: bad thing: at line 3 column 5".to_string()]);
        assert_eq!(layers[0].error.as_deref(), Some("bad thing: at line 3 column 5"));
    }

    #[test]
    fn a_file_that_is_not_there_is_not_an_error() {
        let e = std::io::Error::from(std::io::ErrorKind::NotFound);
        let l = unreadable(LayerId::User, "/h/.claude/settings.json", &e);
        assert!(!l.present);
        assert!(l.error.is_none(), "{:?}", l.error);
    }

    #[test]
    fn a_file_that_exists_but_cannot_be_read_says_so_instead_of_reading_as_absent() {
        // It still governs every session; "not present" would be a wrong answer
        // to the only question this panel is asked.
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let l = unreadable(LayerId::ProjectLocal, "/p/.claude/settings.local.json", &e);
        assert!(l.error.is_some(), "an unreadable layer must explain itself");
    }

    /// Reads the real home directory, so it is skipped where there is none.
    #[test]
    fn a_real_read_answers_without_panicking() {
        if std::env::var("HOME").is_err() {
            return;
        }
        let s = claude_settings(None);
        // Whatever is on this machine, the shape must be answerable.
        assert!(s.layers.iter().any(|l| l.id == settings::LayerId::User));
    }
}
