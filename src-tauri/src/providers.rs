//! API model access — OpenRouter, OpenAI, or anything else speaking the same
//! `/models` + `/chat/completions` shape.
//!
//! This is configuration, not an engine. Nothing in aiterm runs a session
//! against these yet; what they give you today is somewhere to put a key, and a
//! **Test** that proves the key works by asking the provider for its model list.
//! That distinction is kept honest in the UI, because a settings form that
//! silently does nothing is worse than no form.
//!
//! Everything here is OpenAI-compatible on purpose. OpenRouter, Together,
//! Groq, vLLM, llama.cpp and OpenAI itself all serve `GET {base}/models` with a
//! bearer token, so one shape covers them and a provider is just a base URL.
//!
//! ## Where the key lives
//!
//! `~/.config/aiterm/providers.json`, `0600`, directory `0700`. Written by hand
//! rather than through the settings store because that one is a UI-preferences
//! file the app rewrites freely, and a credential does not belong in something
//! with those habits.
//!
//! It is plaintext on disk. That matches how `claude` and most CLI tools keep
//! their own credentials (`~/.claude/.credentials.json` is the same), so it is
//! not a new exposure on this machine — but it is not a secret store either,
//! and `0600` is the whole of the protection. A keyring backend would be a
//! genuine improvement and is deliberately not faked here.
//!
//! ## What never crosses to the frontend
//!
//! The key. [`ProviderView`] carries a `has_key` flag and the last four
//! characters, which is enough to tell two keys apart in a list and useless to
//! anyone reading a screenshot. The renderer cannot ask for the real value —
//! there is no command that returns it.

use serde::{Deserialize, Serialize};

/// A configured provider, as stored.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Provider {
    /// Stable slug, generated from the name on first save. Used as the key for
    /// updates and deletes so renaming a provider does not orphan it.
    pub id: String,
    pub name: String,
    /// Base URL *without* a trailing slash or `/models` — e.g.
    /// `https://openrouter.ai/api/v1`.
    pub base_url: String,
    pub api_key: String,
}

/// A provider as the UI sees it: everything except the key.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub has_key: bool,
    /// Last four characters, for telling two keys apart. Empty when there is
    /// no key, or when the key is too short to redact meaningfully — showing
    /// most of a six-character secret would defeat the point.
    pub key_hint: String,
}

impl Provider {
    fn view(&self) -> ProviderView {
        ProviderView {
            id: self.id.clone(),
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            has_key: !self.api_key.is_empty(),
            key_hint: key_hint(&self.api_key),
        }
    }
}

fn key_hint(key: &str) -> String {
    let n = key.chars().count();
    if n < 12 {
        return String::new();
    }
    key.chars().skip(n - 4).collect()
}

/// Slug for `name`, lowercased, non-alphanumerics collapsed to `-`.
///
/// Falls back to `provider` for a name with nothing usable in it (say, one
/// written entirely in emoji), because an empty id would collide with the next
/// such name and silently overwrite it.
pub fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    let mut out = String::with_capacity(s.len());
    let mut last_dash = false;
    for c in s.chars() {
        if c == '-' && last_dash {
            continue;
        }
        last_dash = c == '-';
        out.push(c);
    }
    if out.is_empty() {
        "provider".to_string()
    } else {
        out
    }
}

/// Trim a base URL into the form the request builder expects: no trailing
/// slash, and no `/models` if the user pasted the endpoint they were reading
/// about rather than the base.
pub fn normalise_base(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    u.strip_suffix("/models").unwrap_or(u).trim_end_matches('/').to_string()
}

fn config_path() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".config/aiterm/providers.json"))
}

