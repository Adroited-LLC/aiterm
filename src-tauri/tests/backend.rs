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

/// The commands below are `async` now: each hands its real work to the blocking
/// pool so Tauri never runs it on the GTK main thread. Tests drive them through
/// the same runtime Tauri would, which keeps them testing the command itself
/// rather than a private twin of it.
fn call<T>(f: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(f)
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
    let entries = call(list_dir(repo())).expect("list_dir on repo root");
    assert!(entries.iter().any(|e| e.name == "src-tauri" && e.is_dir));
    // Dirs sort before files.
    let first_file = entries.iter().position(|e| !e.is_dir).unwrap_or(entries.len());
    assert!(entries[first_file..].iter().all(|e| !e.is_dir));
}

#[test]
fn reads_git_repo() {
    let state = call(git_repo_state(repo()));
    assert!(state.is_repo);
    // Don't hardcode the branch name — this suite must pass on feature
    // branches too. Just require a non-empty current branch.
    let head_branch = state.branch.clone().expect("current branch");
    assert!(!head_branch.is_empty());

    let log = call(git_log(repo(), 10)).expect("git_log");
    assert!(!log.is_empty());
    assert!(!log[0].short_id.is_empty());
    // Every commit except the root has parents recorded (graph edges).
    assert!(log[..log.len() - 1].iter().all(|c| !c.parents.is_empty()));

    let branches = call(git_branches(repo())).expect("git_branches");
    // The reported HEAD branch is the one flagged is_head in the list.
    assert!(branches.iter().any(|b| b.name == head_branch && b.is_head));

    call(git_status(repo())).expect("git_status");

    // Branch structure browsing: root tree has src-tauri/, subtree lists lib.rs.
    let files = call(git_branch_files(repo(), "main".into(), "".into())).expect("branch files");
    assert!(files.iter().any(|f| f.name == "src-tauri" && f.is_dir));
    let sub = call(git_branch_files(repo(), "main".into(), "src-tauri/src".into())).expect("subtree");
    assert!(sub.iter().any(|f| f.name == "lib.rs" && !f.is_dir));
    let blog = call(git_branch_log(repo(), "main".into(), 5)).expect("branch log");
    assert!(!blog.is_empty() && !blog[0].summary.is_empty());
}

#[test]
fn non_repo_reports_cleanly() {
    let state = call(git_repo_state("/tmp".into()));
    assert!(!state.is_repo);
}

