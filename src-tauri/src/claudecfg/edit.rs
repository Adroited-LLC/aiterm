//! Applying a single key's edit to a settings file.
//!
//! The edit goes onto the *parsed original*, which is then re-serialised with
//! `preserve_order`. It is never rebuilt from a list of known settings — the
//! panel shows the union of keys actually present precisely so it cannot hide
//! one, and a save that dropped what it did not recognise would give that back.

use serde_json::{Map, Value};

/// Set `dotted_key` to `value` in `original`, returning the new file text.
///
/// Missing intermediate objects are created. A path running through a
/// non-object is refused rather than overwriting it: replacing a scalar with a
/// map to make room for a nested key destroys a value nobody asked to change.
pub fn set_key(original: &str, dotted_key: &str, value: Value) -> Result<String, String> {
    let mut root: Value = if original.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(original).map_err(|e| e.to_string())?
    };
    if !root.is_object() {
        return Err("not a JSON object".into());
    }

    let parts: Vec<&str> = dotted_key.split('.').collect();
    let mut cursor = &mut root;
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let map = match cursor {
            Value::Object(m) => m,
            _ => return Err(format!("{} is not an object", parts[..i].join("."))),
        };
        if last {
            map.insert((*part).to_string(), value);
            break;
        }
        cursor = map
            .entry((*part).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !cursor.is_object() {
            return Err(format!("{} is not an object", parts[..=i].join(".")));
        }
    }

    // Pretty, because a human edits this file too and a one-line settings.json
    // is a hostile thing to hand back.
    serde_json::to_string_pretty(&root).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_scalar_is_replaced_in_place() {
        let out = set_key(r#"{"model":"opus"}"#, "model", json!("sonnet")).unwrap();
        assert_eq!(out.replace([' ', '\n'], ""), r#"{"model":"sonnet"}"#);
    }

    #[test]
    fn a_key_the_editor_does_not_understand_survives_the_save() {
        // The whole reason this applies an edit to the parsed original instead
        // of rebuilding from a schema.
        let out = set_key(
            r#"{"model":"opus","worktree":{"bgIsolation":"none"}}"#,
            "model",
            json!("sonnet"),
        )
        .unwrap();
        assert!(out.contains("bgIsolation"), "{out}");
    }

    #[test]
    fn key_order_is_preserved() {
        // serde_json's preserve_order feature; a save that reshuffled a user's
        // file would make every diff unreadable.
        let out = set_key(r#"{"z":1,"a":2}"#, "a", json!(3)).unwrap();
        assert!(
            out.find("\"z\"").unwrap() < out.find("\"a\"").unwrap(),
            "{out}"
        );
    }

    #[test]
    fn a_nested_key_is_reached_through_its_path() {
        let out = set_key(
            r#"{"permissions":{"deny":["a"]}}"#,
            "permissions.deny",
            json!(["b"]),
        )
        .unwrap();
        assert!(out.contains("\"b\""), "{out}");
        assert!(!out.contains("\"a\""), "{out}");
    }

    #[test]
    fn a_missing_intermediate_object_is_created() {
        let out = set_key("{}", "permissions.deny", json!(["x"])).unwrap();
        assert!(out.contains("permissions"), "{out}");
        assert!(out.contains("deny"), "{out}");
    }

    #[test]
    fn a_path_running_through_a_scalar_is_refused_rather_than_clobbering_it() {
        // "model.nested" when model is a string: overwriting would silently
        // destroy a value the user did not ask to change.
        let err = set_key(r#"{"model":"opus"}"#, "model.nested", json!(1)).unwrap_err();
        assert!(err.contains("model"), "{err}");
    }

    #[test]
    fn an_original_that_is_not_json_is_refused() {
        assert!(set_key("{ broken", "model", json!("x")).is_err());
    }

    #[test]
    fn an_empty_original_starts_from_an_object() {
        let out = set_key("", "model", json!("opus")).unwrap();
        assert!(out.contains("model"), "{out}");
    }
}
