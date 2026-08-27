//! The layered settings, resolved.
//!
//! Claude merges several files, most local winning. The panel needs more than
//! the winner — "project overrides user" is the useful sentence — so every
//! layer that sets a key is carried through.

use serde::Serialize;
use serde_json::{Map, Value};

/// Precedence is not encoded here. `resolve` takes the layers as a slice and
/// trusts *the caller's* order — lowest precedence first — so the invariant
/// lives in `layer_paths`, not in how this enum is declared. Reordering these
/// variants changes nothing; reordering that slice changes everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LayerId {
    /// `~/.claude/settings.json`
    User,
    /// `<project>/.claude/settings.json`
    Project,
    /// `<project>/.claude/settings.local.json`
    ProjectLocal,
    /// aiterm's own file, passed with `--settings`, which sits at CLI level.
    Injected,
}

impl LayerId {
    pub fn label(self) -> &'static str {
        match self {
            LayerId::User => "user",
            LayerId::Project => "project",
            LayerId::ProjectLocal => "project local",
            LayerId::Injected => "aiterm",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub id: LayerId,
    pub path: String,
    pub present: bool,
    /// Why this layer could not be used, when it exists but did not parse.
    pub error: Option<String>,
    /// The exact bytes read, empty when the file is absent.
    ///
    /// Serves two jobs at once on purpose: the raw editor's initial content,
    /// and the token a save is checked against. Reading the file twice would
    /// invite the two to disagree.
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetIn {
    pub layer: LayerId,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    /// Dotted path — `permissions.deny`, not a nested tree, so the UI is a list.
    pub key: String,
    pub concern: String,
    pub effective: Value,
    pub winner: LayerId,
    /// Lowest-precedence first, so the last entry is always the winner.
    pub set_in: Vec<SetIn>,
    /// The lower layers are not losers: Claude collects this key from all of
    /// them. Calling that "overridden" would be a misreport.
    pub merged: bool,
    /// True when some segment on this key's own path contained a literal `.`
    /// — a JSON key that is itself dotted, e.g. an MCP server named
    /// `docs.search`. `key` joins path segments with `.` and nothing escapes
    /// one that was already there, so the joined string cannot be split back
    /// into the path it came from: `mcpServers.docs.search.command` reads as
    /// four levels when it is really two. Editing such a key through
    /// `edit::set_key` (which does exactly that split) would build a bogus
    /// branch instead of touching the real key, so the panel must refuse to
    /// offer it inline rather than invent an escaping scheme.
    pub ambiguous: bool,
}

/// Key roots Claude collects from *every* source instead of letting the most
/// local one win: the `permissions` rule lists and the `hooks` tree.
///
/// aiterm's own feature depends on that additivity — `hooklink` injects a
/// `SessionStart` hook through `--settings` precisely because it is added to
/// the user's hooks rather than replacing them (see the module docs there). So
/// the day a user writes their own `SessionStart` hook, both run, and a panel
/// that said "overridden" would be describing behaviour that does not happen.
const ADDITIVE_ROOTS: &[&str] = &["permissions", "hooks"];

/// Whether Claude will apply every layer's value for this key rather than only
/// the winner's.
///
/// Root membership alone is not enough: `permissions.defaultMode` lives under
/// an additive root but is a single mode, and the most local one really does
/// win. Only the collections merge, so the values must be collections — which
/// after `flatten` means arrays.
fn is_merged(key: &str, set_in: &[SetIn]) -> bool {
    let root = key.split('.').next().unwrap_or(key);
    ADDITIVE_ROOTS.contains(&root)
        && set_in.len() > 1
        && set_in.iter().all(|s| s.value.is_array())
}

/// Walk an object into dotted leaves. Arrays are leaves: `permissions.deny` is
/// a list of rules the user recognises, and `permissions.deny.0` is not.
///
/// `prefix_ambiguous` carries whether an ancestor segment already contained a
/// literal `.`; a leaf is ambiguous if that is true or its own key `k` is. The
/// check happens here, while `k` is still the real un-joined segment name —
/// once it is folded into the joined `key` string there is no way to tell a
/// key that legitimately contains a dot from a path that merely has several
/// segments.
fn flatten(prefix: &str, prefix_ambiguous: bool, map: &Map<String, Value>, out: &mut Vec<(String, Value, bool)>) {
    for (k, v) in map {
        let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        let ambiguous = prefix_ambiguous || k.contains('.');
        match v {
            Value::Object(inner) if !inner.is_empty() => flatten(&key, ambiguous, inner, out),
            _ => out.push((key, v.clone(), ambiguous)),
        }
    }
}

/// Resolve layers given lowest-precedence first. Returns the settings and one
/// error string per layer that existed but did not parse.
pub fn resolve(layers: &[(LayerId, &str)]) -> (Vec<Setting>, Vec<String>) {
    let mut errors = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut found: std::collections::HashMap<String, Vec<SetIn>> = std::collections::HashMap::new();
    // A key's ambiguity comes from its own path shape, not from which layer
    // set it or what value it holds — so one map, keyed the same as `found`,
    // is enough even though a key can be set in several layers.
    let mut ambiguous: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

    for (id, text) in layers {
        let parsed: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("{}: {e}", id.label()));
                continue;
            }
        };
        let Some(map) = parsed.as_object() else {
            errors.push(format!("{}: not a JSON object", id.label()));
            continue;
        };
        let mut leaves = Vec::new();
        flatten("", false, map, &mut leaves);
        for (key, value, is_ambiguous) in leaves {
            if !found.contains_key(&key) {
                order.push(key.clone());
            }
            found.entry(key.clone()).or_default().push(SetIn { layer: *id, value });
            ambiguous.insert(key, is_ambiguous);
        }
    }

    let settings = order
        .into_iter()
        .map(|key| {
            let set_in = found.remove(&key).unwrap_or_default();
            let last = set_in.last().expect("a key exists because a layer set it");
            Setting {
                concern: super::concern::of(&key).to_string(),
                effective: last.value.clone(),
                winner: last.layer,
                merged: is_merged(&key, &set_in),
                ambiguous: ambiguous.get(&key).copied().unwrap_or(false),
                key,
                set_in,
            }
        })
        .collect();

    (settings, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = r#"{"model": "claude-opus-5", "cleanupPeriodDays": 30}"#;
    const PROJECT: &str = r#"{"model": "sonnet", "worktree": {"bgIsolation": "none"}}"#;
    const LOCAL: &str = r#"{"model": "haiku"}"#;

    fn layered() -> Vec<(LayerId, &'static str)> {
        vec![
            (LayerId::User, USER),
            (LayerId::Project, PROJECT),
            (LayerId::ProjectLocal, LOCAL),
        ]
    }

    fn find<'a>(s: &'a [Setting], key: &str) -> &'a Setting {
        s.iter().find(|x| x.key == key).expect(key)
    }

    #[test]
    fn the_most_local_layer_wins() {
        let (s, _) = resolve(&layered());
        assert_eq!(find(&s, "model").winner, LayerId::ProjectLocal);
        assert_eq!(find(&s, "model").effective, serde_json::json!("haiku"));
    }

    #[test]
    fn every_layer_that_set_a_key_is_reported_not_just_the_winner() {
        // "project overrides user" is the display this exists for.
        let (s, _) = resolve(&layered());
        let layers: Vec<_> = find(&s, "model").set_in.iter().map(|x| x.layer).collect();
        assert_eq!(layers, vec![LayerId::User, LayerId::Project, LayerId::ProjectLocal]);
    }

    #[test]
    fn a_key_only_one_layer_sets_still_appears() {
        let (s, _) = resolve(&layered());
        assert_eq!(find(&s, "cleanupPeriodDays").winner, LayerId::User);
        assert_eq!(find(&s, "worktree.bgIsolation").winner, LayerId::Project);
    }

    #[test]
    fn an_injected_layer_outranks_the_project() {
        // aiterm's own --settings file sits at CLI level, above project files.
        let mut l = layered();
        l.push((LayerId::Injected, r#"{"model": "fable"}"#));
        let (s, _) = resolve(&l);
        assert_eq!(find(&s, "model").winner, LayerId::Injected);
    }

    #[test]
    fn a_malformed_layer_reports_its_error_and_leaves_the_rest_readable() {
        // The panel is most likely opened when a file is broken.
        let (s, errors) = resolve(&[
            (LayerId::User, USER),
            (LayerId::Project, "{ this is not json"),
        ]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("project"), "{errors:?}");
        assert_eq!(find(&s, "model").winner, LayerId::User);
    }

    #[test]
    fn a_key_claude_collects_from_every_layer_is_reported_as_merged_not_overridden() {
        // permissions.deny is concatenated across sources, so both lists are in
        // force. aiterm relies on the same additivity for its SessionStart hook.
        let (s, _) = resolve(&[
            (LayerId::User, r#"{"permissions": {"deny": ["Bash(rm:*)"]}}"#),
            (LayerId::Project, r#"{"permissions": {"deny": ["Read(.env)"]}}"#),
        ]);
        assert!(find(&s, "permissions.deny").merged);
    }

    #[test]
    fn a_key_the_most_local_layer_simply_wins_is_not_reported_as_merged() {
        let (s, _) = resolve(&layered());
        assert!(!find(&s, "model").merged);
    }

    #[test]
    fn a_single_setter_under_an_additive_root_is_not_called_merged() {
        // Nothing to merge with; "merged, all apply" would be noise.
        let (s, _) = resolve(&[(LayerId::User, r#"{"hooks": {"SessionStart": [{"x": 1}]}}"#)]);
        assert!(!find(&s, "hooks.SessionStart").merged);
    }

    #[test]
    fn a_scalar_under_an_additive_root_still_reports_as_overridden() {
        // permissions.defaultMode is one mode, not a list Claude concatenates —
        // the most local layer really does win, root membership notwithstanding.
        let (s, _) = resolve(&[
            (LayerId::User, r#"{"permissions": {"defaultMode": "ask"}}"#),
            (LayerId::Project, r#"{"permissions": {"defaultMode": "auto"}}"#),
        ]);
        assert!(!find(&s, "permissions.defaultMode").merged);
    }

    #[test]
    fn nested_objects_are_flattened_to_dotted_keys() {
        let (s, _) = resolve(&[(LayerId::User, r#"{"permissions": {"deny": ["Bash"]}}"#)]);
        assert_eq!(find(&s, "permissions.deny").effective, serde_json::json!(["Bash"]));
    }

    #[test]
    fn a_leaf_array_is_a_value_not_a_branch_to_walk_into() {
        // permissions.deny is a list of rules; walking into it would produce
        // permissions.deny.0 and lose the shape the user recognises.
        let (s, _) = resolve(&[(LayerId::User, r#"{"a": {"b": [1, 2]}}"#)]);
        assert!(s.iter().all(|x| x.key != "a.b.0"), "{:?}", s.iter().map(|x| &x.key).collect::<Vec<_>>());
    }

    #[test]
    fn a_literal_dot_in_a_key_name_marks_the_setting_ambiguous() {
        // "docs.search" is one MCP server name, not two path segments — but
        // the dotted key this produces, mcpServers.docs.search.command, reads
        // as four. `edit::set_key` would split it back into four and build a
        // bogus branch, so the panel must know not to route this key through
        // it.
        let (s, _) = resolve(&[(
            LayerId::User,
            r#"{"mcpServers": {"docs.search": {"command": "x"}}}"#,
        )]);
        assert!(find(&s, "mcpServers.docs.search.command").ambiguous);
    }

    #[test]
    fn an_ordinary_nested_key_is_not_ambiguous() {
        let (s, _) = resolve(&[(LayerId::User, r#"{"permissions": {"deny": ["Bash"]}}"#)]);
        assert!(!find(&s, "permissions.deny").ambiguous);
    }
}
