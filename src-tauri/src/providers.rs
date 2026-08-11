//! API model access — OpenRouter, OpenAI, or anything else speaking the same
//! `/models` + `/chat/completions` shape.
//!
//! Configuration lives here; the engine is `aiterm chat` (src/chat.rs) — a
//! console harness the new-session menu launches in a tab's PTY exactly the
//! way it launches `claude`, loaded with a model off a provider's startup
//! shortlist. **Test** proves a key works by asking for the model list.
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

/// A ceiling in USD per *million* tokens — OpenRouter's unit for `max_price`,
/// which is not the per-token unit `/models` quotes. Convert at the boundary.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct MaxPrice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<f64>,
}

impl MaxPrice {
    pub fn is_empty(&self) -> bool {
        self.prompt.is_none() && self.completion.is_none()
    }
}

/// Which hosts this account will not use, and the most it will pay.
///
/// `resolved_ignore` is the policy *compiled* against the provider directory:
/// slug → the reason it is out. It is stored rather than computed per request
/// because both places that build a request — a CLI process in a pty, and a
/// launch one keystroke from a running terminal — are the wrong places for a
/// network call.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Policy {
    #[serde(default)]
    pub blocked_countries: Vec<String>,
    #[serde(default)]
    pub block_unknown_country: bool,
    #[serde(default)]
    pub blocked_providers: Vec<String>,
    #[serde(default)]
    pub max_price: MaxPrice,
    #[serde(default)]
    pub resolved_ignore: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub resolved_at: u64,
}

/// What one model prefers. An empty `order` is "no pin".
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Route {
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub allow_fallbacks: bool,
    #[serde(default)]
    pub max_price: MaxPrice,
}

/// A configured provider, as stored.
///
/// `Debug` is written by hand rather than derived: a derived one prints
/// `api_key` in full, and the whole point of [`ProviderView`] is that the key
/// does not leave this module. One `{:?}` in a log line or an error message
/// would undo that, and nothing about the derive would warn you.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Provider {
    /// Stable slug, generated from the name on first save. Used as the key for
    /// updates and deletes so renaming a provider does not orphan it.
    pub id: String,
    pub name: String,
    /// Base URL *without* a trailing slash or `/models` — e.g.
    /// `https://openrouter.ai/api/v1`.
    pub base_url: String,
    pub api_key: String,
    /// Model ids picked for the new-session menu — the shortlist, not the
    /// catalog. `default` so files written before this field existed load.
    #[serde(default)]
    pub startup_models: Vec<String>,
    /// Routing policy for this provider. `default` so files written before
    /// this field existed load.
    #[serde(default)]
    pub policy: Policy,
    /// Per-model routing, keyed by model id. A route outlives its star, so
    /// re-adding a model to the startup list restores its pin.
    #[serde(default)]
    pub routes: std::collections::BTreeMap<String, Route>,
}

/// Replace a provider's startup shortlist. Order is the caller's; duplicates
/// fold to their first appearance.
pub fn set_startup_models(
    list: &mut [Provider],
    id: &str,
    models: Vec<String>,
) -> Result<(), String> {
    let p = list
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or("No such provider.")?;
    let mut seen = std::collections::HashSet::new();
    p.startup_models = models.into_iter().filter(|m| seen.insert(m.clone())).collect();
    Ok(())
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
    pub startup_models: Vec<String>,
    pub policy: Policy,
    pub routes: std::collections::BTreeMap<String, Route>,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &format_args!("<{} chars>", self.api_key.len()))
            .finish()
    }
}

impl Provider {
    /// Is this OpenRouter?
    ///
    /// Asked in three places that must agree: which providers OpenCode will
    /// run models from, which of its models are offered, and whether
    /// `pty_spawn` puts a key in the tab's environment. Two of those are
    /// security-shaped, so the test lives here once rather than being spelled
    /// out wherever it is needed.
    pub fn is_openrouter(&self) -> bool {
        self.base_url.contains("openrouter.ai")
    }

