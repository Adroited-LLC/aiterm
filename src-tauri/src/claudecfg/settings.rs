//! The layered settings, resolved.
//!
//! Claude merges several files, most local winning. The panel needs more than
//! the winner — "project overrides user" is the useful sentence — so every
//! layer that sets a key is carried through.

use serde::Serialize;
use serde_json::{Map, Value};

/// Ordered lowest-precedence first. `resolve` relies on that order rather than
/// on a comparison, so adding a layer means putting it in the right place here.
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
}

/// Walk an object into dotted leaves. Arrays are leaves: `permissions.deny` is
/// a list of rules the user recognises, and `permissions.deny.0` is not.
fn flatten(prefix: &str, map: &Map<String, Value>, out: &mut Vec<(String, Value)>) {
    for (k, v) in map {
        let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match v {
            Value::Object(inner) if !inner.is_empty() => flatten(&key, inner, out),
            _ => out.push((key, v.clone())),
        }
    }
}

/// Resolve layers given lowest-precedence first. Returns the settings and one
/// error string per layer that existed but did not parse.
pub fn resolve(layers: &[(LayerId, &str)]) -> (Vec<Setting>, Vec<String>) {
    let mut errors = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut found: std::collections::HashMap<String, Vec<SetIn>> = std::collections::HashMap::new();

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
        flatten("", map, &mut leaves);
        for (key, value) in leaves {
            if !found.contains_key(&key) {
                order.push(key.clone());
            }
            found.entry(key).or_default().push(SetIn { layer: *id, value });
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
}
