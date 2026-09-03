//! Skills available to a Claude session, and which tree each came from.
//!
//! User and project skills are one directory each. Plugin skills are not:
//! the plugin cache keeps several versions of the same plugin side by side
//! (`document-skills` is here at three version hashes), so globbing it reports
//! every skill two or three times. `installed_plugins.json` records an
//! `installPath` per installed plugin, which is the only non-guess available.
//!
//! Being installed is not the same as being enabled: `enabledPlugins` in the
//! settings layers switches a plugin off while its files stay in the cache, and
//! a session cannot reach the skills of a plugin that is off. Listing them
//! would be a promise the panel cannot keep.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// "user", "project", or the plugin's name.
    pub source: String,
    pub path: String,
}

/// Where to look for plugin skills, and what was left out on the way.
#[derive(Debug, Default)]
pub struct PluginRoots {
    /// `(label, skills directory)` per enabled plugin.
    pub roots: Vec<(String, String)>,
    /// Installed plugins switched off in settings. Counted rather than named so
    /// the panel can say the list is short on purpose.
    pub disabled: usize,
    /// Why the plugin record could not be used. A malformed file that silently
    /// yielded nothing would read as "no plugins installed".
    pub errors: Vec<String>,
}

/// `enabledPlugins` across the settings layers, lowest precedence first — the
/// same layers `claude_settings` reads, because a plugin can be switched off in
/// a project file and not only in `~/.claude/settings.json`.
///
/// Keys are `name@marketplace`, exactly as `installed_plugins.json` spells
/// them, so no normalising is needed to match the two up. A layer that does not
/// parse is skipped without comment here: the Settings section reads the same
/// files and already reports the parse error against the file it came from.
pub fn enabled_plugins(layer_texts: &[&str]) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    for text in layer_texts {
        let Ok(root) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        let Some(map) = root.get("enabledPlugins").and_then(Value::as_object) else {
            continue;
        };
        for (id, v) in map {
            if let Some(b) = v.as_bool() {
                out.insert(id.clone(), b);
            }
        }
    }
    out
}

/// `(label, skills directory)` for every *enabled* installed plugin, from the
/// record that names the live version.
///
/// A plugin absent from `enabled` counts as enabled: `enabledPlugins` records
/// the choices made, and the default for an installed plugin is on.
pub fn plugin_roots(installed_plugins_json: &str, enabled: &HashMap<String, bool>) -> PluginRoots {
    let mut view = PluginRoots::default();
    let root = match serde_json::from_str::<Value>(installed_plugins_json) {
        Ok(v) => v,
        Err(e) => {
            view.errors.push(format!("installed_plugins.json: {e}"));
            return view;
        }
    };
    let Some(plugins) = root.get("plugins").and_then(Value::as_object) else {
        view.errors
            .push("installed_plugins.json: no \"plugins\" object".into());
        return view;
    };
    for (id, entries) in plugins {
        if enabled.get(id) == Some(&false) {
            view.disabled += 1;
            continue;
        }
        // "superpowers@claude-plugins-official" reads better as "superpowers".
        let label = id.split('@').next().unwrap_or(id).to_string();
        let Some(first) = entries.as_array().and_then(|a| a.first()) else {
            continue;
        };
        let Some(path) = first.get("installPath").and_then(Value::as_str) else {
            continue;
        };
        view.roots.push((label, format!("{path}/skills")));
    }
    view.roots.sort();
    view
}

