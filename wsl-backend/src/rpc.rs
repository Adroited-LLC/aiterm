//! Short-lived, local requests from the Windows workbench. No TCP service.
use crate::{fsx, git};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

fn arg(args: &Value, name: &str) -> Result<String, String> {
    args[name]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("Missing {name}"))
}
fn value(value: impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

async fn dispatch(command: &str, a: &Value) -> Result<Value, String> {
    let path = || arg(a, "path");
    let limit = || a["limit"].as_u64().unwrap_or(100).min(500) as usize;
    match command {
        "workspace" => Ok(
            json!({"home": dirs::home_dir(), "distribution": std::env::var("WSL_DISTRO_NAME").unwrap_or_default(), "shell": std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())}),
        ),
        "list_projects" => {
            let mut projects = fsx::list_projects().await;
            if let Some(home) = dirs::home_dir() {
                if let Ok(entries) = fs::read_dir(home.join("projects")) {
                    for entry in entries.flatten().filter(|e| e.path().is_dir()) {
                        let p = entry.path();
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if !name.starts_with('.')
                            && !projects.iter().any(|v| v.path == p.to_string_lossy())
                        {
                            projects.push(fsx::ProjectInfo {
                                name,
                                path: p.to_string_lossy().into_owned(),
                                is_git: p.join(".git").exists(),
                                last_modified: 0,
                            });
                        }
                    }
                }
            }
            value(projects)
        }
        "list_dir" => value(fsx::list_dir(path()?).await?),
        "read_text_file" => value(fsx::read_text_file(path()?).await?),
        "write_text_file" => value(
            fsx::write_text_file(path()?, arg(a, "content")?, a["expectedMtimeMs"].as_u64())
                .await?,
        ),
        "render_markdown" => {
            let mut options = comrak::Options::default();
            options.extension.table = true;
            options.extension.strikethrough = true;
            options.extension.tasklist = true;
            value(comrak::markdown_to_html(&arg(a, "source")?, &options))
        }
        "git_repo_state" => value(git::git_repo_state(path()?).await),
        "git_status" => value(git::git_status(path()?).await?),
        "git_branches" => value(git::git_branches(path()?).await?),
        "git_log" => value(git::git_log(path()?, limit()).await?),
        "git_branch_files" => {
            value(git::git_branch_files(path()?, arg(a, "branch")?, arg(a, "subpath")?).await?)
        }
        "git_branch_log" => value(git::git_branch_log(path()?, arg(a, "branch")?, limit()).await?),
        "git_diff_file" => value(git::git_diff_file(path()?, arg(a, "file")?).await?),
        "git_commit_diff" => value(git::git_commit_diff(path()?, arg(a, "commitId")?).await?),
        "agent_choices" => agents(),
        "list_sessions" => value(sessions()),
        _ => Err(format!("Unsupported WSL operation: {command}")),
    }
}

fn agents() -> Result<Value, String> {
    // The same login environment used to launch terminals, including npm/nvm.
    let output = std::process::Command::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()))
        .args(["-lc", "for a in claude codex opencode gemini; do if command -v \"$a\" >/dev/null 2>&1; then printf 'AITERM_AGENT:%s\\n' \"$a\"; fi; done"])
        .output().map_err(|e| e.to_string())?;
    let agents: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let id = line.strip_prefix("AITERM_AGENT:")?;
            let name = match id {
                "claude" => "Claude Code",
                "codex" => "Codex",
                "opencode" => "OpenCode",
                "gemini" => "Gemini CLI",
                _ => return None,
            };
            Some(json!({"id": id, "display_name": name}))
        })
        .collect();
    value(agents)
}

fn transcripts(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 || out.len() >= 2000 {
        return;
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if out.len() >= 2000 {
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let p = entry.path();
            if kind.is_dir() {
                transcripts(&p, depth - 1, out);
            } else if kind.is_file() && p.extension().is_some_and(|e| e == "jsonl") {
                out.push(p);
            }
        }
    }
}

fn sessions() -> Vec<Value> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    sessions_in(&home)
}

