//! Integration checks against real data on this machine: the Claude session
//! store in ~/.claude/projects and the aiterm repo itself.

use aiterm_lib::fsx::list_dir;
use aiterm_lib::git::{git_branches, git_log, git_repo_state, git_status};
use aiterm_lib::sessions::{ClaudeProvider, SessionProvider};

fn repo() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/..").to_string()
}

#[test]
fn scans_claude_sessions() {
    let sessions = ClaudeProvider.scan();
    assert!(!sessions.is_empty(), "expected sessions in ~/.claude/projects");
    let s = &sessions[0];
    assert_eq!(s.agent, "claude");
    assert!(!s.title.is_empty());
    assert!(s.project_path.starts_with('/'));
    // Sorted newest-first.
    assert!(sessions.windows(2).all(|w| w[0].last_active >= w[1].last_active));
}

#[test]
fn lists_directories() {
    let entries = list_dir(repo()).expect("list_dir on repo root");
    assert!(entries.iter().any(|e| e.name == "src-tauri" && e.is_dir));
    // Dirs sort before files.
    let first_file = entries.iter().position(|e| !e.is_dir).unwrap_or(entries.len());
    assert!(entries[first_file..].iter().all(|e| !e.is_dir));
}

#[test]
fn reads_git_repo() {
    let state = git_repo_state(repo());
    assert!(state.is_repo);
    assert_eq!(state.branch.as_deref(), Some("main"));

    let log = git_log(repo(), 10).expect("git_log");
    assert!(!log.is_empty());
    assert!(!log[0].short_id.is_empty());

    let branches = git_branches(repo()).expect("git_branches");
    assert!(branches.iter().any(|b| b.name == "main" && b.is_head));

    git_status(repo()).expect("git_status");
}

#[test]
fn non_repo_reports_cleanly() {
    let state = git_repo_state("/tmp".into());
    assert!(!state.is_repo);
}
