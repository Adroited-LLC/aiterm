//! A count on the taskbar icon, for sessions waiting while aiterm is behind
//! another window.
//!
//! Not Tauri's `set_badge_count`: on Linux that goes through libunity, which it
//! dlopens and then gates on Unity actually running. Neither is true on a KDE
//! or GNOME desktop, so the call succeeds and does nothing. What Plasma (and
//! Unity's descendants, and GNOME with an extension) actually listen for is the
//! `com.canonical.Unity.LauncherEntry.Update` signal on the session bus, which
//! is a plain D-Bus emission any process can make. Verified live on Plasma
//! before this was written: the icon took the number.
//!
//! Emitted by shelling out to `gdbus` rather than linking a D-Bus crate — the
//! same trade as reading OpenCode's database with `sqlite3` and talking to
//! OpenRouter with `curl`. The payload carries no user text, only a number, so
//! there is nothing here that could be made to escape the argument.

use std::process::Command;

/// The desktop file this badge attaches to. Derived from the crate name so a
/// rename cannot silently detach the badge from the installed
/// `/usr/share/applications/<name>.desktop`.
const DESKTOP_ID: &str = concat!("application://", env!("CARGO_PKG_NAME"), ".desktop");

const OBJECT_PATH: &str = concat!(
    "/com/canonical/unity/launcherentry/",
    env!("CARGO_PKG_NAME")
);

/// The GVariant dictionary Plasma reads. Zero hides the badge rather than
/// drawing a "0", which would be a worse lie than showing nothing.
pub fn payload(count: u32) -> String {
    if count == 0 {
        "{'count': <int64 0>, 'count-visible': <false>}".to_string()
    } else {
        format!("{{'count': <int64 {count}>, 'count-visible': <true>}}")
    }
}

/// Set (or with 0, clear) the number on the taskbar icon.
///
/// Best-effort: a desktop that ignores the signal, or a machine with no
/// `gdbus`, is not an error worth surfacing — the in-app indicator is the one
/// that has to work.
#[tauri::command]
pub fn taskbar_badge(count: u32) {
    let _ = Command::new("gdbus")
        .args([
            "emit",
            "--session",
            "--object-path",
            OBJECT_PATH,
            "--signal",
            "com.canonical.Unity.LauncherEntry.Update",
            DESKTOP_ID,
            &payload(count),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_is_shown_as_a_visible_number() {
        assert_eq!(payload(3), "{'count': <int64 3>, 'count-visible': <true>}");
    }

    #[test]
    fn zero_hides_the_badge_rather_than_drawing_a_zero() {
        let p = payload(0);
        assert!(p.contains("'count-visible': <false>"), "{p}");
    }

    #[test]
    fn the_badge_targets_the_desktop_file_that_ships_with_the_package() {
        // If the rpm installs aiterm.desktop, this must name it exactly, or
        // the signal is emitted for an application nothing on the bar matches.
        assert_eq!(DESKTOP_ID, "application://aiterm.desktop");
    }

    #[test]
    fn the_only_thing_interpolated_is_a_decimal_number() {
        // Why this can be a shell-out with no quoting worries: the sole
        // substitution is a u32, so no input can reach the argument.
        for n in [1u32, 42, u32::MAX] {
            assert_eq!(
                payload(n),
                format!("{{'count': <int64 {n}>, 'count-visible': <true>}}")
            );
        }
    }
}