/// `name` and `description` from a SKILL.md's YAML frontmatter. Absent fields
/// come back empty: a skill that exists is worth a row even undocumented.
pub fn frontmatter(text: &str) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    let mut in_front = false;
    for line in text.lines() {
        if line.trim() == "---" {
            if in_front {
                break; // only the first block counts
            }
            in_front = true;
            continue;
        }
        if !in_front {
            break; // no frontmatter at all
        }
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("description:") {
            description = v.trim().to_string();
        }
    }
    (name, description)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTALLED: &str = r#"{
      "version": 2,
      "plugins": {
        "superpowers@claude-plugins-official": [
          {"scope": "user",
           "installPath": "/h/.claude/plugins/cache/claude-plugins-official/superpowers/6.2.0",
           "version": "6.2.0"}
        ],
        "document-skills@anthropic-agent-skills": [
          {"scope": "user",
           "installPath": "/h/.claude/plugins/cache/anthropic-agent-skills/document-skills/fa0fa64bdc96",
           "version": "fa0fa64bdc96"}
        ]
      }
    }"#;

    /// One of the two plugins in `INSTALLED` switched off, the other on.
    const SETTINGS: &str = r#"{
      "enabledPlugins": {
        "document-skills@anthropic-agent-skills": false,
        "superpowers@claude-plugins-official": true
      }
    }"#;

    fn all_enabled() -> HashMap<String, bool> {
        HashMap::new()
    }

    #[test]
    fn a_plugins_skills_directory_comes_from_its_recorded_install_path() {
        // The cache holds three versions of document-skills. Globbing it would
        // list every skill three times; installed_plugins.json names the live
        // one, so nothing is guessed.
        let v = plugin_roots(INSTALLED, &all_enabled());
        assert!(v.roots.iter().any(|(label, dir)| label == "superpowers"
            && dir == "/h/.claude/plugins/cache/claude-plugins-official/superpowers/6.2.0/skills"));
        assert_eq!(
            v.roots.len(),
            2,
            "one root per installed plugin, not per cached version"
        );
    }

    #[test]
    fn a_plugin_with_no_install_path_is_skipped_rather_than_guessed() {
        let v = plugin_roots(
            r#"{"plugins": {"broken@x": [{"scope": "user"}]}}"#,
            &all_enabled(),
        );
        assert!(v.roots.is_empty());
    }

    #[test]
    fn a_plugin_settings_switched_off_contributes_no_skills_and_is_counted() {
        // Its SKILL.md files are still in the cache, but no session can reach
        // them, so listing them would offer skills that do not exist in use.
        let v = plugin_roots(INSTALLED, &enabled_plugins(&[SETTINGS]));
        assert!(
            v.roots.iter().all(|(label, _)| label != "document-skills"),
            "{:?}",
            v.roots
        );
        assert_eq!(v.disabled, 1);
    }

    #[test]
    fn a_plugin_settings_switched_on_is_kept() {
        let v = plugin_roots(INSTALLED, &enabled_plugins(&[SETTINGS]));
        assert!(
            v.roots.iter().any(|(label, _)| label == "superpowers"),
            "{:?}",
            v.roots
        );
    }

    #[test]
    fn a_plugin_no_layer_mentions_is_kept_because_installed_means_on_by_default() {
        let v = plugin_roots(INSTALLED, &enabled_plugins(&[r#"{"enabledPlugins": {}}"#]));
        assert_eq!(v.roots.len(), 2, "{:?}", v.roots);
        assert_eq!(v.disabled, 0);
    }

    #[test]
    fn a_more_local_layer_can_switch_a_plugin_back_on() {
        // Lowest precedence first, same order the settings resolver is given.
        let on = r#"{"enabledPlugins": {"document-skills@anthropic-agent-skills": true}}"#;
        let map = enabled_plugins(&[SETTINGS, on]);
        assert_eq!(
            map.get("document-skills@anthropic-agent-skills"),
            Some(&true)
        );
    }

    #[test]
    fn a_malformed_record_reports_its_parse_error_rather_than_reading_as_no_plugins() {
        let v = plugin_roots("{ not json", &all_enabled());
        assert!(v.roots.is_empty());
        assert_eq!(v.errors.len(), 1, "{:?}", v.errors);
        assert!(
            v.errors[0].contains("installed_plugins.json"),
            "{:?}",
            v.errors
        );
    }

    #[test]
    fn a_record_that_parses_but_has_no_plugins_object_still_says_why_it_gave_nothing() {
        let v = plugin_roots(r#"{"version": 2}"#, &all_enabled());
        assert!(v.roots.is_empty());
        assert_eq!(v.errors.len(), 1, "{:?}", v.errors);
    }

    #[test]
    fn a_skill_is_named_and_described_by_its_frontmatter() {
        let text =
            "---\nname: deploy-rpm\ndescription: Install an RPM on Matt's machines\n---\n\nbody\n";
        assert_eq!(
            frontmatter(text),
            (
                "deploy-rpm".to_string(),
                "Install an RPM on Matt's machines".to_string()
            )
        );
    }

    #[test]
    fn a_skill_with_no_frontmatter_still_gets_a_row() {
        // A skill that exists is worth listing even if it is undocumented.
        assert_eq!(frontmatter("just a body"), (String::new(), String::new()));
    }

    #[test]
    fn a_description_running_past_the_frontmatter_is_not_swallowed_whole() {
        let text = "---\nname: a\ndescription: one\n---\ndescription: two\n";
        assert_eq!(frontmatter(text).1, "one");
    }
}
