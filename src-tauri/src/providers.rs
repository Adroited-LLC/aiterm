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

/// The ceiling that applies to one model: its own if it sets one, otherwise
/// the account default. A per-model ceiling *replaces* the account one rather
/// than merging field by field, so a model priced with one number does not
/// quietly inherit the other.
///
/// Shared by the request builder and the endpoint reader. Two copies of this
/// choice would be two answers the day one of them is edited — and the panel
/// would then mark rows "over cap" that the request happily routes to.
fn effective_cap<'a>(p: &'a Provider, model: &str) -> &'a MaxPrice {
    match p.routes.get(model) {
        Some(r) if !r.max_price.is_empty() => &r.max_price,
        _ => &p.policy.max_price,
    }
}

/// The `provider` object for one request: the account policy, plus whatever
/// this model asks for on top.
///
/// `None` when there is nothing to send, so an unrouted model's request is
/// byte-for-byte what it was before this feature existed.
///
/// A pin sets `allow_fallbacks: false` — "only that host", per the decision
/// recorded in the spec. A pinned host that is down, or priced above the
/// ceiling, fails the request rather than routing elsewhere. That is the
/// point; the caller is responsible for saying so when it reports the error.
pub fn routing_block(p: &Provider, model: &str) -> Option<serde_json::Value> {
    let route = p.routes.get(model);
    let cap = effective_cap(p, model);
    let pinned = route.map(|r| !r.order.is_empty()).unwrap_or(false);
    if p.policy.resolved_ignore.is_empty() && !pinned && cap.is_empty() {
        return None;
    }
    let mut b = serde_json::Map::new();
    if !p.policy.resolved_ignore.is_empty() {
        let slugs: Vec<&String> = p.policy.resolved_ignore.keys().collect();
        b.insert("ignore".into(), serde_json::json!(slugs));
    }
    if let Some(r) = route.filter(|_| pinned) {
        b.insert("order".into(), serde_json::json!(r.order));
        b.insert("allow_fallbacks".into(), serde_json::json!(false));
    }
    if !cap.is_empty() {
        b.insert("max_price".into(), serde_json::to_value(cap).ok()?);
    }
    Some(serde_json::Value::Object(b))
}

/// The inline OpenCode config that carries this model's routing.
///
/// `OPENCODE_CONFIG_CONTENT` *merges* over the user's own config — verified
/// 2026-08-10 by running `opencode models` with this set and finding their
/// `local/local` provider still listed. aiterm therefore never writes
/// `~/.config/opencode/opencode.json`.
///
/// The block goes under the model's `options`, which OpenCode copies onto the
/// request body as-is. Not `extraBody`: that key is passed through literally,
/// so OpenRouter would receive — and ignore — an `extraBody` object instead of
/// the routing it names.
///
/// Built with `serde_json`, never by formatting: the model id is user data
/// that lands inside a JSON key.
pub fn opencode_config_content(p: &Provider, model: &str) -> Option<String> {
    let block = routing_block(p, model)?;
    Some(
        serde_json::json!({
            "provider": {"openrouter": {"models": {model: {"options": {"provider": block}}}}}
        })
        .to_string(),
    )
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
    fetch(p, &format!("{}/models", p.base_url))
}

/// One authenticated GET as curl sees it: body, newline, HTTP status.
pub fn fetch(p: &Provider, url: &str) -> Result<String, String> {
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
            url,
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

/// One host's offer of one model.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct EndpointCard {
    pub provider_name: String,
    /// The routing slug — `novita`. What `order` and `ignore` take.
    pub slug: String,
    /// The full tag — `novita/fp8`. Shown, never sent: OpenRouter filters
    /// quantization through a separate field, so a pin cannot name one.
    pub tag: String,
    pub quantization: Option<String>,
    pub context_length: Option<u64>,
    /// USD per token, as quoted — the UI scales to $/M.
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    pub max_completion_tokens: Option<u64>,
    pub uptime_30m: Option<f64>,
    /// Why the stored policy rules this row out, or `None`.
    pub excluded: Option<String>,
}

