//! A tray icon whose menu is one row per waiting session.
//!
//! The taskbar icon cannot do this. Plasma takes an icon's menu entries from
//! the `Actions=` list in the installed .desktop file, which is fixed at
//! package time — and the Unity LauncherEntry "quicklist" that Unity used for
//! dynamic entries is the one part of that API Plasma does not implement. A
//! StatusNotifierItem, which is what Tauri's tray is, owns its menu at runtime
//! and can rebuild it as often as it likes. So the count goes on the taskbar
//! icon (see `taskbar.rs`) and the list goes here.

use serde::Deserialize;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

/// One waiting session, as the frontend sees it.
#[derive(Debug, Clone, Deserialize)]
pub struct TrayAlert {
    /// Tab key — round-tripped through the menu id so a click can say which.
    pub key: i64,
    pub title: String,
    pub message: Option<String>,
}

const TRAY_ID: &str = "alerts";
/// Menu ids are `alert:<tab key>`; the prefix is what tells a click on a
/// session apart from a click on "Nothing waiting".
const PREFIX: &str = "alert:";

/// One row's text. Kept apart from the menu building so the shape can be
/// checked without a display server.
pub fn row_label(a: &TrayAlert) -> String {
    match a.message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        // The title alone is rarely enough to choose between two waiting
        // sessions; the sentence is the part that decides.
        Some(m) => format!("{} — {}", a.title, ellipsis(m, 60)),
        None => format!("{} — waiting for input", a.title),
    }
}

/// Menus are a poor place for a paragraph, and some of these messages are long.
fn ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Parse a menu id back to the tab it came from.
pub fn key_of(id: &str) -> Option<i64> {
    id.strip_prefix(PREFIX)?.parse().ok()
}

/// Create the tray at startup, with an empty list.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("window icon".into()))?;
    let menu = Menu::with_items(app, &[&MenuItem::with_id(app, "idle", "Nothing waiting", false, None::<&str>)?])?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("aiterm")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let Some(key) = key_of(event.id.as_ref()) else { return };
            // Raise the window first: picking a session from the tray means
            // going to it, and it is behind something or the tray would not
            // have been the way you reached for it.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("tray-alert", key);
        })
        .build(app)?;
    Ok(())
}

/// Replace the menu with the current list of waiting sessions.
#[tauri::command]
pub fn tray_alerts(app: AppHandle, alerts: Vec<TrayAlert>) -> Result<(), String> {
    let tray = app.tray_by_id(TRAY_ID).ok_or("no tray icon")?;
    let items: Vec<MenuItem<_>> = if alerts.is_empty() {
        vec![MenuItem::with_id(&app, "idle", "Nothing waiting", false, None::<&str>)
            .map_err(|e| e.to_string())?]
    } else {
        alerts
            .iter()
            .map(|a| {
                MenuItem::with_id(&app, format!("{PREFIX}{}", a.key), row_label(a), true, None::<&str>)
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<_, _>>()?
    };
    let refs: Vec<&dyn tauri::menu::IsMenuItem<_>> =
        items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<_>).collect();
    let menu = Menu::with_items(&app, &refs).map_err(|e| e.to_string())?;
    tray.set_menu(Some(menu)).map_err(|e| e.to_string())?;
    tray.set_tooltip(Some(&match alerts.len() {
        0 => "aiterm".to_string(),
        1 => "aiterm — 1 session waiting".to_string(),
        n => format!("aiterm — {n} sessions waiting"),
    }))
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(key: i64, title: &str, message: Option<&str>) -> TrayAlert {
        TrayAlert { key, title: title.into(), message: message.map(Into::into) }
    }

    #[test]
    fn a_row_carries_the_sentence_that_decides_between_sessions() {
        assert_eq!(
            row_label(&alert(3, "aiterm", Some("Permission to run git push"))),
            "aiterm — Permission to run git push"
        );
    }

    #[test]
    fn a_session_that_only_rang_the_bell_still_gets_a_row() {
        assert_eq!(row_label(&alert(1, "mojo", None)), "mojo — waiting for input");
    }

    #[test]
    fn an_empty_message_is_treated_as_no_message() {
        assert_eq!(row_label(&alert(1, "mojo", Some("   "))), "mojo — waiting for input");
    }

    #[test]
    fn a_long_message_is_cut_rather_than_stretching_the_menu() {
        let long = "x".repeat(200);
        let row = row_label(&alert(1, "t", Some(&long)));
        assert!(row.chars().count() < 80, "{} chars", row.chars().count());
        assert!(row.ends_with('…'));
    }

    #[test]
    fn a_click_maps_back_to_the_tab_it_came_from() {
        assert_eq!(key_of("alert:42"), Some(42));
    }

    #[test]
    fn the_idle_row_is_not_mistaken_for_a_session() {
        // It is disabled, but an id that parsed would still be a bug waiting.
        assert_eq!(key_of("idle"), None);
        assert_eq!(key_of("alert:notanumber"), None);
    }
}
