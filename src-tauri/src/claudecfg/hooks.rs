//! Hooks are shell commands that fire at defined Claude events. They span
//! layers additively (all hooks in all present layers run), so each must carry
//! its source to explain whether editing is safe: aiterm's own hook lives in
//! aiterm's own file and must never be edited through this panel.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hook {
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    pub layer: String,
    pub is_aiterm: bool,
}

/// Parse hooks from a settings layer's value.
///
/// The shape expected is `{event: [{matcher?, hooks: [{type, command}]}]}`.
/// A value of the wrong shape at any level pushes an error naming the event,
/// rather than being skipped silently. The reason: this panel exists to explain
/// configuration, so a hooks blob it cannot read must say so rather than
/// appear empty.
///
/// `is_aiterm` is set when the command contains `aiterm_marker`. The marker
/// is what identifies aiterm's own hook, which aiterm injects through
/// `--settings` with the command `"aiterm --hook-report"`.
pub fn parse(
    layer_label: &str,
    hooks_value: &Value,
    aiterm_marker: &str,
) -> (Vec<Hook>, Vec<String>) {
    let mut hooks = Vec::new();
    let mut errors = Vec::new();

    let Some(obj) = hooks_value.as_object() else {
        errors.push("hooks must be an object".to_string());
        return (hooks, errors);
    };

    for (event, entries) in obj {
        let Some(entries_arr) = entries.as_array() else {
            errors.push(format!("{event}: must be an array"));
            continue;
        };

        for entry in entries_arr {
            let Some(entry_obj) = entry.as_object() else {
                errors.push(format!("{event}: each entry must be an object"));
                continue;
            };

            let matcher = entry_obj
                .get("matcher")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let Some(hooks_arr) = entry_obj.get("hooks").and_then(|v| v.as_array()) else {
                errors.push(format!("{event}: missing or non-array 'hooks' field"));
                continue;
            };

            for hook_obj_val in hooks_arr {
                let Some(hook_obj) = hook_obj_val.as_object() else {
                    errors.push(format!("{event}: each hook must be an object"));
                    continue;
                };

                let Some(command) = hook_obj.get("command").and_then(|v| v.as_str()) else {
                    errors.push(format!("{event}: each hook must have a 'command' field"));
                    continue;
                };

                let is_aiterm = command.contains(aiterm_marker);

                hooks.push(Hook {
                    event: event.clone(),
                    matcher: matcher.clone(),
                    command: command.to_string(),
                    layer: layer_label.to_string(),
                    is_aiterm,
                });
            }
        }
    }

    (hooks, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MARKER: &str = "--hook-report";

    #[test]
    fn one_event_with_two_hooks_yields_two_rows() {
        let v = json!({"SessionStart":[{"hooks":[
            {"type":"command","command":"a"},
            {"type":"command","command":"b"}]}]});
        let (h, _) = parse("user", &v, MARKER);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].event, "SessionStart");
    }

    #[test]
    fn a_matcher_is_carried_when_present_and_absent_when_not() {
        let v = json!({"PreToolUse":[
            {"matcher":"Bash","hooks":[{"type":"command","command":"x"}]},
            {"hooks":[{"type":"command","command":"y"}]}]});
        let (h, _) = parse("user", &v, MARKER);
        assert_eq!(h[0].matcher.as_deref(), Some("Bash"));
        assert_eq!(h[1].matcher, None);
    }

    #[test]
    fn aiterms_own_hook_is_recognised_so_it_is_not_offered_for_editing() {
        // It lives in aiterm's own --settings file by design; an editor that
        // offered to change it here would either fail or fight the writer.
        let v = json!({"SessionStart":[{"hooks":[
            {"type":"command","command":"'/usr/bin/aiterm' --hook-report"}]}]});
        let (h, _) = parse("aiterm", &v, MARKER);
        assert!(h[0].is_aiterm);
    }

    #[test]
    fn someone_elses_hook_is_not_mistaken_for_aiterms() {
        let v = json!({"SessionStart":[{"hooks":[{"type":"command","command":"echo hi"}]}]});
        let (h, _) = parse("user", &v, MARKER);
        assert!(!h[0].is_aiterm);
    }

    #[test]
    fn a_malformed_hooks_blob_is_reported_rather_than_dropped() {
        let (h, errors) = parse("user", &json!({"SessionStart":"not a list"}), MARKER);
        assert!(h.is_empty());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("SessionStart"), "{errors:?}");
    }

    #[test]
    fn a_hook_carries_the_layer_it_came_from() {
        // Hooks are additive across layers, so a row must never imply it
        // replaced another.
        let v = json!({"Stop":[{"hooks":[{"type":"command","command":"z"}]}]});
        let (h, _) = parse("project local", &v, MARKER);
        assert_eq!(h[0].layer, "project local");
    }
}