/// The `/endpoints` reply as rows, annotated against the stored policy.
///
/// The annotation happens here, in the one place that already knows the
/// policy, rather than in the panel. Two implementations of "is this row
/// allowed" would be two answers the day one of them is edited.
pub fn parse_endpoints(
    response: &str,
    p: &Provider,
    model: &str,
) -> Result<Vec<EndpointCard>, String> {
    let body = checked_body(response)?;
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "The provider did not return JSON.".to_string())?;
    let items = v
        .pointer("/data/endpoints")
        .and_then(|e| e.as_array())
        .ok_or_else(|| "No endpoint list in the reply.".to_string())?;

    let cap = effective_cap(p, model);
    let price = |m: &serde_json::Value, key: &str| -> Option<f64> {
        let x = m.pointer(&format!("/pricing/{key}"))?;
        x.as_f64().or_else(|| x.as_str()?.trim().parse().ok())
    };

    Ok(items
        .iter()
        .filter_map(|e| {
            let tag = e
                .get("tag")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let slug = tag.split('/').next().unwrap_or("").to_string();
            let prompt_price = price(e, "prompt");
            let completion_price = price(e, "completion");
            // Per token here, per million in the ceiling.
            let over = |p_tok: Option<f64>, cap_m: Option<f64>| {
                matches!((p_tok, cap_m), (Some(t), Some(c)) if t * 1e6 > c)
            };
            let excluded = p
                .policy
                .resolved_ignore
                .get(&slug)
                .cloned()
                .or_else(|| {
                    (over(prompt_price, cap.prompt) || over(completion_price, cap.completion))
                        .then(|| "over cap".to_string())
                });
            Some(EndpointCard {
                provider_name: e.get("provider_name").and_then(|n| n.as_str())?.to_string(),
                slug,
                tag,
                quantization: e
                    .get("quantization")
                    .and_then(|q| q.as_str())
                    .map(String::from),
                context_length: e.get("context_length").and_then(|c| c.as_u64()),
                prompt_price,
                completion_price,
                max_completion_tokens: e.get("max_completion_tokens").and_then(|c| c.as_u64()),
                uptime_30m: e.get("uptime_last_30m").and_then(|u| u.as_f64()),
                excluded,
            })
        })
        .collect())
}

/// The endpoint list for one model. OpenRouter only — the path is theirs, and
/// a `/endpoints` call to a bare llama.cpp is a 404 nobody can act on.
#[tauri::command(async)]
pub fn provider_model_endpoints(id: String, model: String) -> Result<Vec<EndpointCard>, String> {
    let list = load();
    let p = list.iter().find(|p| p.id == id).ok_or("No such provider.")?;
    if !p.is_openrouter() {
        return Err("Provider routing is an OpenRouter feature.".into());
    }
    if p.api_key.is_empty() {
        return Err("No API key saved for this provider.".into());
    }
    let url = format!("{}/models/{model}/endpoints", p.base_url);
    parse_endpoints(&fetch(p, &url)?, p, &model)
}

/// One provider in OpenRouter's directory. Country is optional and often
/// missing — 29 of 101 providers reported none on 2026-08-10 — which is why
/// `block_unknown_country` exists as its own decision.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DirectoryEntry {
    pub slug: String,
    pub name: String,
    pub headquarters: Option<String>,
    #[serde(default)]
    pub datacenters: Vec<String>,
}

/// The `/providers` reply as directory rows.
pub fn parse_directory(response: &str) -> Result<Vec<DirectoryEntry>, String> {
    let body = checked_body(response)?;
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "The provider did not return JSON.".to_string())?;
    let items = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "No provider list in the reply.".to_string())?;
    Ok(items
        .iter()
        // A row with no `slug` is dropped: the slug is the only field a policy
        // can act on, so such a row could never be blocked or pinned anyway.
        .filter_map(|e| {
            Some(DirectoryEntry {
                slug: e.get("slug")?.as_str()?.to_string(),
                name: e
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                headquarters: e
                    .get("headquarters")
                    .and_then(|h| h.as_str())
                    .map(String::from),
                datacenters: e
                    .get("datacenters")
                    .and_then(|d| d.as_array())
                    .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
            })
        })
        .collect())
}

