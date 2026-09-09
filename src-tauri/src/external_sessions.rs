//! Live ownership, not transcript origin: a closed Codex Desktop conversation
//! can still be resumed in AITerm. Only its currently held rollouts are marked.
use std::collections::HashMap;

pub fn owners() -> HashMap<String, String> {
    #[cfg(target_os = "linux")]
    return owners_in(std::path::Path::new("/proc"));
    #[cfg(not(target_os = "linux"))]
    HashMap::new()
}

pub fn ensure_available(session_id: &str) -> Result<(), String> {
    match owners().get(session_id) {
        Some(owner) => Err(format!(
            "This session is open in {owner}. Close it there before resuming or deleting it in AITerm."
        )),
        None => Ok(()),
    }
}

#[cfg(target_os = "linux")]
fn owners_in(proc_root: &std::path::Path) -> HashMap<String, String> {
    let mut owners = HashMap::new();
    let Ok(processes) = std::fs::read_dir(proc_root) else {
        return owners;
    };
    for process in processes.flatten().take(32768) {
        if !process
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let root = process.path();
        let Ok(exe) = std::fs::read_link(root.join("exe")) else {
            continue;
        };
        // The desktop bundles its app-server here; a standalone CLI named
        // codex is not evidence of ownership by the desktop app.
        if !exe.ends_with("resources/codex") {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(root.join("fd")) else {
            continue;
        };
        for fd in fds.flatten().take(16384) {
            let Ok(path) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name
                .strip_prefix("rollout-")
                .and_then(|n| n.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let Some(id) = stem.get(stem.len().saturating_sub(36)..) else {
                continue;
            };
            if uuid::Uuid::parse_str(id).is_ok() {
                owners.insert(id.to_owned(), "Codex Desktop".to_owned());
            }
        }
    }
    owners
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn only_desktop_held_rollouts_are_marked_and_release_clears_ownership() {
        let root = std::env::temp_dir().join(format!("aiterm-external-{}", uuid::Uuid::new_v4()));
        let id = "01a081a2-396c-7f50-8d8c-ab294d481809";
        let cli_id = "01a081c7-1303-7d83-b262-0ecf1eda90c4";
        for (pid, exe, session) in [
            ("123", "/usr/lib/chatgpt/resources/codex", id),
            ("456", "/home/matt/.local/bin/codex", cli_id),
        ] {
            let process = root.join(pid);
            std::fs::create_dir_all(process.join("fd")).unwrap();
            symlink(exe, process.join("exe")).unwrap();
            symlink(
                format!("/sessions/rollout-2026-09-08T11-28-07-{session}.jsonl"),
                process.join("fd/5"),
            )
            .unwrap();
        }
        symlink("/sessions/rollout-invalid.jsonl", root.join("123/fd/6")).unwrap();
        symlink("/sessions/unrelated.jsonl", root.join("123/fd/7")).unwrap();
        assert_eq!(
            owners_in(&root),
            HashMap::from([(id.to_owned(), "Codex Desktop".to_owned())])
        );
        std::fs::remove_file(root.join("123/fd/5")).unwrap();
        assert!(owners_in(&root).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
        assert!(owners_in(&root).is_empty());
    }
}
