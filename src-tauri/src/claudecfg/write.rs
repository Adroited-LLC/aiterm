//! The one place in `claudecfg` that writes.
//!
//! Everything else here reads; that split is deliberate and worth keeping
//! visible, because a reader that "just needs to fix up" a file is how a
//! read-only guarantee stops being one.
//!
//! These are files every Claude session on the machine reads, so a save is
//! four steps in a fixed order: refuse if the file moved under us, refuse if
//! the new text is not a settings file, keep the old contents, and replace by
//! rename so a crash cannot leave a truncated file behind.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "camelCase")]
pub enum SaveError {
    /// The file changed since the panel read it. Claude writes settings.json
    /// itself, so this is an ordinary event, not a corruption.
    Collision,
    /// Parsed, but not an object — a settings file is a map of keys.
    NotAnObject,
    /// Did not parse; carries the reason so the editor can point at it.
    Invalid(String),
    Io(String),
}

/// Beside the file it copies, matching the `settings.json.bak-aiterm`
/// convention already in use on these machines.
pub fn backup_path(path: &str) -> String {
    format!("{path}.bak-aiterm")
}

/// Replace a settings file's contents.
///
/// `loaded_text` is the exact bytes the caller read. Empty means "there was no
/// file" — creating one is allowed, and a file existing anyway is a collision
/// like any other.
pub fn save_layer(path: &str, new_text: &str, loaded_text: &str) -> Result<(), SaveError> {
    let current = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(SaveError::Io(e.to_string())),
    };
    if current != loaded_text {
        return Err(SaveError::Collision);
    }

    let parsed: Value = serde_json::from_str(new_text)
        .map_err(|e| SaveError::Invalid(e.to_string()))?;
    if !parsed.is_object() {
        return Err(SaveError::NotAnObject);
    }

    // Before anything is replaced. A save that cannot keep the old contents is
    // one where being helpful is worse than being useless.
    if !current.is_empty() {
        std::fs::write(backup_path(path), &current)
            .map_err(|e| SaveError::Io(format!("backup failed: {e}")))?;
    }

    // Same directory, so the rename is on one filesystem and therefore atomic.
    let tmp = format!("{path}.tmp-aiterm");
    std::fs::write(&tmp, new_text).map_err(|e| SaveError::Io(e.to_string()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // Leaving this behind would put a stray settings.json.tmp-aiterm next
        // to the real file, which reads as aiterm having half-broken something.
        let _ = std::fs::remove_file(&tmp);
        return Err(SaveError::Io(e.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real files, in a temp directory. The rule is that aiterm never writes
    /// *Claude's* files unbidden — not that tests cannot write at all, and a
    /// writer this consequential is not worth testing through a fake.
    fn scratch(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("aiterm-write-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json").to_string_lossy().to_string()
    }

    #[test]
    fn a_clean_save_replaces_the_contents() {
        let p = scratch("clean");
        std::fs::write(&p, r#"{"model":"opus"}"#).unwrap();
        save_layer(&p, r#"{"model":"sonnet"}"#, r#"{"model":"opus"}"#).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"{"model":"sonnet"}"#);
    }

    #[test]
    fn the_previous_contents_are_kept_as_a_backup() {
        let p = scratch("backup");
        std::fs::write(&p, r#"{"model":"opus"}"#).unwrap();
        save_layer(&p, r#"{"model":"sonnet"}"#, r#"{"model":"opus"}"#).unwrap();
        assert_eq!(std::fs::read_to_string(backup_path(&p)).unwrap(), r#"{"model":"opus"}"#);
    }

    #[test]
    fn a_file_that_changed_since_it_was_read_refuses_and_is_left_alone() {
        // Claude writes settings.json itself, so this is the real case.
        let p = scratch("collision");
        std::fs::write(&p, r#"{"model":"haiku"}"#).unwrap();
        let err = save_layer(&p, r#"{"model":"sonnet"}"#, r#"{"model":"opus"}"#).unwrap_err();
        assert!(matches!(err, SaveError::Collision), "{err:?}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"{"model":"haiku"}"#);
    }

    #[test]
    fn a_reformat_by_someone_else_still_counts_as_a_change() {
        // Byte comparison, not parsed equality: refusing a save the user can
        // retry beats silently discarding another writer's edit.
        let p = scratch("reformat");
        std::fs::write(&p, "{\n  \"model\": \"opus\"\n}").unwrap();
        let err = save_layer(&p, r#"{"model":"sonnet"}"#, r#"{"model":"opus"}"#).unwrap_err();
        assert!(matches!(err, SaveError::Collision), "{err:?}");
    }

    #[test]
    fn invalid_json_refuses_and_leaves_the_file_untouched() {
        let p = scratch("invalid");
        std::fs::write(&p, r#"{"model":"opus"}"#).unwrap();
        let err = save_layer(&p, "{ not json", r#"{"model":"opus"}"#).unwrap_err();
        assert!(matches!(err, SaveError::Invalid(_)), "{err:?}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"{"model":"opus"}"#);
    }

    #[test]
    fn valid_json_that_is_not_an_object_refuses() {
        // A settings file is an object. An array parses fine and is still wrong.
        let p = scratch("array");
        std::fs::write(&p, r#"{"a":1}"#).unwrap();
        let err = save_layer(&p, "[1,2,3]", r#"{"a":1}"#).unwrap_err();
        assert!(matches!(err, SaveError::NotAnObject), "{err:?}");
    }

    #[test]
    fn a_layer_that_does_not_exist_yet_can_be_created() {
        let p = scratch("create");
        save_layer(&p, r#"{"model":"opus"}"#, "").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"{"model":"opus"}"#);
    }

    #[test]
    fn a_file_that_appeared_where_none_was_expected_is_a_collision() {
        let p = scratch("appeared");
        std::fs::write(&p, r#"{"someone":"else"}"#).unwrap();
        let err = save_layer(&p, r#"{"model":"opus"}"#, "").unwrap_err();
        assert!(matches!(err, SaveError::Collision), "{err:?}");
    }

    #[test]
    fn the_backup_name_sits_beside_the_file_it_copies() {
        assert_eq!(backup_path("/h/.claude/settings.json"), "/h/.claude/settings.json.bak-aiterm");
    }

    #[test]
    fn a_failed_replace_does_not_leave_a_temp_file_behind() {
        // The rename cannot succeed when the target's parent is a file, which
        // is a real enough failure to pin the cleanup without fault injection.
        let dir = std::env::temp_dir().join("aiterm-write-test-norename");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&dir);
        std::fs::write(&dir, "i am a file, not a directory").unwrap();
        let path = dir.join("settings.json").to_string_lossy().to_string();
        let err = save_layer(&path, r#"{"a":1}"#, "").unwrap_err();
        assert!(matches!(err, SaveError::Io(_)), "{err:?}");
        assert!(
            !std::path::Path::new(&format!("{path}.tmp-aiterm")).exists(),
            "temp file was left behind"
        );
    }
}