/// The policy compiled to slugs: what actually goes in `ignore`, and why.
///
/// A hand block wins the reason line, because it is the one the user typed and
/// the one they will look for when they wonder where a host went.
pub fn resolve_ignore(
    policy: &Policy,
    dir: &[DirectoryEntry],
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for e in dir {
        let countries: Vec<&str> = e
            .headquarters
            .iter()
            .map(String::as_str)
            .chain(e.datacenters.iter().map(String::as_str))
            .collect();
        if countries.is_empty() {
            if policy.block_unknown_country {
                out.insert(e.slug.clone(), "no country".to_string());
            }
        } else if let Some(c) = countries.iter().find(|c| {
            policy
                .blocked_countries
                .iter()
                .any(|b| b.eq_ignore_ascii_case(c))
        }) {
            out.insert(e.slug.clone(), c.to_string());
        }
    }
    // Hand blocks last: they overwrite a country reason, and they apply to
    // slugs the directory has never heard of.
    for slug in &policy.blocked_providers {
        out.insert(slug.clone(), "blocked by hand".to_string());
    }
    out
}

/// The provider directory — every host OpenRouter can route to, with the
/// country data a policy is written against. OpenRouter only, for the same
/// reason `/endpoints` is.
///
/// `#[tauri::command(async)]` rather than `run_blocking`, matching the other
/// commands here that make a network call: the curl is synchronous, and this
/// keeps it off the GTK main thread.
#[tauri::command(async)]
pub fn provider_directory(id: String) -> Result<Vec<DirectoryEntry>, String> {
    let list = load();
    let p = list.iter().find(|p| p.id == id).ok_or("No such provider.")?;
    if !p.is_openrouter() {
        return Err("Provider routing is an OpenRouter feature.".into());
    }
    if p.api_key.is_empty() {
        return Err("No API key saved for this provider.".into());
    }
    parse_directory(&fetch(p, &format!("{}/providers", p.base_url))?)
}

/// Save a policy, compiling it against the live directory first.
///
/// A directory that will not load is a hard error rather than a silent save:
/// storing a policy whose `resolved_ignore` is empty would read in the panel
/// as "nothing is blocked", which is the opposite of what was asked for.
#[tauri::command(async)]
pub fn provider_policy_set(id: String, policy: Policy) -> Result<Vec<ProviderView>, String> {
    let dir = provider_directory(id.clone())?;
    let mut list = load();
    let p = list
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or("No such provider.")?;
    let mut policy = policy;
    policy.resolved_ignore = resolve_ignore(&policy, &dir);
    policy.resolved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    p.policy = policy;
    save(&list)?;
    Ok(list.iter().map(Provider::view).collect())
}

/// Set or clear one model's route. An empty `order` with no ceiling removes
/// the entry rather than storing a route that says nothing.
///
/// OpenRouter only, for the same reason `/endpoints` and `/providers` are: a
/// route names hosts only OpenRouter can route to, so a stored route on any
/// other provider is dead weight the request path would never send.
pub fn set_route(
    list: &mut [Provider],
    id: &str,
    model: String,
    route: Route,
) -> Result<(), String> {
    let p = list
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or("No such provider.")?;
    if !p.is_openrouter() {
        return Err("Provider routing is an OpenRouter feature.".into());
    }
    if route.order.is_empty() && route.max_price.is_empty() {
        p.routes.remove(&model);
    } else {
        p.routes.insert(model, route);
    }
    Ok(())
}

#[tauri::command]
pub async fn provider_route_set(
    id: String,
    model: String,
    route: Route,
) -> Result<Vec<ProviderView>, String> {
    crate::run_blocking(move || provider_route_set_sync(id, model, route)).await
}

fn provider_route_set_sync(
    id: String,
    model: String,
    route: Route,
) -> Result<Vec<ProviderView>, String> {
    let mut list = load();
    set_route(&mut list, &id, model, route)?;
    save(&list)?;
    Ok(list.iter().map(Provider::view).collect())
}

/// One day of one model on one host, from OpenRouter's activity record.
///
/// Account-wide, not app-wide: this includes traffic aiterm never launched.
/// The panel says so — see Task 12.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct ActivityRow {
    pub date: String,
    pub model: String,
    pub provider_name: String,
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// USD.
    pub usage: f64,
}

