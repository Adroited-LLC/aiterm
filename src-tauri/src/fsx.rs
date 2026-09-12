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

/// All project directories under ~/Projects or ~/projects, whether or not they still have
/// Claude sessions (transcripts get purged after cleanupPeriodDays).
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn list_projects() -> Vec<ProjectInfo> {
    crate::run_blocking(list_projects_sync).await
}

fn list_projects_sync() -> Vec<ProjectInfo> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let mut roots = std::collections::HashSet::new();
    let entries = [home.join("Projects"), home.join("projects")].into_iter()
        .filter(|path| roots.insert(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())))
        .filter_map(|path| std::fs::read_dir(path).ok())
        .flatten();
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
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn open_path(path: String) -> Result<(), String> {
    crate::run_blocking(move || open_path_sync(path)).await
}

fn open_path_sync(path: String) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("xdg-open {path}: {e}"))
}

#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn list_dir(path: String) -> Result<Vec<DirEntry>, String> {
    crate::run_blocking(move || list_dir_sync(path)).await
}

fn list_dir_sync(path: String) -> Result<Vec<DirEntry>, String> {
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

/// A cheap revision token for previews. Nanoseconds remain exact as text.
/// Checking this never reads or decodes the file's contents.
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn file_revision(path: String) -> Result<String, String> {
    crate::run_blocking(move || file_revision_sync(&path)).await
}

fn file_revision_sync(path: &str) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() { return Err("not a regular file".into()); }
    let modified = metadata.modified().map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?;
    Ok(format!("{}:{}", metadata.len(), modified.as_nanos()))
}

/// A text file's content for the in-app viewer, with what a save needs to
/// detect a concurrent writer: the mtime the content was read at.
#[derive(Serialize)]
pub struct TextFile {
    pub content: String,
    /// Unix millis of the file's mtime when this content was read.
    pub mtime_ms: u64,
    /// The file was larger than the cap and `content` is only its head.
    /// The viewer shows it read-only: saving a truncated read back would
    /// destroy the tail.
    pub truncated: bool,
}

/// 2 MB. Past this a file is not something to hand a text editor whole —
/// the head is shown read-only instead.
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

fn file_mtime_ms(path: &str) -> Result<u64, String> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|e| e.to_string())
}

#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn read_text_file(path: String) -> Result<TextFile, String> {
    crate::run_blocking(move || read_text_file_sync(&path)).await
}

fn read_text_file_sync(path: &str) -> Result<TextFile, String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err("that is a directory".into());
    }
    let mtime_ms = file_mtime_ms(path)?;
    let truncated = meta.len() > MAX_TEXT_BYTES;
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    file.take(MAX_TEXT_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    // A NUL in the head is the classic binary tell (it is what grep uses).
    // Lossy-decoding an executable into the editor helps nobody.
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err("binary file — use the system app".into());
    }
    Ok(TextFile {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        mtime_ms,
        truncated,
    })
}

/// The marker `write_text_file` rejects a stale save with. The frontend
/// matches on it to offer Reload / Overwrite instead of a plain error.
pub const CHANGED_ON_DISK: &str = "changed-on-disk";

/// Write the viewer's buffer back. `expected_mtime_ms` is the mtime the
/// buffer was loaded (or last saved) at: if the file has moved past it,
/// someone else — an agent, most likely — wrote in between, and silently
/// clobbering their edit is the one wrong answer. Pass `None` to overwrite
/// deliberately. Returns the new mtime so the next save has its baseline.
#[cfg_attr(not(aiterm_headless), tauri::command)]
pub async fn write_text_file(
    path: String,
    content: String,
    expected_mtime_ms: Option<u64>,
) -> Result<u64, String> {
    crate::run_blocking(move || write_text_file_sync(&path, &content, expected_mtime_ms)).await
}

fn write_text_file_sync(
    path: &str,
    content: &str,
    expected_mtime_ms: Option<u64>,
) -> Result<u64, String> {
    if let Some(expected) = expected_mtime_ms {
        if file_mtime_ms(path)? != expected {
            return Err(CHANGED_ON_DISK.into());
        }
    }
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    file_mtime_ms(path)
}

#[cfg(test)]
mod revision_tests {
    use super::*;

    #[test]
    fn preview_revision_ignores_neighbors_and_detects_replacement() {
        let root = std::env::temp_dir().join(format!("aiterm-image-revision-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let image = root.join("preview.png");
        std::fs::write(&image, b"first image").unwrap();
        let path = image.to_str().unwrap();
        let first = file_revision_sync(path).unwrap();
        std::fs::write(root.join("unrelated.txt"), b"agent working").unwrap();
        assert_eq!(file_revision_sync(path).unwrap(), first);
        // Same length, different timestamp: size alone must not decide freshness.
        let file = std::fs::OpenOptions::new().write(true).open(&image).unwrap();
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        file.set_times(std::fs::FileTimes::new().set_modified(future)).unwrap();
        assert_ne!(file_revision_sync(path).unwrap(), first);
        assert!(file_revision_sync(root.to_str().unwrap()).is_err());
        std::fs::remove_file(&image).unwrap();
        assert!(file_revision_sync(path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
