use serde::Serialize;

#[derive(Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
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
