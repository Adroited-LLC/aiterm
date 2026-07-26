//! Stops the window growing a little taller on every restart.
//!
//! tauri-plugin-window-state saves `inner_size()` on exit but restores with
//! `set_size()`. Those two do not measure the same rectangle under GNOME's
//! client-side decorations on Wayland: the window comes back as the saved size
//! *plus* its titlebar, and on the next exit that inflated number is what gets
//! saved. Repeat, and the window walks off the bottom of the screen.
//!
//! Rather than hardcode a decoration height — which differs per desktop, per
//! theme, and is zero on the platforms that never had the bug — measure it.
//! Ask for the saved size, see what we actually got, and take the difference
//! back off. A no-op wherever the two already agree.

use tauri::{Manager, PhysicalSize};

/// Anything larger than this is not decoration drift — a monitor was
/// unplugged, the display scale changed, a tiling WM overrode us. Leave those
/// alone rather than fight whoever really owns the geometry. A titlebar is
/// 35-50px and shadows add a little either side, so this is generous already;
/// it was 200 until a test pointed out that admits a 176px width change.
const MAX_DRIFT: i64 = 120;

/// The size to ask for so that the window ends up at `saved`, given that
/// asking for `saved` produced `actual`. None when no correction is wanted.
fn compensate(saved: (u32, u32), actual: (u32, u32)) -> Option<(u32, u32)> {
    let (sw, sh) = (saved.0 as i64, saved.1 as i64);
    let (aw, ah) = (actual.0 as i64, actual.1 as i64);
    let (dw, dh) = (aw - sw, ah - sh);
    if dw == 0 && dh == 0 {
        return None;
    }
    if dw.abs() > MAX_DRIFT || dh.abs() > MAX_DRIFT {
        return None;
    }
    // Want x such that x + drift == saved.
    Some((((sw - dw).max(200)) as u32, ((sh - dh).max(200)) as u32))
}

fn saved_size(app: &tauri::AppHandle) -> Option<(u32, u32)> {
    let path = app.path().app_config_dir().ok()?.join(".window-state.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let main = v.get("main")?;
    Some((
        main.get("width")?.as_u64()? as u32,
        main.get("height")?.as_u64()? as u32,
    ))
}

/// Undo the restore-time drift, once, at startup.
pub fn correct_restored_size(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Some(saved) = saved_size(app) else {
        return; // first run — nothing was restored, so nothing drifted
    };
    let Ok(actual) = window.inner_size() else {
        return;
    };
    let Some((width, height)) = compensate(saved, (actual.width, actual.height)) else {
        return;
    };
    let _ = window.set_size(PhysicalSize { width, height });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_for_less_when_the_window_came_back_bigger() {
        // Saved 1150, got 1187 — a 37px titlebar. Ask for 1113 so it lands on
        // 1150, which is then what gets saved, so the file stops drifting.
        assert_eq!(compensate((2096, 1150), (2096, 1187)), Some((2096, 1113)));
    }

    #[test]
    fn asks_for_more_when_the_window_came_back_smaller() {
        assert_eq!(compensate((800, 600), (800, 580)), Some((800, 620)));
    }

    #[test]
    fn does_nothing_when_the_size_is_already_right() {
        assert_eq!(compensate((1500, 950), (1500, 950)), None);
    }

    /// A monitor change is not drift, and second-guessing it would fight the
    /// window manager over a window it legitimately resized.
    #[test]
    fn ignores_differences_too_large_to_be_decoration() {
        assert_eq!(compensate((2096, 1150), (1920, 1080)), None);
        assert_eq!(compensate((800, 600), (800, 2000)), None);
    }

    #[test]
    fn never_returns_a_uselessly_tiny_window() {
        // Drift within the threshold, but subtracting it would leave 110px.
        let (w, h) = compensate((210, 210), (310, 310)).unwrap();
        assert!(w >= 200 && h >= 200, "{w}x{h}");
    }
}
