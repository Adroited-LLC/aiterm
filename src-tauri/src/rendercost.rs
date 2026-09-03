//! What the renderer choice actually costs on *this* machine.
//!
//! Settings can describe the GPU/DOM trade in prose, but the size of it is
//! hardware-dependent: it lands almost entirely in the web process's CPU time,
//! and the multiplier differs per box. So the panel measures instead of
//! asserting — it samples the counters before and after a known repaint burst
//! and reports the difference.
//!
//! CPU time is read from `/proc/<pid>/stat` and is available everywhere Linux
//! is. GPU time comes from DRM `fdinfo`, which only some drivers export
//! (`drm-engine-gfx` on amdgpu), so it is optional and the UI simply omits it
//! when the driver stays quiet.

use std::fs;

use serde::Serialize;

/// One reading of the counters. Cumulative since process start — a measurement
/// is the difference between two of these, never a single one.
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// utime+stime of the web process, in milliseconds.
    pub cpu_ms: u64,
    /// GPU graphics-engine time in milliseconds, when the driver exports it.
    pub gpu_ms: Option<u64>,
    /// False when no web process could be found, which makes the reading
    /// meaningless rather than zero.
    pub ok: bool,
}

/// `utime` and `stime` are fields 14 and 15, but `comm` (field 2) is arbitrary
/// text in parentheses and may itself contain spaces or `)`. Splitting on
/// whitespace from the left is the classic way to get this wrong, so cut at the
/// *last* `)` and count from there.
fn parse_cpu_ticks(stat: &str) -> Option<u64> {
    let tail = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // tail starts at field 3 (state), so utime/stime are indices 11 and 12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// Graphics-engine nanoseconds out of a DRM `fdinfo` blob, with the client id
/// it belongs to. Several file descriptors commonly point at the same DRM
/// client; summing them all would multiply the answer by the fd count, so the
/// caller dedupes on the id.
fn parse_gfx(fdinfo: &str) -> Option<(u64, u64)> {
    let mut client = None;
    let mut gfx = None;
    for line in fdinfo.lines() {
        let (key, value) = match line.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        let value = value.trim();
        match key.trim() {
            "drm-client-id" => client = value.parse::<u64>().ok(),
            // amdgpu spells it `drm-engine-gfx`; the value carries a "ns" unit
            // suffix on some drivers, so take the leading number only.
            "drm-engine-gfx" => {
                gfx = value
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
            }
            _ => {}
        }
    }
    Some((client?, gfx?))
}

fn clock_hz() -> u64 {
    // The kernel's USER_HZ is 100 on every architecture Linux ships for
    // desktops; sysconf isn't reachable without libc bindings here.
    100
}

/// The WebKit process that actually paints, which is a child of ours. `comm` is
/// truncated to 15 characters by the kernel, which is why this matches a prefix
/// rather than the full `WebKitWebProcess`.
fn web_process_pid(parent: u32) -> Option<u32> {
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid: u32 = match name.to_str().and_then(|n| n.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !is_web_process_of(&stat, parent) {
            continue;
        }
        return Some(pid);
    }
    None
}

/// Split out so the match rule is testable without a live process tree.
fn is_web_process_of(stat: &str, parent: u32) -> bool {
    let comm_start = match stat.find('(') {
        Some(i) => i + 1,
        None => return false,
    };
    let comm_end = match stat.rfind(')') {
        Some(i) => i,
        None => return false,
    };
    if comm_start > comm_end {
        return false;
    }
    if !stat[comm_start..comm_end].starts_with("WebKitWebProces") {
        return false;
    }
    let tail = &stat[comm_end + 1..];
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // tail starts at field 3 (state), so ppid is index 1.
    fields.get(1).and_then(|p| p.parse::<u32>().ok()) == Some(parent)
}

fn gpu_ms_for(pid: u32) -> Option<u64> {
    let fds = fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    let mut seen: Vec<(u64, u64)> = Vec::new();
    for fd in fds.flatten() {
        let target = match fs::read_link(fd.path()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !target.to_string_lossy().starts_with("/dev/dri/render") {
            continue;
        }
        let name = fd.file_name();
        let info =
            match fs::read_to_string(format!("/proc/{pid}/fdinfo/{}", name.to_string_lossy())) {
                Ok(i) => i,
                Err(_) => continue,
            };
        if let Some((client, gfx)) = parse_gfx(&info) {
            if !seen.iter().any(|(c, _)| *c == client) {
                seen.push((client, gfx));
            }
        }
    }
    if seen.is_empty() {
        return None;
    }
    Some(seen.iter().map(|(_, gfx)| gfx).sum::<u64>() / 1_000_000)
}

/// Read the counters now. Two calls either side of a repaint burst give the
/// cost of that burst under whichever renderer is currently attached.
#[tauri::command]
pub fn renderer_probe() -> Probe {
    let me = std::process::id();
    let pid = match web_process_pid(me) {
        Some(p) => p,
        None => return Probe::default(),
    };
    let cpu_ms = fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| parse_cpu_ticks(&s))
        .map(|ticks| ticks * 1000 / clock_hz())
        .unwrap_or(0);
    Probe {
        cpu_ms,
        gpu_ms: gpu_ms_for(pid),
        ok: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real-shaped line, including the parenthesised comm the naive split
    /// trips over.
    const STAT: &str = "257079 (WebKitWebProces) S 256730 256730 256730 0 -1 4194560 \
        1234 0 0 0 800 740 0 0 20 0 44 0 999 0 0 0";

    #[test]
    fn cpu_time_is_user_plus_system() {
        // fields 14/15 in that line are 800 and 740.
        assert_eq!(parse_cpu_ticks(STAT), Some(1540));
    }

    #[test]
    fn a_comm_containing_spaces_and_a_paren_does_not_shift_the_fields() {
        let odd = "42 (we (ird) proc) S 7 7 7 0 -1 0 0 0 0 0 11 22 0 0 20 0 1 0 9 0 0 0";
        assert_eq!(parse_cpu_ticks(odd), Some(33));
    }

    #[test]
    fn the_web_process_is_matched_by_parent_not_by_name_alone() {
        assert!(is_web_process_of(STAT, 256730));
        // Same binary, someone else's child — another app's webview.
        assert!(!is_web_process_of(STAT, 999));
    }

    #[test]
    fn a_process_that_is_not_a_webview_never_matches() {
        let zsh = "258291 (zsh) S 256730 258291 258291 0 -1 0 0 0 0 0 5 5 0 0 20 0 1 0 9 0 0 0";
        assert!(!is_web_process_of(zsh, 256730));
    }

    #[test]
    fn gfx_time_is_read_with_the_client_that_owns_it() {
        let info = "pos:\t0\ndrm-driver:\tamdgpu\ndrm-client-id:\t78\n\
                    drm-engine-gfx:\t63033012418 ns\ndrm-engine-compute:\t187648386 ns\n";
        assert_eq!(parse_gfx(info), Some((78, 63_033_012_418)));
    }

    #[test]
    fn an_fdinfo_without_a_graphics_engine_reports_nothing() {
        // Drivers that export no engine breakdown must not read as zero cost.
        let info = "pos:\t0\ndrm-driver:\tsomething\ndrm-client-id:\t3\n";
        assert_eq!(parse_gfx(info), None);
    }

    #[test]
    fn a_probe_with_no_web_process_is_marked_not_ok() {
        // Default must never look like a real zero-cost reading.
        let p = Probe::default();
        assert!(!p.ok);
        assert_eq!(p.cpu_ms, 0);
    }
}
