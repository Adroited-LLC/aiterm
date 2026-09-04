use git2::{BranchType, DiffOptions, Repository, StatusOptions};
use serde::Serialize;

#[derive(Serialize)]
pub struct FileStatus {
    pub path: String,
    /// Two-char porcelain-style code, e.g. "M ", " M", "??", "A ".
    pub status: String,
    pub staged: bool,
}

#[derive(Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub is_head: bool,
    pub upstream: Option<String>,
}

#[derive(Serialize)]
pub struct CommitInfo {
    pub id: String,
    pub short_id: String,
    pub summary: String,
    pub author: String,
    pub time: i64,
    pub refs: Vec<String>,
    pub parents: Vec<String>,
}

#[derive(Serialize)]
pub struct RepoState {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

fn open(path: &str) -> Result<Repository, String> {
    Repository::discover(path).map_err(|e| e.message().to_string())
}

#[cfg_attr(not(feature = "wsl-headless"), tauri::command)]
pub async fn git_repo_state(path: String) -> RepoState {
    crate::run_blocking(move || git_repo_state_sync(path)).await
}

fn git_repo_state_sync(path: String) -> RepoState {
    let Ok(repo) = Repository::discover(&path) else {
        return RepoState {
            is_repo: false,
            branch: None,
            ahead: 0,
            behind: 0,
        };
    };
    let head = repo.head().ok();
    let branch = head
        .as_ref()
        .and_then(|h| h.shorthand().ok())
        .map(String::from);
    let (ahead, behind) = head
        .as_ref()
        .and_then(|h| {
            let local = h.target()?;
            let branch_name = h.shorthand().ok()?;
            let b = repo.find_branch(branch_name, BranchType::Local).ok()?;
            let upstream = b.upstream().ok()?.get().target()?;
            repo.graph_ahead_behind(local, upstream).ok()
        })
        .unwrap_or((0, 0));
    RepoState {
        is_repo: true,
        branch,
        ahead,
        behind,
    }
}

#[cfg_attr(not(feature = "wsl-headless"), tauri::command)]
pub async fn git_status(path: String) -> Result<Vec<FileStatus>, String> {
    crate::run_blocking(move || git_status_sync(path)).await
}

fn git_status_sync(path: String) -> Result<Vec<FileStatus>, String> {
    let repo = open(&path)?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for entry in statuses.iter() {
        let s = entry.status();
        let index_char = if s.is_index_new() {
            'A'
        } else if s.is_index_modified() {
            'M'
        } else if s.is_index_deleted() {
            'D'
        } else if s.is_index_renamed() {
            'R'
        } else {
            ' '
        };
        let wt_char = if s.is_wt_new() {
            '?'
        } else if s.is_wt_modified() {
            'M'
        } else if s.is_wt_deleted() {
            'D'
        } else if s.is_wt_renamed() {
            'R'
        } else {
            ' '
        };
        let code = if s.is_wt_new() {
            "??".to_string()
        } else {
            format!("{index_char}{wt_char}")
        };
        out.push(FileStatus {
            path: entry.path().unwrap_or("").to_string(),
            staged: index_char != ' ',
            status: code,
        });
    }
    Ok(out)
}

#[cfg_attr(not(feature = "wsl-headless"), tauri::command)]
pub async fn git_branches(path: String) -> Result<Vec<BranchInfo>, String> {
    crate::run_blocking(move || git_branches_sync(path)).await
}

fn git_branches_sync(path: String) -> Result<Vec<BranchInfo>, String> {
    let repo = open(&path)?;
    let head_name = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().ok().map(String::from));
    let mut out = Vec::new();
    let branches = repo
        .branches(Some(BranchType::Local))
        .map_err(|e| e.to_string())?;
    for branch in branches.flatten() {
        let (b, _) = branch;
        let name = b.name().ok().flatten().unwrap_or("").to_string();
        let upstream = b
            .upstream()
            .ok()
            .and_then(|u| u.name().ok().flatten().map(String::from));
        out.push(BranchInfo {
            is_head: Some(&name) == head_name.as_ref(),
            name,
            upstream,
        });
    }
    Ok(out)
}

#[cfg_attr(not(feature = "wsl-headless"), tauri::command)]
pub async fn git_log(path: String, limit: usize) -> Result<Vec<CommitInfo>, String> {
    crate::run_blocking(move || git_log_sync(path, limit)).await
}

