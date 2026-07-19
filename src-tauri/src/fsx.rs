use serde::Serialize;

#[derive(Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Serialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: String,
    pub is_git: bool,
    /// Unix millis of last directory modification.
    pub last_modified: u64,
}

/// All project directories under ~/Projects, whether or not they still have
/// Claude sessions (transcripts get purged after cleanupPeriodDays).
#[tauri::command]
pub fn list_projects() -> Vec<ProjectInfo> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(home.join("Projects")) else {
        return vec![];
    };
    let mut projects: Vec<ProjectInfo> = entries
        .flatten()
        .filter_map(|e| {
            if !e.file_type().ok()?.is_dir() {
                return None;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let path = e.path();
            let last_modified = e
                .metadata()
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as u64;
            Some(ProjectInfo {
                is_git: path.join(".git").exists(),
                path: path.to_string_lossy().to_string(),
                name,
                last_modified,
            })
        })
        .collect();
    projects.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    projects
}

/// Open a file with the desktop's default application.
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("xdg-open {path}: {e}"))
}

#[tauri::command]
pub fn list_dir(path: String) -> Result<Vec<DirEntry>, String> {
    let mut entries: Vec<DirEntry> = std::fs::read_dir(&path)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().ok()?.is_dir();
            Some(DirEntry {
                path: e.path().to_string_lossy().to_string(),
                name,
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}