/// `checked_body` with one activity-only exception.
///
/// OpenRouter answers `/activity` with `403` and a real sentence — "Only
/// management keys can fetch activity for an account" — when the key is good
/// but is an inference key. The canned "rejected that API key" would send the
/// user to replace a key that works, so the server's own sentence wins on this
/// path, and only on this path. A `403` with no `error.message`, and every
/// `401`, still read as a rejected key.
fn activity_body(response: &str) -> Result<&str, String> {
    let (body, status) = response.rsplit_once('\n').unwrap_or(("", response));
    if status.trim().parse::<u16>() == Ok(403) {
        let detail = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(String::from)
            });
        if let Some(d) = detail {
            return Err(d);
        }
    }
    checked_body(response)
}

/// The `/activity` reply as rows, exactly as OpenRouter gives them. Grouping
/// is the panel's job — one day of one model on one host is the finest grain
/// the record has, and summing it here would throw away the host column the
/// whole feature exists to show.
pub fn parse_activity(response: &str) -> Result<Vec<ActivityRow>, String> {
    let body = activity_body(response)?;
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "The provider did not return JSON.".to_string())?;
    let items = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "No activity in the reply.".to_string())?;
    Ok(items
        .iter()
        // A row with no `date` is dropped: every reading of this record is per
        // day, so a dateless row could only ever be counted under the wrong one.
        .filter_map(|r| {
            Some(ActivityRow {
                date: r.get("date")?.as_str()?.to_string(),
                model: r
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string(),
                provider_name: r
                    .get("provider_name")
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string(),
                requests: r.get("requests").and_then(|x| x.as_u64()).unwrap_or(0),
                prompt_tokens: r.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                completion_tokens: r
                    .get("completion_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0),
                usage: r.get("usage").and_then(|x| x.as_f64()).unwrap_or(0.0),
            })
        })
        .collect())
}

