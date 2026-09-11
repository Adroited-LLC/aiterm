//! Startup-only Linux graphics compatibility. Read before GTK or worker threads start.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
struct Preferences {
    nvidia_wayland_compatibility: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            nvidia_wayland_compatibility: false,
        }
    }
}

pub(crate) struct GraphicsState {
    preferences: Mutex<Preferences>,
    startup_enabled: bool,
    pub(crate) injected: bool,
    environment_override: bool,
}

#[derive(Serialize)]
pub(crate) struct GraphicsSettings {
    supported: bool,
    enabled: bool,
    active: bool,
    environment_override: bool,
    restart_required: bool,
}

fn settings_path() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|p| p.join("aiterm/graphics.json"))
        .ok_or("App data directory unavailable".into())
}
fn load(path: &Path) -> Preferences {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
fn save(path: &Path, preferences: &Preferences) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid graphics settings path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(".graphics-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(preferences).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

// GDK tries backend lists in order. Do not mistake a Wayland fallback after an
// available X11 backend for a native Wayland launch. Explicit Wayland can also
// use the default socket without WAYLAND_DISPLAY being set.
fn expects_wayland(backend: Option<&str>, wayland_available: bool, x11_available: bool) -> bool {
    let Some(backend) = backend else {
        return wayland_available;
    };
    for name in backend.split(',').map(str::trim) {
        match name {
            "wayland" => return true,
            "x11" if x11_available => return false,
            "*" => return wayland_available,
            _ => {}
        }
    }
    false
}
fn should_inject(
    enabled: bool,
    supported: bool,
    nvidia: bool,
    wayland: bool,
    overridden: bool,
) -> bool {
    enabled && supported && nvidia && wayland && !overridden
}
fn present(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

impl GraphicsState {
    pub(crate) fn initialize() -> Self {
        let preferences = settings_path().map(|p| load(&p)).unwrap_or_default();
        let environment_override = std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_some();
        let backend = std::env::var("GDK_BACKEND").ok();
        let injected = should_inject(
            preferences.nvidia_wayland_compatibility,
            cfg!(target_os = "linux"),
            Path::new("/sys/module/nvidia").exists(),
            expects_wayland(
                backend.as_deref(),
                present("WAYLAND_DISPLAY") || present("WAYLAND_SOCKET"),
                present("DISPLAY"),
            ),
            environment_override,
        );
        // Called once at the start of run(), before GTK and the async runtime.
        if injected {
            std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
        }
        Self {
            startup_enabled: preferences.nvidia_wayland_compatibility,
            preferences: Mutex::new(preferences),
            injected,
            environment_override,
        }
    }
    fn snapshot(&self, preferences: &Preferences) -> GraphicsSettings {
        GraphicsSettings {
            supported: cfg!(target_os = "linux"),
            enabled: preferences.nvidia_wayland_compatibility,
            active: self.injected,
            environment_override: self.environment_override,
            restart_required: preferences.nvidia_wayland_compatibility != self.startup_enabled,
        }
    }
    fn set_at(&self, path: &Path, enabled: bool) -> Result<GraphicsSettings, String> {
        let mut preferences = self.preferences.lock().map_err(|e| e.to_string())?;
        let next = Preferences {
            nvidia_wayland_compatibility: enabled,
        };
        save(path, &next)?;
        *preferences = next;
        Ok(self.snapshot(&preferences))
    }
}

#[tauri::command]
pub(crate) fn graphics_settings(
    state: tauri::State<'_, GraphicsState>,
) -> Result<GraphicsSettings, String> {
    let preferences = state.preferences.lock().map_err(|e| e.to_string())?;
    Ok(state.snapshot(&preferences))
}
#[tauri::command]
pub(crate) fn graphics_settings_set(
    enabled: bool,
    state: tauri::State<'_, GraphicsState>,
) -> Result<GraphicsSettings, String> {
    if !cfg!(target_os = "linux") {
        return Err("This setting is only available on Linux".into());
    }
    state.set_at(&settings_path()?, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backend_lists_follow_gtk_order_and_keep_forced_x11_out() {
        for (backend, wl, x11, expected) in [
            (None, true, true, true),
            (None, false, true, false),
            (Some("wayland"), false, true, true),
            (Some("x11"), true, true, false),
            (Some("wayland,x11"), true, true, true),
            (Some("x11,wayland"), true, true, false),
            (Some("x11,wayland"), true, false, true),
            (Some("*"), true, true, true),
            (Some("*"), false, true, false),
            (Some(" wayland, x11,*"), true, true, true),
        ] {
            assert_eq!(expects_wayland(backend, wl, x11), expected, "{backend:?}");
        }
    }
    #[test]
    fn opt_out_other_platforms_other_gpus_and_environment_overrides_disable_injection() {
        assert!(should_inject(true, true, true, true, false));
        for (enabled, supported, nvidia, wayland, overridden) in [
            (false, true, true, true, false),
            (true, false, true, true, false),
            (true, true, false, true, false),
            (true, true, true, false, false),
            (true, true, true, true, true),
        ] {
            assert!(!should_inject(
                enabled, supported, nvidia, wayland, overridden
            ));
        }
    }
    #[test]
    fn preference_persists_without_changing_current_run_and_can_be_reverted() {
        let directory =
            std::env::temp_dir().join(format!("aiterm-graphics-{}", uuid::Uuid::new_v4()));
        let path = directory.join("graphics.json");
        assert!(!load(&path).nvidia_wayland_compatibility);
        let state = GraphicsState {
            preferences: Mutex::new(Preferences {
                nvidia_wayland_compatibility: true,
            }),
            startup_enabled: true,
            injected: true,
            environment_override: false,
        };
        let updated = state.set_at(&path, false).unwrap();
        assert!(!updated.enabled);
        assert!(updated.active && updated.restart_required);
        assert!(!load(&path).nvidia_wayland_compatibility);
        assert!(!state.set_at(&path, true).unwrap().restart_required);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn failed_save_keeps_previous_preference() {
        let path =
            std::env::temp_dir().join(format!("aiterm-graphics-file-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "not a directory").unwrap();
        let state = GraphicsState {
            preferences: Mutex::new(Preferences::default()),
            startup_enabled: false,
            injected: false,
            environment_override: true,
        };
        assert!(state.set_at(&path.join("graphics.json"), true).is_err());
        let snapshot = state.snapshot(&state.preferences.lock().unwrap());
        assert!(!snapshot.enabled && snapshot.environment_override);
        assert!(!snapshot.restart_required);
        std::fs::remove_file(path).unwrap();
    }
}
