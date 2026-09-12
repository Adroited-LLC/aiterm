//! Startup-only Linux graphics compatibility. Read before GTK or worker threads start.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GraphicsMode {
    #[default]
    Automatic,
    On,
    Off,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(from = "StoredPreferences")]
struct Preferences {
    mode: GraphicsMode,
}

// A missing setting gets automatic hardware detection. A previously saved
// boolean was an explicit user choice: never turn an old Off into Automatic.
#[derive(Default, Deserialize)]
#[serde(default)]
struct StoredPreferences {
    mode: Option<GraphicsMode>,
    nvidia_wayland_compatibility: Option<bool>,
}
impl From<StoredPreferences> for Preferences {
    fn from(stored: StoredPreferences) -> Self {
        Self {
            mode: stored
                .mode
                .unwrap_or(match stored.nvidia_wayland_compatibility {
                    Some(true) => GraphicsMode::On,
                    Some(false) => GraphicsMode::Off,
                    None => GraphicsMode::Automatic,
                }),
        }
    }
}

pub(crate) struct GraphicsState {
    preferences: Mutex<Preferences>,
    startup_mode: GraphicsMode,
    eligible: bool,
    pub(crate) injected: bool,
    environment_override: bool,
}

#[derive(Serialize)]
pub(crate) struct GraphicsSettings {
    supported: bool,
    mode: GraphicsMode,
    eligible: bool,
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
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Preferences {
            mode: GraphicsMode::Off,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Preferences::default(),
        // An unreadable or corrupt saved choice must not silently opt someone in.
        Err(_) => Preferences {
            mode: GraphicsMode::Off,
        },
    }
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
    mode: GraphicsMode,
    supported: bool,
    nvidia: bool,
    wayland: bool,
    overridden: bool,
) -> bool {
    mode != GraphicsMode::Off && supported && nvidia && wayland && !overridden
}
fn present(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

impl GraphicsState {
    pub(crate) fn initialize() -> Self {
        let preferences = settings_path().map(|p| load(&p)).unwrap_or_default();
        let environment_override = std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_some();
        let backend = std::env::var("GDK_BACKEND").ok();
        let supported = cfg!(target_os = "linux");
        let nvidia = Path::new("/sys/module/nvidia").exists();
        let wayland = expects_wayland(
            backend.as_deref(),
            present("WAYLAND_DISPLAY") || present("WAYLAND_SOCKET"),
            present("DISPLAY"),
        );
        let eligible = supported && nvidia && wayland;
        let injected = should_inject(
            preferences.mode,
            supported,
            nvidia,
            wayland,
            environment_override,
        );
        // Called once at the start of run(), before GTK and the async runtime.
        if injected {
            std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
        }
        Self {
            startup_mode: preferences.mode,
            eligible,
            preferences: Mutex::new(preferences),
            injected,
            environment_override,
        }
    }
    fn snapshot(&self, preferences: &Preferences) -> GraphicsSettings {
        GraphicsSettings {
            supported: cfg!(target_os = "linux"),
            mode: preferences.mode,
            eligible: self.eligible,
            active: self.injected,
            environment_override: self.environment_override,
            restart_required: preferences.mode != self.startup_mode,
        }
    }
    fn set_at(&self, path: &Path, mode: GraphicsMode) -> Result<GraphicsSettings, String> {
        let mut preferences = self.preferences.lock().map_err(|e| e.to_string())?;
        let next = Preferences { mode };
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
    mode: GraphicsMode,
    state: tauri::State<'_, GraphicsState>,
) -> Result<GraphicsSettings, String> {
    if !cfg!(target_os = "linux") {
        return Err("This setting is only available on Linux".into());
    }
    state.set_at(&settings_path()?, mode)
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
    fn automatic_and_on_are_limited_to_nvidia_wayland_without_overrides() {
        use GraphicsMode::{Automatic, Off, On};
        for (mode, supported, nvidia, wayland, overridden, expected) in [
            (Automatic, true, true, true, false, true),
            (On, true, true, true, false, true),
            (Off, true, true, true, false, false),
            (Automatic, true, false, true, false, false),
            (Automatic, true, true, false, false, false),
            (Automatic, false, true, true, false, false),
            (Automatic, true, true, true, true, false),
            (On, true, false, true, false, false),
            (On, true, true, false, false, false),
            (On, false, true, true, false, false),
            (On, true, true, true, true, false),
        ] {
            assert_eq!(should_inject(mode, supported, nvidia, wayland, overridden), expected,
                "{mode:?}, supported={supported}, nvidia={nvidia}, wayland={wayland}, override={overridden}");
        }
    }

    #[test]
    fn new_settings_default_to_automatic_but_legacy_explicit_choices_survive() {
        for (json, expected) in [
            ("{}", GraphicsMode::Automatic),
            (r#"{"nvidia_wayland_compatibility":true}"#, GraphicsMode::On),
            (
                r#"{"nvidia_wayland_compatibility":false}"#,
                GraphicsMode::Off,
            ),
            (
                r#"{"mode":"automatic","nvidia_wayland_compatibility":false}"#,
                GraphicsMode::Automatic,
            ),
            (r#"{"mode":"off"}"#, GraphicsMode::Off),
        ] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert_eq!(preferences.mode, expected);
        }
    }
    fn state(mode: GraphicsMode) -> GraphicsState {
        GraphicsState {
            preferences: Mutex::new(Preferences { mode }),
            startup_mode: mode,
            eligible: true,
            injected: mode != GraphicsMode::Off,
            environment_override: false,
        }
    }
    #[test]
    fn all_modes_persist_without_changing_the_running_graphics_stack() {
        let directory =
            std::env::temp_dir().join(format!("aiterm-graphics-{}", uuid::Uuid::new_v4()));
        let path = directory.join("graphics.json");
        assert_eq!(load(&path).mode, GraphicsMode::Automatic);
        let state = state(GraphicsMode::Automatic);
        for mode in [GraphicsMode::Off, GraphicsMode::On, GraphicsMode::Automatic] {
            let updated = state.set_at(&path, mode).unwrap();
            assert_eq!(load(&path).mode, mode);
            assert_eq!(updated.mode, mode);
            assert!(updated.active && updated.eligible);
            assert_eq!(updated.restart_required, mode != GraphicsMode::Automatic);
        }
        std::fs::write(&path, "broken settings").unwrap();
        assert_eq!(load(&path).mode, GraphicsMode::Off);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn failed_save_keeps_previous_preference() {
        let path =
            std::env::temp_dir().join(format!("aiterm-graphics-file-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "not a directory").unwrap();
        let state = state(GraphicsMode::Off);
        assert!(state
            .set_at(&path.join("graphics.json"), GraphicsMode::Automatic)
            .is_err());
        let snapshot = state.snapshot(&state.preferences.lock().unwrap());
        assert_eq!(snapshot.mode, GraphicsMode::Off);
        assert!(!snapshot.active && !snapshot.restart_required);
        std::fs::remove_file(path).unwrap();
    }
}