fn load() -> Vec<Provider> {
    let Some(path) = config_path() else {
        return vec![];
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    // A corrupt or hand-edited file reads as "no providers" rather than
    // throwing: the settings panel still opens, and re-adding one rewrites it.
    serde_json::from_str(&text).unwrap_or_default()
}

fn save(list: &[Provider]) -> Result<(), String> {
    let path = config_path().ok_or("no home directory")?;
    let dir = path.parent().ok_or("bad config path")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    restrict(dir, 0o700);
    let text = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    // Written then locked down. There is a window between create and chmod
    // where the file exists at the umask default; it is closed below rather
    // than left, because the content is a credential.
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    restrict(&path, 0o600);
    Ok(())
}

#[cfg(unix)]
fn restrict(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path, _mode: u32) {}

#[tauri::command]
pub fn providers_list() -> Vec<ProviderView> {
    load().iter().map(Provider::view).collect()
}

/// Add or update a provider.
///
/// An empty `api_key` on an existing provider keeps the stored one, so editing
/// a base URL does not require re-entering the secret — the UI cannot show it
/// back, so asking for it again to change something unrelated would mean
/// finding it again every time.
#[tauri::command]
pub fn provider_save(
    id: Option<String>,
    name: String,
    base_url: String,
    api_key: String,
) -> Result<Vec<ProviderView>, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the provider a name.".into());
    }
    let base_url = normalise_base(&base_url);
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err("The base URL should start with http:// or https://".into());
    }

    let mut list = load();
    let id = id.unwrap_or_else(|| slug(&name));
    match list.iter_mut().find(|p| p.id == id) {
        Some(existing) => {
            existing.name = name;
            existing.base_url = base_url;
            if !api_key.trim().is_empty() {
                existing.api_key = api_key.trim().to_string();
            }
        }
        None => list.push(Provider {
            id,
            name,
            base_url,
            api_key: api_key.trim().to_string(),
        }),
    }
    save(&list)?;
    Ok(list.iter().map(Provider::view).collect())
}

#[tauri::command]
pub fn provider_delete(id: String) -> Result<Vec<ProviderView>, String> {
    let mut list = load();
    list.retain(|p| p.id != id);
    save(&list)?;
    Ok(list.iter().map(Provider::view).collect())
}

/// Ask a provider for its models — the Test button.
///
/// This is what makes the form worth having before there is an engine behind
/// it: a wrong key, a typo'd base URL or a provider that is down all look
/// identical until something actually calls it.
///
/// `curl` rather than an HTTP crate, matching `usage.rs` — the project
/// deliberately pulls in no TLS stack. `async` so a slow or unreachable
/// provider does not freeze the window, which is the mistake `usage.rs`
/// documents having made.
#[tauri::command(async)]
pub fn provider_models(id: String) -> Result<Vec<String>, String> {
    let list = load();
    let p = list.iter().find(|p| p.id == id).ok_or("No such provider.")?;
    if p.api_key.is_empty() {
        return Err("No API key saved for this provider.".into());
    }
    let url = format!("{}/models", p.base_url);
    let out = std::process::Command::new("curl")
        .args([
            "-sS",
            "--connect-timeout",
            "5",
            "--max-time",
            "20",
            // Status on its own line after the body, so an HTTP error can be
            // reported as one instead of as "could not parse the reply".
            "-w",
            "\n%{http_code}",
            "-H",
            &format!("Authorization: Bearer {}", p.api_key),
            "-H",
            "Content-Type: application/json",
            &url,
        ])
        .output()
        .map_err(|e| format!("Could not run curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Could not reach {url} — {}", err.trim()));
    }
    parse_models(&String::from_utf8_lossy(&out.stdout))
}

