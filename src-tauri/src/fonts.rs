//! Font discovery and installation.
//!
//! The frontend used to guess which fonts existed by rendering a probe string
//! in a canvas and comparing widths against a hardcoded candidate list. That
//! could only ever find fonts someone had thought to name, and it silently
//! offered nothing when the guess list missed. fontconfig already knows the
//! real answer, so ask it.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Serialize)]
pub struct FontFamily {
    pub name: String,
    /// Fixed-pitch per fontconfig — the set worth offering for a terminal.
    pub mono: bool,
}

/// A coding font we can install from the distro's own repositories.
#[derive(Serialize)]
pub struct FontPackage {
    /// Family name as fontconfig will report it once installed.
    pub name: String,
    pub package: String,
    pub note: String,
    pub installed: bool,
}

/// Curated because these are also the allowlist: `install_font_package` will
/// only ever hand dnf a package name that appears here, so a compromised or
/// buggy frontend cannot turn the install button into arbitrary package
/// installation.
const PACKAGES: &[(&str, &str, &str)] = &[
    ("Hack", "source-foundry-hack-fonts", "Legible at small sizes, wide language coverage"),
    ("JetBrains Mono", "jetbrains-mono-fonts", "Tall x-height, coding ligatures"),
    ("Fira Code", "fira-code-fonts", "The original programming-ligature font"),
    ("Cascadia Mono", "cascadia-mono-fonts", "Microsoft's terminal font, no ligatures"),
    ("Cascadia Code", "cascadia-code-fonts", "Cascadia with ligatures"),
    ("IBM Plex Mono", "ibm-plex-mono-fonts", "Humanist, slightly warmer than most"),
    ("Intel One Mono", "intel-one-mono-fonts", "Designed for low-vision developers"),
    ("Source Code Pro", "adobe-source-code-pro-fonts", "Adobe's, a common default"),
];

/// Extensions fontconfig can actually consume from a user font directory.
const FONT_EXTS: &[&str] = &["ttf", "otf", "ttc", "otc", "pfb"];

fn fc_families(args: &[&str]) -> Vec<String> {
    // The `\n` here must reach fc-list as a backslash and an n. fc-list does
    // its own escape processing and discards a literal newline byte, which
    // runs every family name together into one unusable line.
    let out = match Command::new("fc-list")
        .args(args)
        .arg("--format=%{family[0]}\\n")
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return vec![],
    };
    let mut names: Vec<String> = String::from_utf8_lossy(&out)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        // Emoji and icon fonts report as fixed-pitch but are useless as a
        // terminal face, and only clutter the picker.
        .filter(|l| !l.contains("Emoji"))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// Every font family installed, flagged with whether it is fixed-pitch.
#[tauri::command]
pub fn list_fonts() -> Vec<FontFamily> {
    let mono = fc_families(&[":mono"]);
    fc_families(&[])
        .into_iter()
        .map(|name| FontFamily {
            mono: mono.contains(&name),
            name,
        })
        .collect()
}

/// Which curated packages are already on the system. `rpm -q` rather than
/// checking for the family, because a package can be installed while its
/// family name differs from what we predicted.
#[tauri::command]
pub fn font_packages() -> Vec<FontPackage> {
    PACKAGES
        .iter()
        .map(|(name, package, note)| FontPackage {
            name: name.to_string(),
            package: package.to_string(),
            note: note.to_string(),
            installed: Command::new("rpm")
                .args(["-q", package])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
        })
        .collect()
}

/// Install a curated font package.
///
/// Tries `sudo -n` first and falls back to `pkexec`. On a machine with
/// passwordless sudo the install is silent; everywhere else polkit raises its
/// own graphical prompt, which is the right way for a desktop app to ask. A
/// terminal-style password prompt is the one thing that cannot work here —
/// there is no tty behind the button.
#[tauri::command]
pub fn install_font_package(package: String) -> Result<String, String> {
    if !PACKAGES.iter().any(|(_, p, _)| *p == package) {
        return Err(format!("{package} is not an offered font package"));
    }

    let run = |prog: &str, args: &[&str]| -> Result<(bool, String), String> {
        let out = Command::new(prog)
            .args(args)
            .output()
            .map_err(|e| format!("{prog}: {e}"))?;
        let msg = if out.status.success() {
            String::from_utf8_lossy(&out.stdout).to_string()
        } else {
            String::from_utf8_lossy(&out.stderr).to_string()
        };
        Ok((out.status.success(), msg))
    };

    let (ok, msg) = run("sudo", &["-n", "dnf", "install", "-y", &package])?;
    let (ok, msg) = if ok {
        (ok, msg)
    } else {
        run("pkexec", &["dnf", "install", "-y", &package])?
    };
    if !ok {
        let trimmed = msg.trim();
        return Err(if trimmed.is_empty() {
            format!("installing {package} failed")
        } else {
            trimmed.to_string()
        });
    }

    refresh_font_cache();
    Ok(package)
}

/// User font directory — no privileges needed, and fontconfig picks it up.
fn user_font_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".local/share/fonts/aiterm"))
}

/// Copy font files into the user font directory. Returns how many landed.
#[tauri::command]
pub fn install_font_files(paths: Vec<String>) -> Result<usize, String> {
    let dir = user_font_dir().ok_or_else(|| "no home directory".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let mut installed = 0usize;
    for p in &paths {
        let src = PathBuf::from(p);
        let ext = src
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !FONT_EXTS.contains(&ext.as_str()) {
            return Err(format!(
                "{} is not a font file (need {})",
                src.file_name().unwrap_or_default().to_string_lossy(),
                FONT_EXTS.join(", ")
            ));
        }
        let Some(name) = src.file_name() else {
            return Err(format!("{p} has no filename"));
        };
        std::fs::copy(&src, dir.join(name)).map_err(|e| format!("{p}: {e}"))?;
        installed += 1;
    }

    if installed > 0 {
        refresh_font_cache();
    }
    Ok(installed)
}

/// Rebuild the fontconfig cache so a just-installed family is visible without
/// restarting. Best-effort: a stale cache costs a restart, not correctness.
fn refresh_font_cache() {
    let _ = Command::new("fc-cache").arg("-f").output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_packages_outside_the_curated_list() {
        let err = install_font_package("bash".into()).unwrap_err();
        assert!(err.contains("not an offered font package"), "{err}");
    }

    #[test]
    fn every_curated_package_name_looks_like_a_font_package() {
        for (_, package, _) in PACKAGES {
            assert!(
                package.ends_with("-fonts"),
                "{package} does not look like a font package"
            );
        }
    }

    /// Guards the `--format` escape. Getting it wrong does not error — fc-list
    /// runs every family onto one line, so the picker offers exactly one
    /// nonsense entry and nothing looks broken until you open it.
    #[test]
    fn enumerates_families_one_per_line() {
        if Command::new("fc-list").arg("--version").output().is_err() {
            return; // no fontconfig on this machine — nothing to assert
        }
        let all = fc_families(&[]);
        assert!(
            all.len() > 1,
            "expected many families, got {all:?} — check the --format escape"
        );
        assert!(all.iter().all(|f| f.len() < 100), "family names ran together");
        assert!(fc_families(&[":mono"]).len() <= all.len());
    }

    #[test]
    fn refuses_non_font_files() {
        let err = install_font_files(vec!["/etc/hosts".into()]).unwrap_err();
        assert!(err.contains("not a font file"), "{err}");
    }
}