/// The account's recent activity. OpenRouter only, for the same reason
/// `/endpoints` and `/providers` are: the path is theirs, and no other provider
/// keeps a per-host record to read.
#[tauri::command(async)]
pub fn provider_activity(id: String) -> Result<Vec<ActivityRow>, String> {
    let list = load();
    let p = list.iter().find(|p| p.id == id).ok_or("No such provider.")?;
    if !p.is_openrouter() {
        return Err("Activity is an OpenRouter feature.".into());
    }
    if p.api_key.is_empty() {
        return Err("No API key saved for this provider.".into());
    }
    parse_activity(&fetch(p, &format!("{}/activity", p.base_url))?)
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

    /// An unconfigured provider must send the request it sent before this
    /// feature existed — not an empty `provider` object.
    #[test]
    fn nothing_configured_sends_no_routing_block() {
        let p = provider("openrouter");
        assert_eq!(routing_block(&p, "z-ai/glm-5.2"), None);
    }

    /// The compiled ignore list is account-wide, so it applies to a model with
    /// no route of its own — and on its own it is the whole block.
    #[test]
    fn a_policy_alone_sends_only_the_ignore_list() {
        let mut p = provider("openrouter");
        p.policy.resolved_ignore.insert("baidu".into(), "CN".into());
        p.policy.resolved_ignore.insert("streamlake".into(), "CN".into());
        assert_eq!(
            routing_block(&p, "z-ai/glm-5.2").unwrap(),
            serde_json::json!({"ignore": ["baidu", "streamlake"]}),
        );
    }

    /// A pin is "only that host": the order goes out with fallbacks explicitly
    /// off, so a pinned host that is down fails rather than silently routing
    /// somewhere the user did not choose.
    #[test]
    fn a_pin_means_only_that_host() {
        let mut p = provider("openrouter");
        p.routes.insert("z-ai/glm-5.2".into(), Route {
            order: vec!["novita".into()], allow_fallbacks: false, ..Default::default()
        });
        let b = routing_block(&p, "z-ai/glm-5.2").unwrap();
        assert_eq!(b["order"], serde_json::json!(["novita"]));
        assert_eq!(b["allow_fallbacks"], serde_json::json!(false));
    }

    /// Rust defaults `allow_fallbacks` to false and OpenRouter defaults it to
    /// true, so neither default can be leaned on. With no pin the key is left
    /// out entirely rather than sent as either value.
    #[test]
    fn an_unpinned_model_omits_allow_fallbacks_rather_than_sending_true() {
        let mut p = provider("openrouter");
        p.policy.max_price.completion = Some(2.5);
        let b = routing_block(&p, "z-ai/glm-5.2").unwrap();
        assert!(b.get("allow_fallbacks").is_none());
        assert!(b.get("order").is_none());
    }

    /// A per-model ceiling is the whole ceiling for that model. Merging it
    /// field-by-field with the account default would mean a model priced with
    /// one number quietly inherits the other.
    #[test]
    fn a_models_ceiling_replaces_the_policy_ceiling_rather_than_merging() {
        let mut p = provider("openrouter");
        p.policy.max_price = MaxPrice { prompt: Some(1.0), completion: Some(2.5) };
        p.routes.insert("z-ai/glm-5.2".into(), Route {
            max_price: MaxPrice { prompt: None, completion: Some(1.8) },
            ..Default::default()
        });
        let b = routing_block(&p, "z-ai/glm-5.2").unwrap();
        assert_eq!(b["max_price"], serde_json::json!({"completion": 1.8}));
        // The other model still gets the account default.
        let b2 = routing_block(&p, "z-ai/glm-5.1").unwrap();
        assert_eq!(b2["max_price"], serde_json::json!({"prompt": 1.0, "completion": 2.5}));
    }

    /// Routes are keyed by model and stay that way — pinning one model must
    /// not quietly pin every other model on the account.
    #[test]
    fn a_route_for_another_model_does_not_leak_into_this_one() {
        let mut p = provider("openrouter");
        p.routes.insert("z-ai/glm-5.1".into(), Route {
            order: vec!["novita".into()], allow_fallbacks: false, ..Default::default()
        });
        assert_eq!(routing_block(&p, "z-ai/glm-5.2"), None);
    }

    /* ---- the OpenCode environment ---------------------------------------- */

    /// OpenCode reads routing from a model's `options`, which it copies onto
    /// the request body as-is.
    #[test]
    fn opencode_gets_the_routing_block_under_the_models_options() {
        let mut p = provider("openrouter");
        p.policy.resolved_ignore.insert("baidu".into(), "CN".into());
        p.routes.insert("z-ai/glm-5.2".into(), Route {
            order: vec!["novita".into()], allow_fallbacks: false, ..Default::default()
        });
        let text = opencode_config_content(&p, "z-ai/glm-5.2").unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let opts = &v["provider"]["openrouter"]["models"]["z-ai/glm-5.2"]["options"];
        assert_eq!(opts["provider"]["order"], serde_json::json!(["novita"]));
        assert_eq!(opts["provider"]["allow_fallbacks"], serde_json::json!(false));
        // Verified 2026-08-10: OpenCode passes `options` straight onto the body,
        // and does NOT unwrap `extraBody`. An extraBody key here would be silently
        // ignored by OpenRouter.
        assert!(opts.get("extraBody").is_none());
    }

    /// Nothing to route, nothing in the environment: the tab starts with the
    /// environment it had before this feature existed.
    #[test]
    fn an_unrouted_model_sets_no_environment_variable() {
        let p = provider("openrouter");
        assert_eq!(opencode_config_content(&p, "z-ai/glm-5.2"), None);
    }

    /// The model id is user data and it lands inside a JSON *key*, so the
    /// content is built by serde_json and never by formatting.
    #[test]
    fn a_model_id_with_awkward_characters_stays_valid_json() {
        let mut p = provider("openrouter");
        p.routes.insert("weird/\"quote\"".into(), Route {
            order: vec!["novita".into()], allow_fallbacks: false, ..Default::default()
        });
        let text = opencode_config_content(&p, "weird/\"quote\"").unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).expect("must be valid JSON");
        assert!(v["provider"]["openrouter"]["models"]["weird/\"quote\""].is_object());
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

    /// A route names an OpenRouter host, so a provider that is not OpenRouter
    /// must be refused rather than left holding a route nothing will ever send.
    #[test]
    fn a_route_cannot_be_stored_on_a_provider_that_is_not_openrouter() {
        let mut list = vec![provider("local"), provider("openrouter")];
        list[1].base_url = "https://openrouter.ai/api/v1".into();
        let route = Route {
            order: vec!["novita".into()],
            allow_fallbacks: false,
            ..Default::default()
        };
        let err = set_route(&mut list, "local", "z-ai/glm-5.2".into(), route.clone()).unwrap_err();
        assert_eq!(err, "Provider routing is an OpenRouter feature.");
        assert!(list[0].routes.is_empty());
        // The same route on the OpenRouter provider still stores.
        set_route(&mut list, "openrouter", "z-ai/glm-5.2".into(), route).unwrap();
        assert_eq!(list[1].routes["z-ai/glm-5.2"].order, vec!["novita"]);
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

    /// Trimmed from a real `/models/z-ai/glm-5.2/endpoints` reply, 2026-08-10.
    const ENDPOINTS: &str = r#"{"data":{"id":"z-ai/glm-5.2","endpoints":[
      {"name":"Sail Research | z-ai/glm-5.2","provider_name":"Sail Research",
       "tag":"sail-research/fp8","quantization":"fp8","context_length":1048576,
       "max_completion_tokens":131072,"uptime_last_30m":99.34,
       "pricing":{"prompt":"0.0000005","completion":"0.00000315"}},
      {"name":"Novita | z-ai/glm-5.2","provider_name":"Novita",
       "tag":"novita/fp8","quantization":"fp8","context_length":1048576,
       "max_completion_tokens":131072,"uptime_last_30m":98.71,
       "pricing":{"prompt":"0.0000005026","completion":"0.00000158"}},
      {"name":"Baidu | z-ai/glm-5.2","provider_name":"Baidu",
       "tag":"baidu/fp8","quantization":"fp8","context_length":1048576,
       "max_completion_tokens":131072,"uptime_last_30m":97.0,
       "pricing":{"prompt":"0.000000504","completion":"0.000001584"}}]}}
