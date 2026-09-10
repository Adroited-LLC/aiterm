pub(crate) fn apply_workaround() -> bool {
    if std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_some() {
        return false;
    }

    let wayland_expected = match std::env::var_os("GDK_BACKEND") {
        Some(value) => value == "wayland",
        None => {
            std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
                || std::env::var_os("WAYLAND_SOCKET").is_some_and(|value| !value.is_empty())
        }
    };

    if !wayland_expected {
        return false;
    }

    if !std::path::Path::new("/sys/module/nvidia").exists() {
        return false;
    }

    std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
    true
}