#[test]
fn lists_projects_dir() {
    let projects = call(aiterm_lib::fsx::list_projects());
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
        .map(|s| call(aiterm_lib::sessions::session_artifacts(s.id.clone())).len())
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
            let p = call(aiterm_lib::sessions::session_preview(s.id.clone()));
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
fn deletes_session_to_trash() {
    // Synthetic session file in a scratch project dir — never touches real data.
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let dir = home.join(".claude/projects/-aiterm-delete-test");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "00000000-0000-4000-8000-aitermdelete";
    let file = dir.join(format!("{id}.jsonl"));
    std::fs::write(
        &file,
        concat!(
            r#"{"type":"user","cwd":"/aiterm-delete-test","message":{"role":"user","content":"delete this synthetic session"}}"#,
            "\n",
        ),
    )
    .unwrap();

    call(aiterm_lib::sessions::session_delete(id.into())).expect("delete should succeed");
    assert!(!file.exists(), "transcript should leave the project dir");
    let trashed = home.join(".claude/trash").join(format!("{id}.jsonl"));
    assert!(trashed.exists(), "transcript should land in ~/.claude/trash");
    // Fresh mtime = full keep window (rename alone would keep the old one).
    let age = trashed
        .metadata()
        .and_then(|m| m.modified())
        .map(|m| m.elapsed().unwrap_or_default())
        .unwrap();
    assert!(age.as_secs() < 60, "trashed file should have a fresh mtime");
    assert!(
        call(aiterm_lib::sessions::session_delete("../../etc/passwd".into())).is_err(),
        "path traversal must be rejected"
    );

    // Trash listing shows it, restore brings it back to a project dir
    // derived from the transcript's cwd.
    assert!(
        call(aiterm_lib::sessions::trash_list()).iter().any(|t| t.id == id),
        "trash_list should include the trashed session"
    );
    call(aiterm_lib::sessions::trash_restore(id.into())).expect("restore should succeed");
    assert!(!trashed.exists(), "restore should empty the trash entry");
    let restored = dir.join(format!("{id}.jsonl"));
    assert!(
        restored.exists(),
        "restore should derive the original project from cwd"
    );
    let _ = std::fs::remove_file(&restored);
    let _ = std::fs::remove_dir(&dir);
}

/// A Codex rollout must come back to `~/.codex/sessions/<y>/<m>/<d>/` under its
/// original filename. Restore used to work the destination out from the
/// transcript, which can only ever name a claude project directory — so this
/// path silently put a rollout somewhere neither agent would find it, and in
/// practice failed outright because a rollout keeps its cwd under `payload`
/// where that code was not looking.
#[test]
fn restores_a_codex_rollout_to_its_own_store() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let id = "00000000-0000-4000-8000-aitermcodex";
    let day = home.join(".codex/sessions/1999/01/01");
    std::fs::create_dir_all(&day).unwrap();
    let name = format!("rollout-1999-01-01T00-00-00-{id}.jsonl");
    let file = day.join(&name);
    std::fs::write(
        &file,
        format!("{{\"payload\":{{\"session_id\":\"{id}\",\"cwd\":\"/tmp\"}}}}\n"),
    )
    .unwrap();

    call(aiterm_lib::sessions::session_delete(id.into())).expect("delete should succeed");
    assert!(!file.exists(), "rollout should leave the codex store");
    assert!(
        home.join(".claude/trash").join(format!("{id}.origin")).exists(),
        "delete should record where it came from"
    );

    call(aiterm_lib::sessions::trash_restore(id.into())).expect("restore should succeed");
    assert!(
        file.exists(),
        "rollout should return to its own store under its original filename"
    );
    assert!(
        !home.join(".claude/trash").join(format!("{id}.origin")).exists(),
        "the origin record should not outlive the restore"
    );
    // Nothing of it should have been left in claude's tree.
    assert!(!home.join(".claude/projects/-tmp").join(format!("{id}.jsonl")).exists());

    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir_all(home.join(".codex/sessions/1999"));
}