200"#;

    #[test]
    fn an_endpoint_reply_becomes_rows_with_routing_slugs() {
        let p = provider("openrouter");
        let rows = parse_endpoints(ENDPOINTS, &p, "z-ai/glm-5.2").unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].provider_name, "Novita");
        // The tag is shown; the slug before the slash is what a pin sends.
        assert_eq!(rows[1].tag, "novita/fp8");
        assert_eq!(rows[1].slug, "novita");
        assert_eq!(rows[1].completion_price, Some(0.00000158));
        assert_eq!(rows[1].max_completion_tokens, Some(131072));
        assert!(rows.iter().all(|r| r.excluded.is_none()));
    }

    #[test]
    fn rows_carry_the_reason_the_policy_excludes_them() {
        let mut p = provider("openrouter");
        p.policy.resolved_ignore.insert("baidu".into(), "CN".into());
        // $2/M completion: Sail Research at $3.15/M is over, Novita at $1.58/M is not.
        p.policy.max_price.completion = Some(2.0);
        let rows = parse_endpoints(ENDPOINTS, &p, "z-ai/glm-5.2").unwrap();
        assert_eq!(rows[0].excluded.as_deref(), Some("over cap"));
        assert_eq!(rows[1].excluded, None);
        assert_eq!(rows[2].excluded.as_deref(), Some("CN"));
    }

    #[test]
    fn a_models_own_ceiling_decides_which_rows_are_over_cap() {
        let mut p = provider("openrouter");
        p.policy.max_price.completion = Some(5.0);
        p.routes.insert("z-ai/glm-5.2".into(), Route {
            max_price: MaxPrice { prompt: None, completion: Some(1.6) },
            ..Default::default()
        });
        let rows = parse_endpoints(ENDPOINTS, &p, "z-ai/glm-5.2").unwrap();
        assert_eq!(rows[0].excluded.as_deref(), Some("over cap"));  // $3.15/M
        assert_eq!(rows[1].excluded, None);                         // $1.58/M
        assert_eq!(rows[2].excluded, None);                         // $1.584/M... under 1.6
    }

    fn directory() -> Vec<DirectoryEntry> {
        vec![
            DirectoryEntry {
                slug: "novita".into(),
                name: "Novita".into(),
                headquarters: Some("US".into()),
                datacenters: vec![],
            },
            DirectoryEntry {
                slug: "baidu".into(),
                name: "Baidu".into(),
                headquarters: Some("CN".into()),
                datacenters: vec![],
            },
            // Headquartered Singapore, serving from China — the case that makes
            // reading only `headquarters` wrong.
            DirectoryEntry {
                slug: "alibaba".into(),
                name: "Alibaba".into(),
                headquarters: Some("SG".into()),
                datacenters: vec!["SG".into(), "CN".into()],
            },
            DirectoryEntry {
                slug: "mystery".into(),
                name: "Mystery".into(),
                headquarters: None,
                datacenters: vec![],
            },
        ]
    }

    #[test]
    fn a_country_block_catches_headquarters_and_datacenters() {
        let policy = Policy {
            blocked_countries: vec!["CN".into()],
            ..Default::default()
        };
        let out = resolve_ignore(&policy, &directory());
        assert_eq!(out.get("baidu").map(String::as_str), Some("CN"));
        assert_eq!(out.get("alibaba").map(String::as_str), Some("CN"));
        assert!(!out.contains_key("novita"));
        assert!(!out.contains_key("mystery"), "unknown is not blocked unless asked");
    }

    #[test]
    fn blocking_unknown_countries_is_a_separate_decision() {
        let policy = Policy {
            blocked_countries: vec!["CN".into()],
            block_unknown_country: true,
            ..Default::default()
        };
        let out = resolve_ignore(&policy, &directory());
        assert_eq!(out.get("mystery").map(String::as_str), Some("no country"));
        assert!(!out.contains_key("novita"));
    }

    #[test]
    fn a_hand_block_needs_no_country_data_and_wins_the_reason() {
        let policy = Policy {
            blocked_providers: vec!["novita".into(), "baidu".into()],
            blocked_countries: vec!["CN".into()],
            ..Default::default()
        };
        let out = resolve_ignore(&policy, &directory());
        assert_eq!(out.get("novita").map(String::as_str), Some("blocked by hand"));
        assert_eq!(out.get("baidu").map(String::as_str), Some("blocked by hand"));
    }

    #[test]
    fn a_directory_reply_parses_both_country_fields() {
        let body = r#"{"data":[
          {"slug":"novita","name":"Novita","headquarters":"US","datacenters":null},
          {"slug":"alibaba","name":"Alibaba","headquarters":"SG","datacenters":["SG","CN"]}]}
