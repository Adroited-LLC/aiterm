//! Desktop notifications — the popup a waiting session raises when aiterm is
//! behind another window.
//!
//! `org.freedesktop.Notifications` is the one surface here that needs nothing
//! from the desktop: GNOME and Plasma both implement it natively, unlike the
//! taskbar count (`taskbar.rs`) and the tray menu (`tray.rs`), which lean on
//! Unity's launcher API and StatusNotifierItem respectively.
//!
//! Emitted with `gdbus`, matching how the launcher badge is sent. The message
//! text is a session's own words, so unlike the badge's bare number it is real
//! input — it is passed as an explicitly quoted GVariant string rather than
//! left for gdbus to coerce, because a body that happens to read as `[]` or
//! `@i 5` would otherwise be parsed as something other than text.

use std::process::Command;

const BUS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// A notification body is one or two lines in a corner of the screen. Anything
/// past this is not going to be read there anyway, and a very long prompt
/// should not stretch the popup across the display.
const MAX_BODY: usize = 180;

/// Render a Rust string as a GVariant text-format string, quotes and all.
///
/// Newlines and tabs become spaces rather than escapes: notification daemons
/// vary in how they lay out multi-line bodies, and a session's prompt is one
/// sentence worth reading, not a paragraph worth formatting.
pub fn gvariant_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// The arguments after `gdbus call`, built as a list so nothing goes near a
/// shell. `replaces` of 0 posts a new notification; anything else updates that
/// one in place, which is what keeps a session that asks twice from stacking up
/// two popups.
pub fn notify_args(summary: &str, body: &str, replaces: u32) -> Vec<String> {
    vec![
        "call".into(),
        "--session".into(),
        "--dest".into(),
        BUS.into(),
        "--object-path".into(),
        PATH.into(),
        "--method".into(),
        format!("{BUS}.Notify"),
        gvariant_string("aiterm"),
        format!("@u {replaces}"),
        gvariant_string("aiterm"),
        gvariant_string(&truncate(summary, 80)),
        gvariant_string(&truncate(body, MAX_BODY)),
        "@as []".into(),
        // desktop-entry lets the shell attribute the popup to aiterm and group
        // it with the app rather than showing it as an anonymous message.
        "@a{sv} {'desktop-entry': <'aiterm'>, 'urgency': <byte 1>}".into(),
        "@i -1".into(),
    ]
}

/// `gdbus` prints the daemon's reply as `(uint32 7,)`.
pub fn parse_id(out: &str) -> Option<u32> {
    let start = out.find("uint32")? + "uint32".len();
    out[start..]
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|d| !d.is_empty())
        .and_then(|d| d.parse().ok())
}

/// Raise (or with `replaces`, update) a notification. Returns the daemon's id
/// so it can be taken down again when the session stops waiting — a popup still
/// on screen for a prompt already answered is worse than none.
#[tauri::command]
pub fn desktop_notify(app: tauri::AppHandle, summary: String, body: String, replaces: u32) -> Option<u32> {
    // A phone should hear about a waiting session the same moment the desktop does.
    crate::remote::notify(&app, crate::remote::Event::Attention { title: summary.clone(), body: body.clone() });
    let out = Command::new("gdbus").args(notify_args(&summary, &body, replaces)).output().ok()?;
    parse_id(&String::from_utf8_lossy(&out.stdout))
}

/// Take a notification down. Best-effort: a daemon that already expired it
/// reports no error worth surfacing.
#[tauri::command]
pub fn desktop_notify_close(id: u32) {
    let _ = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            BUS,
            "--object-path",
            PATH,
            "--method",
            &format!("{BUS}.CloseNotification"),
            &format!("@u {id}"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_quoted() {
        assert_eq!(gvariant_string("hello"), "\"hello\"");
    }

    #[test]
    fn quotes_and_backslashes_are_escaped_not_dropped() {
        assert_eq!(gvariant_string(r#"say "hi" \ ok"#), r#""say \"hi\" \\ ok""#);
    }

    #[test]
    fn newlines_become_spaces_rather_than_breaking_the_literal() {
        assert_eq!(gvariant_string("a\nb\tc"), "\"a b c\"");
    }

    #[test]
    fn a_body_that_looks_like_a_variant_is_still_text() {
        // The whole reason bodies are quoted rather than passed bare.
        assert_eq!(gvariant_string("@i 5"), "\"@i 5\"");
        assert_eq!(gvariant_string("[]"), "\"[]\"");
    }

    #[test]
    fn a_long_body_is_cut_rather_than_filling_the_screen() {
        let long = "x".repeat(500);
        let args = notify_args("t", &long, 0);
        let body = &args[12];
        assert!(body.chars().count() < MAX_BODY + 4, "{}", body.chars().count());
        assert!(body.ends_with("…\""));
    }

    #[test]
    fn replacing_reuses_the_daemons_id_instead_of_stacking_popups() {
        assert_eq!(notify_args("t", "b", 0)[9], "@u 0");
        assert_eq!(notify_args("t", "b", 42)[9], "@u 42");
    }

    #[test]
    fn the_reply_is_read_back_as_an_id() {
        assert_eq!(parse_id("(uint32 7,)\n"), Some(7));
        assert_eq!(parse_id("(uint32 1051,)"), Some(1051));
    }

    #[test]
    fn a_reply_that_is_not_an_id_yields_nothing() {
        // A daemon that refused, or gdbus printing an error, must not be read
        // as notification zero and later "closed".
        assert_eq!(parse_id("Error: no such service"), None);
        assert_eq!(parse_id(""), None);
    }
}