#[test]
fn hides_fork_and_orphaned_transcripts() {
    // Orchestrator/compact fork files (no human prompt) and .orphaned-*
    // leftovers must not show up as sessions.
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let dir = home.join(".claude/projects/-aiterm-fork-test");
    std::fs::create_dir_all(&dir).unwrap();
    let fork = dir.join("00000000-0000-4000-8000-aitermfork00.jsonl");
    std::fs::write(
        &fork,
        concat!(
            "{\"type\":\"custom-title\",\"customTitle\":\"aiterm\",\"cwd\":\"/aiterm-fork-test\"}\n",
            "{\"type\":\"system\",\"subtype\":\"compact_boundary\",\"cwd\":\"/aiterm-fork-test\"}\n",
            "{\"type\":\"user\",\"cwd\":\"/aiterm-fork-test\",\"message\":{\"content\":\"This session is being continued from a previous conversation that ran out of context.\"}}\n",
        ),
    )
    .unwrap();
    let orphaned = dir.join("00000000-0000-4000-8000-aitermfork00.orphaned-1-ab.jsonl");
    std::fs::write(
        &orphaned,
        "{\"type\":\"user\",\"cwd\":\"/aiterm-fork-test\",\"message\":{\"content\":\"real prompt\"}}\n",
    )
    .unwrap();
    // A real session in the same dir keeps showing.
    let real = dir.join("00000000-0000-4000-8000-aitermfork01.jsonl");
    std::fs::write(
        &real,
        "{\"type\":\"user\",\"cwd\":\"/aiterm-fork-test\",\"message\":{\"content\":\"hello there\"}}\n",
    )
    .unwrap();

    let ids: Vec<String> = ClaudeProvider.scan().into_iter().map(|s| s.id).collect();
    assert!(
        !ids.iter().any(|i| i.contains("aitermfork00")),
        "fork/orphaned transcripts must be hidden"
    );
    assert!(
        ids.iter().any(|i| i == "00000000-0000-4000-8000-aitermfork01"),
        "real sessions must still be listed"
    );

    let _ = std::fs::remove_file(&fork);
    let _ = std::fs::remove_file(&orphaned);
    let _ = std::fs::remove_file(&real);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn parses_tasks_and_agents_from_transcript() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let dir = home.join(".claude/projects/-aiterm-agent-test");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "00000000-0000-4000-8000-aitermagents";
    let file = dir.join(format!("{id}.jsonl"));
    std::fs::write(&file, concat!(
        // Two TodoWrite calls — the later list wins.
        r#"{"type":"assistant","cwd":"/x","message":{"content":[{"type":"tool_use","name":"TodoWrite","id":"t1","input":{"todos":[{"content":"old","status":"pending","activeForm":"Olding"}]}}]}}"#, "\n",
        r#"{"type":"assistant","cwd":"/x","message":{"content":[{"type":"tool_use","name":"TodoWrite","id":"t2","input":{"todos":[{"content":"build it","status":"completed","activeForm":"Building"},{"content":"ship it","status":"in_progress","activeForm":"Shipping"}]}}]}}"#, "\n",
        // Sync agent: tool_result = final report -> done.
        r#"{"type":"assistant","timestamp":"2026-07-21T10:00:00Z","message":{"content":[{"type":"tool_use","name":"Agent","id":"a1","input":{"description":"sweep tests","subagent_type":"grunt","prompt":"run tests"}}]}}"#, "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a1","content":"All 12 tests pass."}]}}"#, "\n",
        // Background agent: launched, completion via task-notification.
        r#"{"type":"assistant","timestamp":"2026-07-21T10:01:00Z","message":{"content":[{"type":"tool_use","name":"Agent","id":"a2","input":{"description":"long refactor","subagent_type":"builder","prompt":"refactor"}}]}}"#, "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a2","content":"Async agent launched successfully."}]}}"#, "\n",
        // Still-running background agent: launched, no notification yet.
        r#"{"type":"assistant","timestamp":"2026-07-21T10:02:00Z","message":{"content":[{"type":"tool_use","name":"Agent","id":"a3","input":{"description":"watch build","subagent_type":"grunt","prompt":"watch"}}]}}"#, "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a3","content":"Async agent launched successfully."}]}}"#, "\n",
        r#"{"type":"user","message":{"content":"<task-notification><task-id>xyz</task-id><tool-use-id>a2</tool-use-id><status>completed</status><summary>Refactor finished, 8 files.</summary><result>Refactored 8 files, all tests green.</result></task-notification>"}}"#, "\n",
    )).unwrap();

    let tasks = call(aiterm_lib::sessions::session_tasks(id.into()));
    assert_eq!(tasks.len(), 2, "latest TodoWrite list should win");
    assert_eq!(tasks[0].subject, "build it");
    assert_eq!(tasks[1].status, "in_progress");

    // Newer TaskCreate/TaskUpdate system, written after the TodoWrite: wins.
    let id2 = "00000000-0000-4000-8000-aitermtasks2";
    let file2 = dir.join(format!("{id2}.jsonl"));
    std::fs::write(&file2, concat!(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TodoWrite","id":"t0","input":{"todos":[{"content":"stale list","status":"pending"}]}}]}}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TaskCreate","id":"c1","input":{"subject":"first task","activeForm":"Doing first"}}]}}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TaskCreate","id":"c2","input":{"subject":"second task","activeForm":"Doing second"}}]}}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TaskCreate","id":"c3","input":{"subject":"doomed task"}}]}}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TaskUpdate","id":"u1","input":{"taskId":"1","status":"completed"}}]}}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TaskUpdate","id":"u2","input":{"taskId":"2","status":"in_progress"}}]}}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TaskUpdate","id":"u3","input":{"taskId":"3","status":"deleted"}}]}}"#, "\n",
    )).unwrap();
    let tasks2 = call(aiterm_lib::sessions::session_tasks(id2.into()));
    assert_eq!(tasks2.len(), 2, "deleted tasks drop; TaskCreate list beats stale TodoWrite");
    assert_eq!(tasks2[0].subject, "first task");
    assert_eq!(tasks2[0].status, "completed");
    assert_eq!(tasks2[1].status, "in_progress");
    let _ = std::fs::remove_file(&file2);

    let agents = call(aiterm_lib::sessions::session_agents(id.into()));
    assert_eq!(agents.len(), 3);
    let by_id = |i: &str| agents.iter().find(|a| a.id == i).unwrap();
    assert_eq!(by_id("a1").status, "done");
    assert!(by_id("a1").result.as_deref().unwrap().contains("12 tests"));
    assert_eq!(by_id("a2").status, "done");
    assert!(
        by_id("a2").result.as_deref().unwrap().contains("all tests green"),
        "full <result> report should win over the <summary> one-liner"
    );
    assert_eq!(by_id("a3").status, "running", "no notification yet = still running");
    assert_eq!(by_id("a3").agent_type, "grunt");

    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn keeps_both_fork_siblings() {
    // An explicit `--fork-session` writes a new transcript sharing the
    // original's bridgeSessionId but leaves the original intact and
    // independently resumable. BOTH must stay listed — collapsing the family
    // to the newest row (old behavior) hid the parent's context entirely.
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let dir = home.join(".claude/projects/-aiterm-fork-dup-test");
    std::fs::create_dir_all(&dir).unwrap();
    let bridge = "cse_aitermforkduptest";
    let mk = |id: &str, prompt: &str| {
        let f = dir.join(format!("{id}.jsonl"));
        let l1 = format!(
            r#"{{"type":"user","cwd":"/aiterm-fork-dup","message":{{"content":"{prompt}"}}}}"#
        );
        let l2 = format!(
            r#"{{"type":"bridge-session","sessionId":"{id}","bridgeSessionId":"{bridge}","lastSequenceNum":1}}"#
        );
        std::fs::write(&f, format!("{l1}\n{l2}\n")).unwrap();
        f
    };
    let older = mk("00000000-0000-4000-8000-aitermforkold", "older fork");
    let newer = mk("00000000-0000-4000-8000-aitermforknew", "newer fork");
    // Make `older` genuinely older so newest-wins is deterministic.
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::open(&older).unwrap().set_modified(past).unwrap();

    let ids: Vec<String> = ClaudeProvider.scan().into_iter().map(|s| s.id).collect();
    assert!(
        ids.iter().any(|i| i == "00000000-0000-4000-8000-aitermforknew"),
        "fork should be listed"
    );
    assert!(
        ids.iter().any(|i| i == "00000000-0000-4000-8000-aitermforkold"),
        "forked parent must stay listed — its context is still resumable"
    );

    let _ = std::fs::remove_file(&older);
    let _ = std::fs::remove_file(&newer);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn full_text_search_finds_sessions() {
    let r = call(aiterm_lib::indexer::reindex_sessions());
    assert!(r.total > 0, "expected sessions to index");
    let hits = call(aiterm_lib::indexer::search_sessions("aiterm".into()));
    assert!(!hits.is_empty(), "searching 'aiterm' should hit this session");
}
