//! Stops the window growing on every restart.
//!
//! tauri-plugin-window-state saves `inner_size()` on exit but restores with
//! `set_size()`. Under GNOME's client-side decorations those measure different
//! rectangles: `set_size` sets the content box, while `inner_size` reports the
//! GTK allocation, which also covers the titlebar and the invisible resize
//! shadow. So the window comes back bigger than what was asked for, and on the
//! next exit that inflated number is what gets saved.
//!
//! Measured on this machine across four launches, the gap was identical every
//! time — +90 wide, +129 tall — and independent of the window size:
//!
//! ```text
//! saved (2010,1339) -> settled (2100,1468)
//! saved (2100,1468) -> settled (2190,1597)
//! saved (2190,1597) -> settled (2280,1726)
//! saved (2280,1726) -> settled (2370,1855)
//! ```
//!
//! So: ask for the saved size minus that gap, and the window lands exactly on
//! the saved size, which is then what gets saved again. Stable.
//!
//! The gap is not hardcoded — it differs per desktop and theme, and is zero
//! where this bug does not exist. It is measured once the compositor has
//! settled the surface and remembered for next launch. Measuring is why the
//! first attempt at this failed: it ran in `setup()`, where the window is not
//! realized yet and reports the config defaults with an outer size of 0x0.

use std::sync::Mutex;
use tauri::{Manager, PhysicalSize};

/// Never leave the window unusably small, whatever the arithmetic says.
const MIN_SIZE: u32 = 400;

/// Frame extents are tens of pixels. A larger difference is a monitor change,
/// a scale change, or a tiling WM placing the window — none of which we should
/// be second-guessing.
const MAX_DRIFT: i64 = 400;

/// What we asked the window to be this launch, so the gap can be measured
/// against it once the surface settles.
static REQUESTED: Mutex<Option<(u32, u32)>> = Mutex::new(None);

/// The size to request so the window ends up at `saved`.
fn shrink(saved: (u32, u32), drift: (u32, u32)) -> (u32, u32) {
    (
        saved.0.saturating_sub(drift.0).max(MIN_SIZE),
        saved.1.saturating_sub(drift.1).max(MIN_SIZE),
    )
}

/// The gap between what we asked for and what we got. None when there is no
/// gap, or when it is too large to be decoration.
fn measure(requested: (u32, u32), actual: (u32, u32)) -> Option<(u32, u32)> {
    let dw = actual.0 as i64 - requested.0 as i64;
    let dh = actual.1 as i64 - requested.1 as i64;
    if dw == 0 && dh == 0 {
        return None;
    }
    if !(0..=MAX_DRIFT).contains(&dw) || !(0..=MAX_DRIFT).contains(&dh) {
        return None;
    }
    Some((dw as u32, dh as u32))
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

fn drift_path() -> Option<std::path::PathBuf> {
    let dir = dirs::data_dir()?.join("aiterm");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("window-drift.json"))
}

fn stored_drift() -> Option<(u32, u32)> {
    let raw = std::fs::read_to_string(drift_path()?).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some((
        v.get("dw")?.as_u64()? as u32,
        v.get("dh")?.as_u64()? as u32,
    ))
}

fn store_drift(drift: (u32, u32)) {
    let Some(path) = drift_path() else { return };
    let body = format!("{{\"dw\":{},\"dh\":{}}}\n", drift.0, drift.1);
    let _ = std::fs::write(path, body);
}

/// Ask for the saved size less the gap this desktop adds. Runs from `setup`,
/// which is after the plugin's own restore in `on_window_ready`, so this is
/// the last word on the size. Reads nothing from the window — at this point it
/// is not realized and every measurement it offers is a lie.
pub fn correct_restored_size(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Some(saved) = saved_size(app) else {
        return; // first ever run — nothing was restored, so nothing drifted
    };
    match stored_drift() {
        Some(drift) => {
            let want = shrink(saved, drift);
            let _ = window.set_size(PhysicalSize {
                width: want.0,
                height: want.1,
            });
            *REQUESTED.lock().unwrap() = Some(want);
        }
        // Nothing learned yet. Leave the size alone and note what the plugin
        // asked for, so the gap can be measured against it below.
        None => *REQUESTED.lock().unwrap() = Some(saved),
    }
}

/// Measure the gap once the compositor has settled, and remember it.
///
/// On the very first run there was no gap to subtract, so the window is
/// already inflated — correct it here rather than making the restart count for
/// nothing. Every later run is corrected before the window is ever shown.
pub fn learn_drift(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_maximized().unwrap_or(false) {
        return; // the plugin does not save a maximized size, so nothing drifts
    }
    let Some(requested) = *REQUESTED.lock().unwrap() else {
        return;
    };
    let Ok(actual) = window.inner_size() else {
        return;
    };
    let first_run = stored_drift().is_none();
    let Some(drift) = measure(requested, (actual.width, actual.height)) else {
        return;
    };
    store_drift(drift);

    if first_run {
        if let Some(saved) = saved_size(app) {
            let want = shrink(saved, drift);
            let _ = window.set_size(PhysicalSize {
                width: want.0,
                height: want.1,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real numbers off this machine: asking for 2280x1726 produced
    /// 2370x1855, so next time ask for 2190x1597 and land on 2280x1726.
    #[test]
    fn learns_the_gap_and_subtracts_it() {
        let drift = measure((2280, 1726), (2370, 1855)).unwrap();
        assert_eq!(drift, (90, 129));
        assert_eq!(shrink((2280, 1726), drift), (2190, 1597));
    }

    /// The whole point: after one correction the size stops moving.
    #[test]
    fn converges_after_one_launch() {
        let drift = (90u32, 129u32);
        let saved = (2280u32, 1726u32);
        let asked = shrink(saved, drift);
        let landed = (asked.0 + drift.0, asked.1 + drift.1);
        assert_eq!(landed, saved, "the size saved on exit must equal the size restored");
    }

    #[test]
    fn no_gap_means_no_correction() {
        assert_eq!(measure((1500, 950), (1500, 950)), None);
    }

    /// A monitor change is not decoration, and neither is a window a tiling WM
    /// has taken over.
    #[test]
    fn ignores_differences_too_large_to_be_decoration() {
        assert_eq!(measure((2096, 1150), (3840, 2160)), None);
    }

    /// Only ever seen the window come back *bigger*. A smaller one means
    /// something else owns the geometry, so leave it be.
    #[test]
    fn ignores_a_window_that_came_back_smaller() {
        assert_eq!(measure((1500, 950), (1400, 900)), None);
    }

    #[test]
    fn never_shrinks_below_something_usable() {
        assert_eq!(shrink((420, 420), (300, 300)), (400, 400));
    }
}