fn git_log_sync(path: String, limit: usize) -> Result<Vec<CommitInfo>, String> {
    let repo = open(&path)?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    // Topological order across all local branches so the graph has proper lanes.
    let _ = walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME);
    walk.push_head().map_err(|e| e.to_string())?;
    if let Ok(branches) = repo.branches(Some(BranchType::Local)) {
        for (b, _) in branches.flatten() {
            if let Some(oid) = b.get().target() {
                let _ = walk.push(oid);
            }
        }
    }

    // Collect ref decorations (branch/tag tips) for GitLens-style labels.
    let mut ref_map: std::collections::HashMap<git2::Oid, Vec<String>> =
        std::collections::HashMap::new();
    if let Ok(refs) = repo.references() {
        for r in refs.flatten() {
            if let (Some(oid), Some(name)) = (r.target(), r.shorthand().ok()) {
                if name != "HEAD" {
                    ref_map.entry(oid).or_default().push(name.to_string());
                }
            }
        }
    }

    let mut out = Vec::new();
    for oid in walk.flatten().take(limit) {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        out.push(CommitInfo {
            id: oid.to_string(),
            short_id: oid.to_string()[..7].to_string(),
            summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
            author: commit.author().name().ok().unwrap_or("").to_string(),
            time: commit.time().seconds(),
            refs: ref_map.get(&oid).cloned().unwrap_or_default(),
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        });
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct TreeEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Structure (not content) of a branch tip's tree at `subpath` ("" = root).
#[cfg_attr(not(feature = "wsl-headless"), tauri::command)]
pub async fn git_branch_files(
    path: String,
    branch: String,
    subpath: String,
) -> Result<Vec<TreeEntry>, String> {
    crate::run_blocking(move || git_branch_files_sync(path, branch, subpath)).await
}

fn git_branch_files_sync(
    path: String,
    branch: String,
    subpath: String,
) -> Result<Vec<TreeEntry>, String> {
    let repo = open(&path)?;
    let b = repo
        .find_branch(&branch, BranchType::Local)
        .map_err(|e| e.message().to_string())?;
    let commit = b.get().peel_to_commit().map_err(|e| e.to_string())?;
    let root = commit.tree().map_err(|e| e.to_string())?;
    let tree = if subpath.is_empty() {
        root
    } else {
        root.get_path(std::path::Path::new(&subpath))
            .map_err(|e| e.to_string())?
            .to_object(&repo)
            .map_err(|e| e.to_string())?
            .peel_to_tree()
            .map_err(|e| e.to_string())?
    };
    let mut entries: Vec<TreeEntry> = tree
        .iter()
        .map(|e| TreeEntry {
            name: e.name().unwrap_or("").to_string(),
            is_dir: e.kind() == Some(git2::ObjectType::Tree),
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Recent commits reachable from one branch tip.
#[cfg_attr(not(feature = "wsl-headless"), tauri::command)]
pub async fn git_branch_log(
    path: String,
    branch: String,
    limit: usize,
) -> Result<Vec<CommitInfo>, String> {
    crate::run_blocking(move || git_branch_log_sync(path, branch, limit)).await
}

fn git_branch_log_sync(
    path: String,
    branch: String,
    limit: usize,
) -> Result<Vec<CommitInfo>, String> {
    let repo = open(&path)?;
    let b = repo
        .find_branch(&branch, BranchType::Local)
        .map_err(|e| e.message().to_string())?;
    let tip = b.get().target().ok_or("branch has no target")?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.push(tip).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for oid in walk.flatten().take(limit) {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        out.push(CommitInfo {
            id: oid.to_string(),
            short_id: oid.to_string()[..7].to_string(),
            summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
            author: commit.author().name().ok().unwrap_or("").to_string(),
            time: commit.time().seconds(),
            refs: vec![],
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        });
    }
    Ok(out)
}

#[cfg_attr(not(feature = "wsl-headless"), tauri::command)]
pub async fn git_diff_file(path: String, file: String) -> Result<String, String> {
    crate::run_blocking(move || git_diff_file_sync(path, file)).await
}

fn git_diff_file_sync(path: String, file: String) -> Result<String, String> {
    let repo = open(&path)?;
    let mut opts = DiffOptions::new();
    opts.pathspec(&file)
        .include_untracked(true)
        .show_untracked_content(true);
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| e.to_string())?;
    let mut text = String::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        let origin = line.origin();
        if matches!(origin, '+' | '-' | ' ') {
            text.push(origin);
        }
        text.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .map_err(|e| e.to_string())?;
    Ok(text)
}

#[cfg_attr(not(feature = "wsl-headless"), tauri::command)]
pub async fn git_commit_diff(path: String, commit_id: String) -> Result<String, String> {
    crate::run_blocking(move || git_commit_diff_sync(path, commit_id)).await
}

fn git_commit_diff_sync(path: String, commit_id: String) -> Result<String, String> {
    let repo = open(&path)?;
    let oid = git2::Oid::from_str(&commit_id).map_err(|e| e.to_string())?;
    let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
    let tree = commit.tree().map_err(|e| e.to_string())?;
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| e.to_string())?;
    let mut text = String::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        let origin = line.origin();
        if matches!(origin, '+' | '-' | ' ') {
            text.push(origin);
        }
        text.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .map_err(|e| e.to_string())?;
    Ok(text)
}
