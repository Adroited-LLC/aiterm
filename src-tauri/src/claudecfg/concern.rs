//! Which group of the panel a setting belongs in.
//!
//! A lookup on the key's root rather than a schema of every setting Claude
//! supports. `ClaudeBackend::models()` already carries a comment about
//! hardcoded Claude knowledge ageing; a settings schema would age the same way,
//! and its failure mode is worse — quietly omitting a key that is in effect.
//! Anything unrecognised is shown under "Other", which is plain but never a lie.

const GROUPS: &[(&str, &str)] = &[
    ("model", "Model"),
    ("fallbackModel", "Model"),
    ("permissions", "Permissions"),
    ("defaultMode", "Permissions"),
    ("additionalDirectories", "Permissions"),
    ("hooks", "Hooks"),
    ("env", "Environment"),
    ("apiKeyHelper", "Environment"),
    ("mcpServers", "MCP"),
    ("enableAllProjectMcpServers", "MCP"),
    ("enabledMcpjsonServers", "MCP"),
    ("disabledMcpjsonServers", "MCP"),
    ("preferredNotifChannel", "Notifications & UI"),
    ("statusLine", "Notifications & UI"),
    ("outputStyle", "Notifications & UI"),
    ("theme", "Notifications & UI"),
    ("cleanupPeriodDays", "Housekeeping"),
    ("includeCoAuthoredBy", "Housekeeping"),
];

/// The order groups appear in the panel. "Other" last: it is the overflow, and
/// a reader should meet the settings we can explain first.
pub const ORDER: &[&str] = &[
    "Model",
    "Permissions",
    "Hooks",
    "Environment",
    "MCP",
    "Notifications & UI",
    "Housekeeping",
    "Other",
];

pub fn of(key: &str) -> &'static str {
    let root = key.split('.').next().unwrap_or(key);
    GROUPS
        .iter()
        .find(|(k, _)| *k == root)
        .map(|(_, group)| *group)
        .unwrap_or("Other")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keys_land_in_their_group() {
        assert_eq!(of("model"), "Model");
        assert_eq!(of("permissions.deny"), "Permissions");
        assert_eq!(of("hooks.SessionStart"), "Hooks");
        assert_eq!(of("env.FOO"), "Environment");
        assert_eq!(of("mcpServers.chrome"), "MCP");
        assert_eq!(of("preferredNotifChannel"), "Notifications & UI");
        assert_eq!(of("cleanupPeriodDays"), "Housekeeping");
    }

    #[test]
    fn a_key_we_have_never_heard_of_is_shown_rather_than_hidden() {
        // The failure mode a hardcoded schema has: silently omitting a setting
        // that is genuinely in effect.
        assert_eq!(of("worktree.bgIsolation"), "Other");
        assert_eq!(of("somethingClaudeAddedLastTuesday"), "Other");
    }

    #[test]
    fn grouping_reads_the_root_of_a_dotted_key() {
        // permissions.deny and permissions.allow are one concern, not two.
        assert_eq!(of("permissions.allow"), of("permissions.deny"));
    }
}