fn sessions_in(home: &Path) -> Vec<Value> {
    let mut result = Vec::new();
    for (agent, root, depth) in [
        ("claude", home.join(".claude/projects"), 2),
        ("codex", home.join(".codex/sessions"), 4),
    ] {
        let mut files = Vec::new();
        transcripts(&root, depth, &mut files);
        files.sort_by_key(|p| std::cmp::Reverse(fs::metadata(p).and_then(|m| m.modified()).ok()));
        for file in files.into_iter().take(300) {
            let Ok(f) = fs::File::open(&file) else {
                continue;
            };
            let at = f
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|t| t.as_millis() as u64)
                .unwrap_or(0);
            let mut id = String::new();
            let mut cwd = String::new();
            let mut title = String::new();
            for line in BufReader::new(f.take(512 * 1024))
                .lines()
                .take(100)
                .map_while(Result::ok)
            {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if agent == "claude" {
                    if id.is_empty() {
                        id = v["sessionId"].as_str().unwrap_or_default().into();
                    }
                    if cwd.is_empty() {
                        cwd = v["cwd"].as_str().unwrap_or_default().into();
                    }
                    if v["type"] == "user" && title.is_empty() {
                        if let Some(s) = v["message"]["content"].as_str() {
                            title = s.into();
                        } else if let Some(parts) = v["message"]["content"].as_array() {
                            title = parts
                                .iter()
                                .filter_map(|p| p["text"].as_str())
                                .collect::<Vec<_>>()
                                .join(" ");
                        }
                    }
                } else {
                    if v["type"] == "session_meta" {
                        id = v["payload"]["id"].as_str().unwrap_or_default().into();
                        cwd = v["payload"]["cwd"].as_str().unwrap_or_default().into();
                    }
                    if title.is_empty()
                        && v["type"] == "event_msg"
                        && v["payload"]["type"] == "user_message"
                    {
                        title = v["payload"]["message"].as_str().unwrap_or_default().into();
                    }
                }
                if !id.is_empty() && !cwd.is_empty() && !title.is_empty() {
                    break;
                }
            }
            if id.is_empty() || cwd.is_empty() {
                continue;
            }
            if title.is_empty() {
                title = format!("{} session", agent);
            }
            let title: String = title
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(120)
                .collect();
            result.push(
                json!({"id":id,"agent":agent,"title":title,"project_path":cwd,"last_active":at}),
            );
        }
    }
    result.sort_by_key(|v| std::cmp::Reverse(v["last_active"].as_u64().unwrap_or(0)));
    result
}

pub fn serve() {
    let result = (|| {
        let request: Value = aiterm_wsl_protocol::read_frame(&mut std::io::stdin().lock())
            .map_err(|e| e.to_string())?
            .ok_or("Missing RPC request")?;
        pollster::block_on(dispatch(&arg(&request, "command")?, &request["args"]))
    })();
    let response = match result {
        Ok(value) => json!({"value":value}),
        Err(error) => json!({"error":error}),
    };
    if let Err(error) = aiterm_wsl_protocol::write_frame(&mut std::io::stdout().lock(), &response) {
        let _ = aiterm_wsl_protocol::write_frame(
            &mut std::io::stdout().lock(),
            &json!({"error":error.to_string()}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_claude_and_codex_transcripts_without_changing_them() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home =
            std::env::temp_dir().join(format!("aiterm-history-{}-{unique}", std::process::id()));
        let claude = home.join(".claude/projects/project");
        let codex = home.join(".codex/sessions/2026/09/04");
        fs::create_dir_all(&claude).unwrap();
        fs::create_dir_all(&codex).unwrap();
        let first = json!({"type":"user","sessionId":"claude-id","cwd":"/tmp/my project","message":{"content":[{"type":"text","text":"Fix the Unicode 世界 test"}]}}).to_string();
        fs::write(claude.join("claude-id.jsonl"), &first).unwrap();
        let second = format!(
            "{}\n{}\ninvalid partial line",
            json!({"type":"session_meta","payload":{"id":"codex-id","cwd":"/tmp/another project"}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Build the Windows UI"}})
        );
        fs::write(codex.join("rollout.jsonl"), &second).unwrap();
        let history = sessions_in(&home);
        assert_eq!(history.len(), 2);
        assert!(history
            .iter()
            .any(|v| v["id"] == "claude-id" && v["title"] == "Fix the Unicode 世界 test"));
        assert!(history
            .iter()
            .any(|v| v["id"] == "codex-id" && v["project_path"] == "/tmp/another project"));
        assert_eq!(
            fs::read_to_string(claude.join("claude-id.jsonl")).unwrap(),
            first
        );
        assert_eq!(
            fs::read_to_string(codex.join("rollout.jsonl")).unwrap(),
            second
        );
        fs::remove_dir_all(home).unwrap();
    }
}
