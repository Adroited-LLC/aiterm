//! Skills available to a Claude session, and which tree each came from.
//!
//! User and project skills are one directory each. Plugin skills are not:
//! the plugin cache keeps several versions of the same plugin side by side
//! (`document-skills` is here at three version hashes), so globbing it reports
//! every skill two or three times. `installed_plugins.json` records an
//! `installPath` per installed plugin, which is the only non-guess available.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// "user", "project", or the plugin's name.
    pub source: String,
    pub path: String,
}

/// `(label, skills directory)` for every installed plugin, from the record that
/// names the live version.
pub fn plugin_roots(installed_plugins_json: &str) -> Vec<(String, String)> {
    let Ok(root) = serde_json::from_str::<Value>(installed_plugins_json) else {
        return Vec::new();
    };
    let Some(plugins) = root.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, entries) in plugins {
        // "superpowers@claude-plugins-official" reads better as "superpowers".
        let label = id.split('@').next().unwrap_or(id).to_string();
        let Some(first) = entries.as_array().and_then(|a| a.first()) else { continue };
        let Some(path) = first.get("installPath").and_then(Value::as_str) else { continue };
        out.push((label, format!("{path}/skills")));
    }
    out.sort();
    out
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

    #[test]
    fn a_plugins_skills_directory_comes_from_its_recorded_install_path() {
        // The cache holds three versions of document-skills. Globbing it would
        // list every skill three times; installed_plugins.json names the live
        // one, so nothing is guessed.
        let roots = plugin_roots(INSTALLED);
        assert!(roots.iter().any(|(label, dir)| label == "superpowers"
            && dir == "/h/.claude/plugins/cache/claude-plugins-official/superpowers/6.2.0/skills"));
        assert_eq!(roots.len(), 2, "one root per installed plugin, not per cached version");
    }

    #[test]
    fn a_plugin_with_no_install_path_is_skipped_rather_than_guessed() {
        let roots = plugin_roots(r#"{"plugins": {"broken@x": [{"scope": "user"}]}}"#);
        assert!(roots.is_empty());
    }

    #[test]
    fn a_malformed_record_yields_no_roots_rather_than_failing_the_panel() {
        assert!(plugin_roots("{ not json").is_empty());
    }

    #[test]
    fn a_skill_is_named_and_described_by_its_frontmatter() {
        let text = "---\nname: deploy-rpm\ndescription: Install an RPM on Matt's machines\n---\n\nbody\n";
        assert_eq!(
            frontmatter(text),
            ("deploy-rpm".to_string(), "Install an RPM on Matt's machines".to_string())
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
