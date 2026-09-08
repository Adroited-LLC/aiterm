use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A GUI launch may omit the user's local bin directories. Respect PATH's
/// ordering first, then check stable install locations without sourcing an rc
/// file or searching version-manager caches for an arbitrary installed version.
pub(super) fn find_executable(
    bin: &str,
    path: Option<&OsStr>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = path
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .collect();
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
    }
    dirs.extend(["/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    dirs.into_iter()
        .map(|dir| dir.join(bin))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(unix)]
pub(super) fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub(super) fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "aiterm-discovery-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn executable(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn native_claude_symlink_is_found_with_a_gui_path_or_no_path() {
        let f = Fixture::new();
        let target = f.executable(".local/share/claude/versions/test");
        let link = f.0.join(".local/bin/claude");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(target, &link).unwrap();
        assert_eq!(
            find_executable(
                "claude",
                Some(f.0.join("empty-path").as_os_str()),
                Some(&f.0)
            ),
            Some(link.clone())
        );
        assert_eq!(find_executable("claude", None, Some(&f.0)), Some(link));
    }

    #[test]
    fn explicit_path_order_wins_over_standard_install_locations() {
        let f = Fixture::new();
        let first = f.executable("custom/claude");
        f.executable("second/claude");
        f.executable(".local/bin/claude");
        let path = std::env::join_paths([f.0.join("custom"), f.0.join("second")]).unwrap();
        assert_eq!(
            find_executable("claude", Some(&path), Some(&f.0)),
            Some(first)
        );
    }

    #[test]
    fn skips_non_executable_files_directories_and_broken_links() {
        let f = Fixture::new();
        let name = "aiterm-fixture-cli";
        let blocked = f.executable(".local/bin/aiterm-fixture-cli");
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_executable(name, None, Some(&f.0)), None);
        std::fs::remove_file(&blocked).unwrap();
        std::fs::create_dir(&blocked).unwrap();
        assert_eq!(find_executable(name, None, Some(&f.0)), None);
        std::fs::remove_dir(&blocked).unwrap();
        symlink(f.0.join("missing"), &blocked).unwrap();
        assert_eq!(find_executable(name, None, Some(&f.0)), None);
        let fallback = f.executable("bin/aiterm-fixture-cli");
        assert_eq!(find_executable(name, None, Some(&f.0)), Some(fallback));
    }

    #[test]
    fn missing_path_and_home_can_still_find_system_programs() {
        assert!(find_executable("sh", None, None).is_some());
        assert!(find_executable("aiterm-fixture-not-installed", None, None).is_none());
    }
}