200"#;
        let dir = parse_directory(body).unwrap();
        assert_eq!(dir[0].datacenters, Vec::<String>::new());
        assert_eq!(dir[1].datacenters, vec!["SG", "CN"]);
        assert_eq!(dir[1].headquarters.as_deref(), Some("SG"));
    }

    #[test]
    fn an_activity_reply_becomes_rows_with_dollars_and_hosts() {
        let body = r#"{"data":[
          {"date":"2026-08-09","model":"z-ai/glm-5.2","provider_name":"Novita",
           "requests":5,"prompt_tokens":50,"completion_tokens":125,"usage":0.015},
          {"date":"2026-08-09","model":"z-ai/glm-5.2","provider_name":"Baidu",
           "requests":2,"prompt_tokens":20,"completion_tokens":40,"usage":0.004}]}
200"#;
        let rows = parse_activity(body).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].provider_name, "Baidu");
        assert_eq!(rows[0].usage, 0.015);
        assert_eq!(rows[0].requests, 5);
    }

    #[test]
    fn an_activity_row_missing_optional_counts_still_parses() {
        let body = r#"{"data":[{"date":"2026-08-09","model":"m","provider_name":"X"}]}
200"#;
        let rows = parse_activity(body).unwrap();
        assert_eq!(rows[0].requests, 0);
        assert_eq!(rows[0].usage, 0.0);
    }

    /// The 403 a working inference key gets from `/activity` — verified live,
    /// 2026-08-10. The key is fine; it is the wrong class of key. Saying
    /// "rejected that API key" would send the user to replace it.
    #[test]
    fn an_activity_403_says_what_the_server_said() {
        let body = r#"{"error":{"message":"Only management keys can fetch activity for an account"}}
403"#;
        let err = parse_activity(body).unwrap_err();
        assert_eq!(
            err, "Only management keys can fetch activity for an account",
            "got: {err}"
        );
    }

    /// A genuinely bad key still reads as a bad key here, like everywhere else.
    #[test]
    fn an_activity_401_is_still_a_rejected_key() {
        let err = parse_activity("{\"error\":{\"message\":\"no\"}}\n401").unwrap_err();
        assert_eq!(err, "The provider rejected that API key.", "got: {err}");
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