/// Split the `-w` status off the body and pull out model ids.
///
/// Kept separate from the request so the parsing — which is where the shapes
/// actually differ between providers — can be tested without a network.
pub fn parse_models(response: &str) -> Result<Vec<String>, String> {
    let (body, status) = response.rsplit_once('\n').unwrap_or(("", response));
    let code: u16 = status.trim().parse().unwrap_or(0);
    if code == 401 || code == 403 {
        return Err("The provider rejected that API key.".into());
    }
    if !(200..300).contains(&code) {
        // Providers put a useful sentence in `error.message`; prefer it over a
        // bare status, and fall back to the status when there is nothing.
        let detail = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(String::from)
            });
        return Err(match detail {
            Some(d) => format!("HTTP {code} — {d}"),
            None => format!("HTTP {code} from the provider."),
        });
    }
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "The provider did not return JSON.".to_string())?;
    // OpenAI-compatible: `{"data":[{"id":…}]}`. A few servers return a bare
    // array, so accept that too rather than calling a working provider broken.
    let items = v
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| v.as_array())
        .ok_or("No model list in the reply.")?;
    let mut models: Vec<String> = items
        .iter()
        .filter_map(|m| {
            m.get("id")
                .and_then(|i| i.as_str())
                .or_else(|| m.as_str())
                .map(String::from)
        })
        .collect();
    models.sort();
    if models.is_empty() {
        return Err("The provider returned an empty model list.".into());
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_stable_and_never_empty() {
        assert_eq!(slug("OpenRouter"), "openrouter");
        assert_eq!(slug("My  Local  Server"), "my-local-server");
        assert_eq!(slug("  Together.ai  "), "together-ai");
        // Nothing usable in it: must not collapse to "" and collide with the
        // next such name.
        assert_eq!(slug("🙂"), "provider");
    }

    #[test]
    fn base_urls_are_normalised() {
        assert_eq!(normalise_base("https://openrouter.ai/api/v1/"), "https://openrouter.ai/api/v1");
        // The endpoint people actually have in front of them when copying.
        assert_eq!(normalise_base("https://openrouter.ai/api/v1/models"), "https://openrouter.ai/api/v1");
        assert_eq!(normalise_base("  https://x.dev/v1  "), "https://x.dev/v1");
    }

    /// A key must never be reconstructable from what the UI receives.
    #[test]
    fn the_view_never_carries_the_key() {
        let p = Provider {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: "sk-or-v1-abcdefghijklmnop".into(),
        };
        let v = p.view();
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("sk-or-v1-abcdefghijklmnop"), "the key reached the frontend");
        assert!(!json.contains("abcdefghijkl"), "too much of the key reached the frontend");
        assert!(v.has_key);
        assert_eq!(v.key_hint, "mnop");
    }

    /// A short key is not redacted, it is withheld — four of six characters is
    /// not a hint, it is most of the secret.
    #[test]
    fn a_short_key_gets_no_hint() {
        assert_eq!(key_hint("sk-123"), "");
        assert_eq!(key_hint(""), "");
        assert_eq!(key_hint("0123456789ab"), "89ab");
    }

    #[test]
    fn models_are_parsed_from_the_openai_shape() {
        let body = r#"{"data":[{"id":"z-model"},{"id":"a-model"}]}"#;
        let got = parse_models(&format!("{body}\n200")).unwrap();
        assert_eq!(got, vec!["a-model", "z-model"], "not sorted");
    }

    #[test]
    fn a_bare_array_is_accepted_too() {
        let got = parse_models("[{\"id\":\"m1\"}]\n200").unwrap();
        assert_eq!(got, vec!["m1"]);
    }

    /// The three failures a user will actually hit, each needing its own
    /// sentence — "it didn't work" would send them to the wrong fix.
    #[test]
    fn failures_are_reported_distinctly() {
        let bad_key = parse_models("{\"error\":{\"message\":\"no\"}}\n401").unwrap_err();
        assert!(bad_key.contains("rejected"), "got: {bad_key}");

        let server = parse_models("{\"error\":{\"message\":\"upstream is down\"}}\n502").unwrap_err();
        assert!(server.contains("502") && server.contains("upstream is down"), "got: {server}");

        let html = parse_models("<html>not json</html>\n200").unwrap_err();
        assert!(html.contains("did not return JSON"), "got: {html}");

        let empty = parse_models("{\"data\":[]}\n200").unwrap_err();
        assert!(empty.contains("empty"), "got: {empty}");
    }
}
