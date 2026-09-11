use super::store;
use crate::remote::auth::write_private_file;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::Mutex};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Selection {
    pub tab_id: String,
    pub session_id: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Navigation {
    pub desktop_id: Option<String>,
    pub sessions: HashMap<String, Selection>,
}
static LOCK: Mutex<()> = Mutex::new(());
fn read(path: &Path) -> Result<Navigation, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Navigation::default()),
        Err(e) => Err(e.to_string()),
    }
}
pub(super) fn load() -> Result<Navigation, String> {
    let _guard = LOCK.lock().unwrap();
    read(&store::root()?.join("navigation.json"))
}
pub(super) fn remember(desktop: &str, selection: Option<Selection>) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap();
    let path = store::root()?.join("navigation.json");
    let mut nav = read(&path)?;
    nav.desktop_id = Some(desktop.into());
    if let Some(selection) = selection {
        nav.sessions.insert(desktop.into(), selection);
    }
    save(&path, &nav)
}
fn save(path: &Path, nav: &Navigation) -> Result<(), String> {
    let temp = path.with_extension("tmp");
    write_private_file(&temp, &serde_json::to_vec(nav).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}
/// Prefer the actual tab. A host restart may give the same agent session a new
/// tab ID; never fall back to an unrelated live session.
pub(super) fn resolve(
    tabs: &[Value],
    selected: Option<&str>,
    saved: Option<&Selection>,
) -> Option<String> {
    let live: Vec<_> = tabs.iter().filter(|t| t["state"] == "running").collect();
    let id = selected.or_else(|| saved.map(|s| s.tab_id.as_str()));
    live.iter()
        .find(|t| t["id"].as_str() == id)
        .or_else(|| {
            saved
                .and_then(|s| s.session_id.as_deref())
                .and_then(|session| {
                    live.iter()
                        .find(|t| t["sessionId"].as_str() == Some(session))
                })
        })
        .and_then(|t| t["id"].as_str().map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn navigation_survives_reload_and_resolves_only_the_previous_live_session() {
        let path =
            std::env::temp_dir().join(format!("aiterm-navigation-{}.json", uuid::Uuid::new_v4()));
        let mut nav = Navigation::default();
        nav.desktop_id = Some("work".into());
        nav.sessions.insert(
            "work".into(),
            Selection {
                tab_id: "old".into(),
                session_id: Some("agent-session".into()),
            },
        );
        save(&path, &nav).unwrap();
        let loaded = read(&path).unwrap();
        assert_eq!(loaded.desktop_id.as_deref(), Some("work"));
        let saved = loaded.sessions.get("work");
        assert_eq!(
            resolve(&[json!({"id":"old","state":"running"})], None, saved).as_deref(),
            Some("old")
        );
        assert_eq!(
            resolve(
                &[json!({"id":"new","sessionId":"agent-session","state":"running"})],
                Some("old"),
                saved
            )
            .as_deref(),
            Some("new")
        );
        assert!(resolve(
            &[
                json!({"id":"old","state":"exited"}),
                json!({"id":"unrelated","state":"running"})
            ],
            None,
            saved
        )
        .is_none());
        std::fs::remove_file(path).unwrap();
    }
}
