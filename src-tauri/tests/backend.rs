//! Integration checks against real data on this machine: the Claude session
//! store in ~/.claude/projects and the aiterm repo itself.

use aiterm_lib::fsx::list_dir;
use aiterm_lib::git::{
    git_branch_files, git_branch_log, git_branches, git_log, git_repo_state, git_status,
};
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
    // Every commit except the root has parents recorded (graph edges).
    assert!(log[..log.len() - 1].iter().all(|c| !c.parents.is_empty()));

    let branches = git_branches(repo()).expect("git_branches");
    assert!(branches.iter().any(|b| b.name == "main" && b.is_head));

    git_status(repo()).expect("git_status");

    // Branch structure browsing: root tree has src-tauri/, subtree lists lib.rs.
    let files = git_branch_files(repo(), "main".into(), "".into()).expect("branch files");
    assert!(files.iter().any(|f| f.name == "src-tauri" && f.is_dir));
    let sub = git_branch_files(repo(), "main".into(), "src-tauri/src".into()).expect("subtree");
    assert!(sub.iter().any(|f| f.name == "lib.rs" && !f.is_dir));
    let blog = git_branch_log(repo(), "main".into(), 5).expect("branch log");
    assert!(!blog.is_empty() && !blog[0].summary.is_empty());
}

#[test]
fn non_repo_reports_cleanly() {
    let state = git_repo_state("/tmp".into());
    assert!(!state.is_repo);
}

#[test]
fn lists_projects_dir() {
    let projects = aiterm_lib::fsx::list_projects();
    assert!(projects.iter().any(|p| p.name == "aiterm" && p.is_git));
    assert!(projects.iter().any(|p| p.name == "toponet"));
}

#[test]
fn artifacts_parse_from_transcripts() {
    // Across all real sessions, artifact parsing must not choke and should
    // find at least one Write/Edit somewhere (this build session guarantees it).
    let total: usize = ClaudeProvider
        .scan()
        .iter()
        .map(|s| aiterm_lib::sessions::session_artifacts(s.id.clone()).len())
        .sum();
    assert!(total > 0, "expected some artifacts across sessions");
}

#[test]
fn preview_returns_conversation_tail() {
    // Any listed session with recent activity should yield preview text; the
    // messages must be non-empty, role-tagged, and oldest-first.
    let sessions = ClaudeProvider.scan();
    let with_msgs = sessions
        .iter()
        .find_map(|s| {
            let p = aiterm_lib::sessions::session_preview(s.id.clone());
            (!p.is_empty()).then_some(p)
        })
        .expect("at least one session should have preview messages");
    assert!(with_msgs.len() <= 12);
    for m in &with_msgs {
        assert!(m.role == "user" || m.role == "assistant");
        assert!(!m.text.trim().is_empty());
    }
}

#[test]
fn deletes_session_transcript() {
    // Synthetic session file in a scratch project dir — never touches real data.
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join(".claude/projects/-aiterm-delete-test");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "00000000-0000-4000-8000-aitermdelete";
    let file = dir.join(format!("{id}.jsonl"));
    std::fs::write(&file, "{\"type\":\"user\",\"cwd\":\"/tmp\"}\n").unwrap();

    aiterm_lib::sessions::session_delete(id.into()).expect("delete should succeed");
    assert!(!file.exists(), "transcript should be gone");
    assert!(
        aiterm_lib::sessions::session_delete("../../etc/passwd".into()).is_err(),
        "path traversal must be rejected"
    );
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn full_text_search_finds_sessions() {
    let r = aiterm_lib::indexer::reindex_sessions();
    assert!(r.total > 0, "expected sessions to index");
    let hits = aiterm_lib::indexer::search_sessions("aiterm".into());
    assert!(!hits.is_empty(), "searching 'aiterm' should hit this session");
}
