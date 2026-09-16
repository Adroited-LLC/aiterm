//! GTK chooses its decoration mode before creating the first window.
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Mutex};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
struct Preferences {
    show_app_title_bar: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            show_app_title_bar: true,
        }
    }
}

pub(crate) struct WindowAppearance {
    preferences: Mutex<Preferences>,
    startup: bool,
    supported: bool,
    environment_override: bool,
    pub(crate) injected: bool,
}

#[derive(Serialize)]
pub(crate) struct WindowAppearanceSettings {
    supported: bool,
    show_app_title_bar: bool,
    restart_required: bool,
    environment_override: bool,
}

fn settings_path() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|dir| dir.join("aiterm/window-appearance.json"))
        .ok_or_else(|| "App data directory unavailable".into())
}

impl WindowAppearance {
    pub(crate) fn initialize() -> Self {
        let preferences: Preferences = settings_path()
            .ok()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        // KDE supplies server-side decorations, including the user's frame
        // effects. Do not remove GTK's fallback on desktops that need it.
        let supported = cfg!(target_os = "linux")
            && std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .split(':')
                .any(|name| name.eq_ignore_ascii_case("KDE"));
        let environment_override = std::env::var_os("GTK_CSD").is_some();
        let injected = supported && !preferences.show_app_title_bar && !environment_override;
        if injected {
            std::env::set_var("GTK_CSD", "0");
        }
        Self {
            startup: preferences.show_app_title_bar,
            preferences: Mutex::new(preferences),
            supported,
            environment_override,
            injected,
        }
    }

    fn snapshot(&self, preferences: &Preferences) -> WindowAppearanceSettings {
        WindowAppearanceSettings {
            supported: self.supported,
            show_app_title_bar: preferences.show_app_title_bar,
            restart_required: preferences.show_app_title_bar != self.startup,
            environment_override: self.environment_override,
        }
    }
}

#[tauri::command]
pub(crate) fn window_appearance_settings(
    state: tauri::State<'_, WindowAppearance>,
) -> Result<WindowAppearanceSettings, String> {
    let preferences = state.preferences.lock().map_err(|e| e.to_string())?;
    Ok(state.snapshot(&preferences))
}

#[tauri::command]
pub(crate) fn window_appearance_settings_set(
    state: tauri::State<'_, WindowAppearance>,
    show_app_title_bar: bool,
) -> Result<WindowAppearanceSettings, String> {
    if !state.supported || state.environment_override {
        return Err(
            "Title bar preference is controlled by your desktop or launch environment.".into(),
        );
    }
    let mut preferences = state.preferences.lock().map_err(|e| e.to_string())?;
    let next = Preferences { show_app_title_bar };
    let path = settings_path()?;
    let parent = path.parent().ok_or("Invalid settings path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(".window-appearance-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    *preferences = next;
    Ok(state.snapshot(&preferences))
}