    fn view(&self) -> ProviderView {
        ProviderView {
            id: self.id.clone(),
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            has_key: !self.api_key.is_empty(),
            key_hint: key_hint(&self.api_key),
            startup_models: self.startup_models.clone(),
            policy: self.policy.clone(),
            routes: self.routes.clone(),
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

/// The stored providers, keys included — for in-process readers only
/// (`aiterm chat` looks its provider up by id). Never expose over IPC.
pub fn load_providers() -> Vec<Provider> {
    load()
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
    // Created 0600 rather than created-then-chmodded. Writing first and
    // restricting after leaves a window — however short — where a file full of
    // API keys is readable at whatever the umask allows, and any other process
    // on the machine only has to open it once. The mode goes on at open(2), so
    // there is no window to lose the race in.
    write_private(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    // Belt and braces for a file that already existed with looser permissions:
    // `create(true)` reuses the old inode and its old mode.
    restrict(&path, 0o600);
    Ok(())
}

#[cfg(unix)]
pub(crate) fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(not(unix))]
pub(crate) fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

#[cfg(unix)]
fn restrict(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path, _mode: u32) {}

#[tauri::command]
pub async fn providers_list() -> Vec<ProviderView> {
    crate::run_blocking(providers_list_sync).await
}

fn providers_list_sync() -> Vec<ProviderView> {
    load().iter().map(Provider::view).collect()
}

/// Add or update a provider.
///
/// An empty `api_key` on an existing provider keeps the stored one, so editing
/// a base URL does not require re-entering the secret — the UI cannot show it
/// back, so asking for it again to change something unrelated would mean
/// finding it again every time.
#[tauri::command]
pub async fn provider_save(
    id: Option<String>,
    name: String,
    base_url: String,
    api_key: String,
) -> Result<Vec<ProviderView>, String> {
    crate::run_blocking(move || provider_save_sync(id, name, base_url, api_key)).await
}

fn provider_save_sync(
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
            startup_models: vec![],
            policy: Default::default(),
            routes: Default::default(),
        }),
    }
    save(&list)?;
    Ok(list.iter().map(Provider::view).collect())
}

#[tauri::command]
pub async fn provider_delete(id: String) -> Result<Vec<ProviderView>, String> {
    crate::run_blocking(move || provider_delete_sync(id)).await
}

fn provider_delete_sync(id: String) -> Result<Vec<ProviderView>, String> {
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
    parse_models(&fetch_models_response(&id)?)
}

/// The same `/models` call, kept whole — the browser's card view wants every
/// field the provider offered, where Test wants only the ids.
#[tauri::command(async)]
pub fn provider_model_cards(id: String) -> Result<Vec<ModelCard>, String> {
    parse_model_cards(&fetch_models_response(&id)?)
}

/// Replace a provider's startup shortlist and persist it.
#[tauri::command]
pub async fn provider_startup_set(
    id: String,
    models: Vec<String>,
) -> Result<Vec<ProviderView>, String> {
    crate::run_blocking(move || provider_startup_set_sync(id, models)).await
}

fn provider_startup_set_sync(id: String, models: Vec<String>) -> Result<Vec<ProviderView>, String> {
    let mut list = load();
    set_startup_models(&mut list, &id, models)?;
    save(&list)?;
    Ok(list.iter().map(Provider::view).collect())
}

