use serde::Serialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub agent: String,
    pub title: String,
    pub project_path: String,
    pub branch: Option<String>,
    /// Unix millis of last activity (file mtime).
    pub last_active: u64,
}

pub trait SessionProvider {
    fn scan(&self) -> Vec<Session>;
}

pub struct ClaudeProvider;

impl SessionProvider for ClaudeProvider {
    fn scan(&self) -> Vec<Session> {
        let Some(home) = dirs::home_dir() else {
            return vec![];
        };
        let root = home.join(".claude/projects");
        let Ok(projects) = std::fs::read_dir(&root) else {
            return vec![];
        };

        let mut sessions = Vec::new();
        for project in projects.flatten() {
            let Ok(files) = std::fs::read_dir(project.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                if let Some(s) = parse_session(&path) {
                    sessions.push(s);
                }
            }
        }
        sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        sessions
    }
}

/// Pull title/cwd/branch out of the first lines of a session jsonl without
/// parsing the whole transcript.
fn parse_session(path: &Path) -> Option<Session> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() == 0 {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;

    let id = path.file_stem()?.to_string_lossy().to_string();
    let reader = BufReader::new(File::open(path).ok()?);

    let mut title: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut first_prompt: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut branch: Option<String> = None;

    for line in reader.lines().take(120).flatten() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("custom-title") => {
                title = v.get("customTitle").and_then(|t| t.as_str()).map(String::from)
            }
            Some("summary") => {
                summary = v.get("summary").and_then(|t| t.as_str()).map(String::from)
            }
            Some("user") if first_prompt.is_none() => {
                first_prompt = v
                    .pointer("/message/content")
                    .and_then(|c| c.as_str())
                    .map(|s| s.chars().take(80).collect());
            }
            _ => {}
        }
        if cwd.is_none() {
            cwd = v.get("cwd").and_then(|c| c.as_str()).map(String::from);
        }
        if branch.is_none() {
            branch = v
                .get("gitBranch")
                .and_then(|b| b.as_str())
                .filter(|b| !b.is_empty() && *b != "HEAD")
                .map(String::from);
        }
        if title.is_some() && cwd.is_some() && branch.is_some() {
            break;
        }
    }

    let project_path = cwd?;
    let title = title
        .or(summary)
        .or(first_prompt)
        .unwrap_or_else(|| basename(&project_path));

    Some(Session {
        id,
        agent: "claude".into(),
        title,
        project_path,
        branch,
        last_active: mtime,
    })
}

fn basename(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string())
}

#[tauri::command]
pub fn list_sessions() -> Vec<Session> {
    // Future agents (codex, gemini, ...) get added to this list.
    let providers: Vec<Box<dyn SessionProvider>> = vec![Box::new(ClaudeProvider)];
    let mut all: Vec<Session> = providers.iter().flat_map(|p| p.scan()).collect();
    all.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    all
}
