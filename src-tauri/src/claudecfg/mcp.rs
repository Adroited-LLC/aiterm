//! MCP servers Claude has registered locally.
//!
//! Its own reader rather than a slice of the settings layers, because MCP does
//! not live in settings.json: user scope is `~/.claude.json`'s `mcpServers`,
//! project scope is a checked-in `.mcp.json`, and whether this project trusts a
//! project server is a per-project list inside `~/.claude.json`.
//!
//! Servers reached as claude.ai connectors appear in none of these files. The
//! caller must say so, or an empty list reads as "no MCP" when there is plenty.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub name: String,
    /// "user" or "project".
    pub scope: String,
    pub command: Option<String>,
    /// Only meaningful for project scope, where a project opts in per server.
    pub enabled: Option<bool>,
}

fn command_of(v: &Value) -> Option<String> {
    v.get("command")?.as_str().map(str::to_string)
}

fn names(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// `project` is the absolute path used as the key inside `~/.claude.json`.
/// Returns the servers and whether any local config could be read at all.
pub fn read(
    claude_json: Option<&str>,
    mcp_json: Option<&str>,
    project: &str,
) -> (Vec<Server>, bool) {
    let mut out = Vec::new();
    let mut any_read = false;

    let user: Option<Value> = claude_json.and_then(|t| serde_json::from_str(t).ok());
    if let Some(root) = &user {
        any_read = true;
        if let Some(map) = root.get("mcpServers").and_then(Value::as_object) {
            for (name, v) in map {
                out.push(Server {
                    name: name.clone(),
                    scope: "user".into(),
                    command: command_of(v),
                    enabled: None,
                });
            }
        }
    }

    if let Some(root) = mcp_json.and_then(|t| serde_json::from_str::<Value>(t).ok()) {
        any_read = true;
        let entry = user
            .as_ref()
            .and_then(|u| u.get("projects"))
            .and_then(|p| p.get(project))
            .cloned()
            .unwrap_or(Value::Null);
        let on = names(&entry, "enabledMcpjsonServers");
        let off = names(&entry, "disabledMcpjsonServers");
        if let Some(map) = root.get("mcpServers").and_then(Value::as_object) {
            for (name, v) in map {
                let enabled = if on.contains(name) {
                    Some(true)
                } else if off.contains(name) {
                    Some(false)
                } else {
                    None
                };
                out.push(Server {
                    name: name.clone(),
                    scope: "project".into(),
                    command: command_of(v),
                    enabled,
                });
            }
        }
    }

    (out, any_read)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_JSON: &str = r#"{
        "mcpServers": {"chrome": {"command": "claude-in-chrome"}},
        "projects": {
            "/p": {"enabledMcpjsonServers": ["repo"], "disabledMcpjsonServers": ["old"]}
        }
    }"#;

    const MCP_JSON: &str = r#"{"mcpServers": {"repo": {"command": "node server.js"}, "old": {"command": "x"}}}"#;

    #[test]
    fn a_user_scope_server_is_listed_with_its_command() {
        let (s, read) = read(Some(CLAUDE_JSON), None, "/p");
        assert!(read);
        let chrome = s.iter().find(|x| x.name == "chrome").expect("chrome");
        assert_eq!(chrome.scope, "user");
        assert_eq!(chrome.command.as_deref(), Some("claude-in-chrome"));
    }

    #[test]
    fn a_project_server_carries_whether_this_project_enabled_it() {
        let (s, _) = read(Some(CLAUDE_JSON), Some(MCP_JSON), "/p");
        let repo = s.iter().find(|x| x.name == "repo").expect("repo");
        assert_eq!(repo.scope, "project");
        assert_eq!(repo.enabled, Some(true));
        let old = s.iter().find(|x| x.name == "old").expect("old");
        assert_eq!(old.enabled, Some(false));
    }

    #[test]
    fn no_local_config_is_reported_as_read_and_empty_not_as_unread() {
        // Observed 2026-08-06: mcpServers is empty here and there is no
        // .mcp.json, while sessions plainly have MCP tools — those are
        // claude.ai connectors, in no local file. "Empty" and "could not read"
        // must not look the same, or the panel implies there is no MCP at all.
        let (s, read) = read(Some(r#"{"mcpServers": {}}"#), None, "/p");
        assert!(read);
        assert!(s.is_empty());
    }

    #[test]
    fn an_unreadable_config_is_not_reported_as_empty() {
        let (s, read) = read(None, None, "/p");
        assert!(!read);
        assert!(s.is_empty());
    }

    #[test]
    fn a_malformed_config_does_not_take_the_other_source_with_it() {
        let (s, read) = read(Some("{ broken"), Some(MCP_JSON), "/p");
        assert!(read, "the project file was still readable");
        assert!(s.iter().any(|x| x.name == "repo"));
    }
}