/// One `GET {base}/models` as curl sees it: body, newline, HTTP status.
fn fetch_models_response(id: &str) -> Result<String, String> {
    let list = load();
    let p = list.iter().find(|p| p.id == id).ok_or("No such provider.")?;
    if p.api_key.is_empty() {
        return Err("No API key saved for this provider.".into());
    }
    let url = format!("{}/models", p.base_url);
    // The key goes in on stdin, never on the argv. `/proc/<pid>/cmdline` is
    // world-readable on Linux, so `-H "Authorization: Bearer …"` publishes the
    // secret to every process on the machine for as long as curl runs — and
    // `ps` is how you would look at a hung request. `--config -` takes the same
    // header from a config file read from stdin, which no other process sees.
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
            "Content-Type: application/json",
            "--config",
            "-",
            &url,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run curl: {e}"))
        .and_then(|mut child| {
            use std::io::Write;
            let config = curl_auth_config(&p.api_key);
            child
                .stdin
                .take()
                .ok_or_else(|| "curl took no stdin".to_string())?
                .write_all(config.as_bytes())
                .map_err(|e| format!("Could not pass the key to curl: {e}"))?;
            child
                .wait_with_output()
                .map_err(|e| format!("Could not run curl: {e}"))
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Could not reach {url} — {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// One `header` line in curl config syntax, carrying the bearer token.
///
/// curl's config parser reads a double-quoted value with backslash escapes, so
/// the two characters that could end the value early are escaped. An API key
/// containing either is not a realistic shape, but a quoting bug here fails by
/// sending a truncated credential and reading as "the provider rejected that
/// key", which is the worst way for this to be wrong.
///
/// `pub(crate)` because `usage.rs` sends a bearer token the same way. It could
/// hold its own copy of these two lines, and that is the version of this that
/// goes wrong: an escaping rule with two implementations is one that gets fixed
/// in one of them.
pub(crate) fn curl_auth_config(token: &str) -> String {
    let escaped = token.replace('\\', "\\\\").replace('"', "\\\"");
    format!("header = \"Authorization: Bearer {escaped}\"\n")
}

/// What a provider is willing to say about one model. Everything past the id
/// is optional: OpenRouter fills all of it, a bare llama.cpp fills none, and
/// the card in the UI shows what it was given.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct ModelCard {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_length: Option<u64>,
    /// USD per input token, as quoted — the UI scales to $/M.
    pub prompt_price: Option<f64>,
    /// USD per output token.
    pub completion_price: Option<f64>,
    /// What the model accepts: "text", "image", …
    pub modalities: Vec<String>,
}

/// Split the `-w` status off the body and turn an HTTP failure into the
/// sentence the UI should show. Shared by every reader of a `/models` reply,
/// so a rejected key reads the same everywhere.
fn checked_body(response: &str) -> Result<&str, String> {
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
    Ok(body)
}

/// The model entries of a `/models` reply. OpenAI-compatible is
/// `{"data":[{"id":…}]}`; a few servers return a bare array, so accept that
/// too rather than calling a working provider broken.
fn model_items(body: &str) -> Result<Vec<serde_json::Value>, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "The provider did not return JSON.".to_string())?;
    v.get("data")
        .and_then(|d| d.as_array())
        .or_else(|| v.as_array())
        .map(|a| a.to_vec())
        .ok_or_else(|| "No model list in the reply.".to_string())
}

/// The full `/models` reply as cards, one per model, in the provider's order.
///
/// A price can arrive as a string ("0.000003", OpenRouter) or a number;
/// modalities under `architecture.input_modalities`, or nothing at all.
/// Absent is absent — the card renders what it was given and no more.
pub fn parse_model_cards(response: &str) -> Result<Vec<ModelCard>, String> {
    let items = model_items(checked_body(response)?)?;
    let price = |m: &serde_json::Value, key: &str| -> Option<f64> {
        let p = m.pointer(&format!("/pricing/{key}"))?;
        p.as_f64().or_else(|| p.as_str()?.trim().parse().ok())
    };
    let cards: Vec<ModelCard> = items
        .iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(|i| i.as_str()).or_else(|| m.as_str())?;
            Some(ModelCard {
                id: id.to_string(),
                name: m.get("name").and_then(|v| v.as_str()).map(String::from),
                description: m
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                context_length: m.get("context_length").and_then(|v| v.as_u64()),
                prompt_price: price(m, "prompt"),
                completion_price: price(m, "completion"),
                modalities: m
                    .pointer("/architecture/input_modalities")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect();
    if cards.is_empty() {
        return Err("The provider returned an empty model list.".into());
    }
    Ok(cards)
}

/// Split the `-w` status off the body and pull out model ids.
///
/// Kept separate from the request so the parsing — which is where the shapes
/// actually differ between providers — can be tested without a network.
pub fn parse_models(response: &str) -> Result<Vec<String>, String> {
    let mut models: Vec<String> = model_items(checked_body(response)?)?
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

    fn provider(id: &str) -> Provider {
        Provider {
            id: id.into(),
            name: id.into(),
            base_url: "https://example.test/v1".into(),
            api_key: "k".into(),
            startup_models: vec![],
            policy: Default::default(),
            routes: Default::default(),
        }
    }

    /// The startup list is a replace: what arrives is what is kept, minus
    /// duplicates, and naming a provider that does not exist is an error
    /// rather than a silent no-op.
    #[test]
    fn startup_models_replace_dedupe_and_require_a_real_provider() {
        let mut list = vec![provider("openrouter")];
        set_startup_models(
            &mut list,
            "openrouter",
            vec!["a/one".into(), "b/two".into(), "a/one".into()],
        )
        .unwrap();
        assert_eq!(list[0].startup_models, vec!["a/one", "b/two"]);
        assert!(set_startup_models(&mut list, "nope", vec![]).is_err());
    }

    /// A 0.10.40 file predates routing entirely: no `policy`, no `routes`. It
    /// must load with the shortlist intact and routing simply switched off,
    /// because the alternative is an upgrade that reads as "no providers".
    #[test]
    fn a_providers_file_without_policy_or_routes_still_loads() {
        let old = r#"[{"id":"openrouter","name":"OpenRouter",
            "base_url":"https://openrouter.ai/api/v1","api_key":"k",
            "startup_models":["z-ai/glm-5.2"]}]"#;
        let list: Vec<Provider> = serde_json::from_str(old).expect("0.10.40 file must load");
        assert_eq!(list[0].startup_models, vec!["z-ai/glm-5.2"]);
        assert!(list[0].routes.is_empty());
        assert!(list[0].policy.blocked_countries.is_empty());
        assert!(!list[0].policy.block_unknown_country);
        // No stored ceiling means no ceiling — an upgrade must not start
        // refusing hosts on a price the user never set.
        assert!(list[0].policy.max_price.is_empty());
    }

    /// Everything routing stores has to survive a save and a load — the
    /// compiled ignore list included, since that is what goes on the wire and
    /// nothing recomputes it at request time.
    #[test]
    fn a_policy_and_a_route_round_trip_through_json() {
        let mut p = provider("openrouter");
        p.policy.blocked_countries = vec!["CN".into()];
        p.policy.block_unknown_country = true;
        p.policy.max_price.completion = Some(2.5);
        p.policy.resolved_ignore.insert("baidu".into(), "CN".into());
        p.policy.resolved_at = 1786000000;
        p.routes.insert(
            "z-ai/glm-5.2".into(),
            Route {
                order: vec!["novita".into()],
                allow_fallbacks: false,
                max_price: MaxPrice {
                    prompt: None,
                    completion: Some(1.8),
                },
            },
        );
        let text = serde_json::to_string(&[p.clone()]).unwrap();
        let back: Vec<Provider> = serde_json::from_str(&text).unwrap();
        assert_eq!(back[0], p);
    }

    /// providers.json is plain text and people edit it. A route typed by hand
    /// is unlikely to carry every key, and one missing key must not fail the
    /// parse of the whole file — `load()` turns any parse error into "no
    /// providers at all", so a half-written route would silently cost the user
    /// their keys until they noticed.
    #[test]
    fn a_hand_edited_route_missing_fields_still_loads() {
        let hand_edited = r#"[{"id":"openrouter","name":"OpenRouter",
            "base_url":"https://openrouter.ai/api/v1","api_key":"k",
            "routes":{"z-ai/glm-5.2":{"max_price":{"completion":1.8}}}}]"#;
        let list: Vec<Provider> =
            serde_json::from_str(hand_edited).expect("a partial route must not fail the file");
        let route = &list[0].routes["z-ai/glm-5.2"];
        assert!(route.order.is_empty());
        assert!(!route.allow_fallbacks);
        assert_eq!(route.max_price.completion, Some(1.8));
    }

    /// providers.json written before startup lists existed has no such key —
    /// it must load as an empty list, not fail.
    #[test]
    fn a_providers_file_from_before_startup_lists_still_loads() {
        let p: Provider = serde_json::from_str(
            r#"{"id":"op","name":"OpenRouter","base_url":"https://x","api_key":"k"}"#,
        )
        .unwrap();
        assert!(p.startup_models.is_empty());
    }

    /// OpenRouter's shape, cut down to the fields the card shows. Prices come
    /// as strings of USD-per-token; context and modalities live under their
    /// own keys. One model with everything, to prove each field survives.
    #[test]
    fn openrouter_metadata_becomes_a_full_card() {
        let body = r#"{"data":[{
            "id":"anthropic/claude-sonnet-5",
            "name":"Anthropic: Claude Sonnet 5",
            "description":"Fast frontier model.",
            "context_length":1000000,
            "architecture":{"input_modalities":["text","image"],"output_modalities":["text"]},
            "pricing":{"prompt":"0.000003","completion":"0.000015"}
        }]}"#;
        let cards = parse_model_cards(&format!("{body}\n200")).unwrap();
        assert_eq!(
            cards,
            vec![ModelCard {
                id: "anthropic/claude-sonnet-5".into(),
                name: Some("Anthropic: Claude Sonnet 5".into()),
                description: Some("Fast frontier model.".into()),
                context_length: Some(1_000_000),
                prompt_price: Some(0.000003),
                completion_price: Some(0.000015),
                modalities: vec!["text".into(), "image".into()],
            }]
        );
    }

    /// A plain OpenAI-compatible server says almost nothing about its models.
    /// That must produce a usable id-only card, not an error and not invented
    /// values.
    #[test]
    fn a_bare_model_list_becomes_id_only_cards() {
        let body = r#"{"data":[{"id":"llama-3.3-70b","object":"model","created":1700000000}]}"#;
        let cards = parse_model_cards(&format!("{body}\n200")).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, "llama-3.3-70b");
        assert_eq!(cards[0].name, None);
        assert_eq!(cards[0].prompt_price, None);
        assert!(cards[0].modalities.is_empty());
    }

    /// The cards path reports HTTP failures the same way the Test button does
    /// — a rejected key must say so, not "could not parse".
    #[test]
    fn cards_report_a_rejected_key_as_such() {
        let err = parse_model_cards("{}\n401").unwrap_err();
        assert!(err.contains("rejected"), "{err}");
    }

    #[test]
    fn the_key_goes_in_a_config_line_and_not_on_the_argv() {
        assert_eq!(
            curl_auth_config("sk-abc123"),
            "header = \"Authorization: Bearer sk-abc123\"\n"
        );
    }

    #[test]
    fn a_key_cannot_close_the_config_value_early() {
        // Anything that would end the quoted value early is escaped, so the
        // whole key reaches curl instead of a truncated one that reads back as
        // a rejected credential.
        let line = curl_auth_config(r#"a"b\c"#);
        assert_eq!(line, "header = \"Authorization: Bearer a\\\"b\\\\c\"\n");
        assert_eq!(line.matches('"').count() - line.matches("\\\"").count(), 2);
    }

    #[test]
    fn debugging_a_provider_never_prints_its_key() {
        let p = Provider {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: "sk-do-not-print-me".into(),
            startup_models: vec![],
            policy: Default::default(),
            routes: Default::default(),
        };
        let shown = format!("{p:?}");
        assert!(!shown.contains("sk-do-not-print-me"), "key leaked: {shown}");
        assert!(shown.contains("OpenRouter"));
    }

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
            startup_models: vec![],
            policy: Default::default(),
            routes: Default::default(),
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
