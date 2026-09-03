//! Where you stand with every service aiterm can see: plan limits for the
//! agents it launches, and credit balances for the API providers it is
//! configured with.
//!
//! Four sources, each read the way its own tool reads it:
//!
//! * **Claude** — `GET https://api.anthropic.com/api/oauth/usage` with the
//!   OAuth token Claude Code stores in `~/.claude/.credentials.json`, plus the
//!   `anthropic-beta: oauth-2025-04-20` header. This is the same call Claude
//!   Code's own `/usage` view makes. Gives the `limits` array (session /
//!   weekly-all / weekly-scoped) and a `spend` object for extra-usage credits.
//! * **Codex** — `GET https://chatgpt.com/backend-api/wham/usage` with the
//!   ChatGPT access token from `~/.codex/auth.json`. Found by running `strings`
//!   over the codex binary (`…/codex-linux-x64/vendor/…/bin/codex`), which
//!   carries both `/wham/usage` and its `/api/codex/usage` alias, then curled
//!   by hand: `/wham/usage` answers 200, `/api/codex/usage` answers 403, so
//!   only the first is used. The `chatgpt-account-id` header is what codex
//!   sends; the endpoint answers 200 without it too, but it is sent anyway
//!   because a workspace account is the case where it will start mattering and
//!   omitting it there would silently read the wrong account.
//!
//!   Nothing else codex ships reports usage. `codex --help` has no usage
//!   subcommand, and `codex doctor --json` — which is the machine-readable
//!   surface, with checks for auth, config, install, network and state — has no
//!   rate-limit or quota check in it. The TUI's own `/usage` view is fed by the
//!   endpoint above.
//! * **Grok** — `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`
//!   with the OIDC access token grok stores in `~/.grok/auth.json`. Found the
//!   same way: `strings` over `~/.grok/bin/grok` has a `billing.rs` extension
//!   carrying the path `/billing?format=credits`, the header name
//!   `x-grok-client-mode`, and a response struct (`creditUsagePercent`,
//!   `currentPeriod`, `prepaidBalance`, `onDemandCap`…); the base was found by
//!   curling the path under each host the binary names — the CLI's chat proxy
//!   answers 200, `api.x.ai` and `grok.com/rest` 404. The header turned out
//!   not to matter and is not sent. The reply is one rolling window (weekly,
//!   for a SuperGrok account) as a percent used with its start and end, plus a
//!   per-product split (Grok Build, Imagine, App Builder, Chat).
//!
//!   The token expires every few hours and grok refreshes it whenever it runs.
//!   This module never refreshes it itself: the binary's own log lines talk of
//!   "sibling rotation" and "refresh-token double-spend", which is a rotating
//!   refresh token, and spending it from here would sign the CLI out. An
//!   expired token is reported as *rejected* with "open grok", not repaired.
//! * **Antigravity** — `agy -p /usage --output-format json`, the CLI's own
//!   read-only slash command, which answers without a model call and
//!   without leaving a conversation behind (verified: no new
//!   `conversations/<id>.db` after a slash-only run). Its
//!   `command.data.groups[].buckets[]` carry `remaining_fraction` and
//!   `reset_time` for two groups (Gemini; Claude and GPT) × two windows
//!   (weekly; 5-hour). The backend endpoint is
//!   `daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary`
//!   behind a consumer OAuth token in the OS keyring with a private request
//!   proto, so the CLI is the supported path — at ~1–2 s per spawn (it
//!   starts a language server every time), so the answer is cached for five
//!   minutes and never fetched twice at once. `/credits` rides along for G1
//!   credits. The account email is read off the CLI's own log, the only
//!   place it is written. [observed: agy 1.1.24]
//! * **API providers** — `GET {base_url}/credits` with the saved bearer token.
//!   Verified against OpenRouter, which answers
//!   `{"data":{"total_credits":…,"total_usage":…}}`. Any provider whose reply
//!   does not have that shape is reported as *publishing no balance*, not as
//!   broken and not as zero.
//!
//! ## Never "nothing" when it means "couldn't ask"
//!
//! Every source always comes back, carrying a `state`. `signed_out`,
//! `unreachable` and `rejected` are distinct because they need different
//! things from you, and because a blank row is indistinguishable from "you
//! have no usage" — which would be a lie in all three cases. The frontend
//! keeps the last good reading for a source that has gone quiet and stamps it
//! with when it was read.
//!
//! ## One request per source per poll
//!
//! `/api/oauth/usage` rate limits. There is exactly one poller in the app
//! (`App.tsx`) calling [`usage_report`] once a minute, and everything that
//! shows usage renders from its result. Do not add a second one: two pollers
//! both double the request rate and let the two views disagree, one showing
//! bars while the other, refused, shows nothing.
//!
//! ## curl, and `#[tauri::command(async)]`
//!
//! `curl` via `std::process::Command` because the project deliberately pulls in
//! no HTTP/TLS crates. `(async)` is load-bearing: a plain `#[tauri::command]`
//! body runs on the main thread, and with no network this curl froze the whole
//! window — every minute, for as long as the connect took — and the freeze that
//! landed on startup read as aiterm hanging on open. Off the main thread, an
//! offline machine just keeps the cached reading.

use serde::Serialize;

/// One usage limit, as a bar: a percentage against a window that resets.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct UsageBar {
    /// Stable-ish key for React lists and for picking a bar out by meaning:
    /// "session" | "weekly_all" | "weekly_scoped" from Anthropic, or
    /// "codex_primary" | "codex_secondary", or "grok_period", or
    /// Antigravity's "weekly_gemini" | "five_hour_gemini" |
    /// "weekly_claude_gpt" | "five_hour_claude_gpt".
    pub kind: String,
    /// Human label: "Current session", "All models", "Fable", "Weekly limit".
    pub label: String,
    /// 0–100 percent used.
    pub percent: f64,
    /// "normal" | "warning" | "critical" — drives the bar colour.
    pub severity: String,
    /// ISO-8601 reset timestamp, or "" when the source did not give one.
    pub resets_at: String,
}

/// A quantity of money or credits, as opposed to a percentage of a window.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct UsageAmount {
    /// "Extra usage", "Credits".
    pub label: String,
    pub amount: f64,
    /// The total this counts against, when the source names one.
    pub of: Option<f64>,
    /// ISO currency code, when the amount is known to be money in one.
    /// Anthropic's `spend` object carries `"currency":"USD"` itself;
    /// OpenRouter's `/credits` does not name one, but its credits are defined
    /// in dollars, so "USD" is set for that shape (see `parse_provider_credits`).
    /// Codex's `credits.balance` and Grok's `prepaidBalance` name nothing and
    /// are documented nowhere, so they stay empty and the UI prints a bare
    /// number rather than guessing a symbol.
    pub currency: String,
    /// "remaining" — `amount` is what is left of `of`.
    /// "used" — `amount` is what has been spent out of `of`.
    pub sense: String,
}

/// One service's answer. Always present, even when it could not be reached —
/// see the module note on why silence is not an acceptable way to say "no".
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct UsageSource {
    /// "anthropic" | "codex" | "grok" | "antigravity" | "provider:<provider id>".
    pub id: String,
    pub name: String,
    /// "ok" — the numbers below are real.
    /// "signed_out" — nothing is logged in here.
    /// "unreachable" — asked, got no answer.
    /// "rejected" — asked, was refused (expired token, bad key, rate limit).
    /// "no_balance" — reachable and authorised, but publishes no balance.
    pub state: String,
    /// One sentence for a non-"ok" state, saying what to do about it. Empty
    /// when the state is "ok".
    pub detail: String,
    /// Plan name when the service names one ("Plus"), else "".
    pub plan: String,
    /// Account identity when known (an email address), else "".
    pub account: String,
    pub bars: Vec<UsageBar>,
    pub amounts: Vec<UsageAmount>,
    /// Extra true facts worth a line each ("2 rate-limit reset credits
    /// available"). Not errors — those go in `detail`.
    pub notes: Vec<String>,
}

impl UsageSource {
    fn blank(id: &str, name: &str) -> Self {
        UsageSource {
            id: id.to_string(),
            name: name.to_string(),
            state: "ok".into(),
            detail: String::new(),
            plan: String::new(),
            account: String::new(),
            bars: vec![],
            amounts: vec![],
            notes: vec![],
        }
    }

    fn failed(id: &str, name: &str, state: &str, detail: &str) -> Self {
        let mut s = Self::blank(id, name);
        s.state = state.into();
        s.detail = detail.into();
        s
    }
}

// ---------------------------------------------------------------------------
// plumbing
// ---------------------------------------------------------------------------

/// GET a URL and hand back the HTTP status alongside the body.
///
/// `Err` means the request never reached the service (no curl, DNS, TLS,
/// timeout) — that is "unreachable". A 4xx comes back as `Ok`, because the
/// service did answer and the status is the interesting part.
///
/// The bearer token goes in on stdin, never on the argv. `/proc/<pid>/cmdline`
/// is world-readable, so `-H "Authorization: Bearer …"` hands the credential
/// to any process on the machine that looks — and this runs every minute, so
/// there is nearly always a curl to look at. `--config -` takes the same
/// header from a config file read from stdin, which nothing else can see. The
/// escaping lives in `providers.rs` so there is one copy of it.
fn curl_get(url: &str, bearer: &str, headers: &[String]) -> Result<(u16, String), String> {
    use std::io::Write;
    let mut args: Vec<String> = vec![
        "-sS".into(),
        // Give up on an unreachable host quickly rather than sitting on the
        // full budget; the reply itself still gets the longer window.
        "--connect-timeout".into(),
        "3".into(),
        "--max-time".into(),
        "10".into(),
        // Status on its own line after the body, so an HTTP error can be
        // reported as one instead of as "could not parse the reply".
        "-w".into(),
        "\n%{http_code}".into(),
        "--config".into(),
        "-".into(),
    ];
    for h in headers {
        args.push("-H".into());
        args.push(h.clone());
    }
    args.push(url.into());
    let mut child = std::process::Command::new("curl")
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("could not write to curl")?
        .write_all(crate::providers::curl_auth_config(bearer).as_bytes())
        .map_err(|e| format!("could not write to curl: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "curl failed".to_string()
        } else {
            err
        });
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (code, body) = split_status(&text);
    Ok((code, body.to_string()))
}

/// Split the trailing `-w "\n%{http_code}"` line off a curl response.
///
/// Bodies contain newlines, so this splits from the right. A response with no
/// newline at all is all status and no body — which is what a body-less error
/// looks like.
fn split_status(response: &str) -> (u16, &str) {
    match response.rsplit_once('\n') {
        Some((body, status)) => (status.trim().parse().unwrap_or(0), body),
        None => (response.trim().parse().unwrap_or(0), ""),
    }
}

/// Seconds since the epoch → `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Codex reports resets as unix timestamps while Anthropic reports them as
/// ISO-8601 strings, and the frontend should not have to care which source a
/// bar came from. Converting here rather than adding a second timestamp field
/// keeps `UsageBar` one shape.
///
/// Hand-rolled because the project has no date crate and one is not worth
/// pulling in for this. The civil-from-days step is Howard Hinnant's algorithm;
/// the tests pin it against `date -u -d @N`, including both flavours of leap
/// year and a time before the epoch.
fn iso_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01 so leap day lands at the end of the year
    // and the month arithmetic below needs no special cases.
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March = 0
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Colour band for a source that reports a percentage but no severity.
///
/// Anthropic sends its own `severity` and that is used verbatim — these
/// thresholds are ours, applied only to Codex, which sends a bare
/// `used_percent`. They are a display choice, not a claim about where OpenAI
/// throttles you.
fn severity_for(percent: f64) -> &'static str {
    if percent >= 90.0 {
        "critical"
    } else if percent >= 75.0 {
        "warning"
    } else {
        "normal"
    }
}

/// "5-hour limit", "Weekly limit", "30-day limit" from a window length.
///
/// Codex names its windows only by length in seconds, so the label has to be
/// derived. Exact multiples get the clean wording; anything else falls back to
/// the largest unit that divides it, so an unfamiliar window is still described
/// truthfully rather than rounded into a lie.
fn window_label(seconds: i64) -> String {
    match seconds {
        604_800 => "Weekly limit".to_string(),
        86_400 => "Daily limit".to_string(),
        s if s > 0 && s % 86_400 == 0 => format!("{}-day limit", s / 86_400),
        s if s > 0 && s % 3_600 == 0 => format!("{}-hour limit", s / 3_600),
        s if s > 0 && s % 60 == 0 => format!("{}-minute limit", s / 60),
        _ => "Limit".to_string(),
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Claude
// ---------------------------------------------------------------------------

fn claude_oauth_token() -> Option<String> {
    let path = dirs::home_dir()?.join(".claude/.credentials.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("claudeAiOauth")?
        .get("accessToken")?
        .as_str()
        .map(|s| s.to_string())
}

/// The signed-in email, for telling two accounts apart at a glance.
///
/// `/api/oauth/usage` does not include one, but Claude Code caches the profile
/// it fetched in `~/.claude.json` under `oauthAccount.emailAddress`. Read
/// best-effort: an absent or reshaped file costs a label, not the reading.
///
/// The same object also carries `organizationType` and
/// `organizationRateLimitTier`, which look like they name the plan. They are
/// not used: the only value this was ever seen holding is one machine's, there
/// is no documented set to map, and a plan badge that is wrong is worse than
/// no plan badge.
fn claude_account_email() -> Option<String> {
    let path = dirs::home_dir()?.join(".claude.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(|s| s.to_string())
}

fn label_for(kind: &str, scope_model: Option<&str>) -> String {
    match kind {
        "session" => "Current session".to_string(),
        "weekly_all" => "All models".to_string(),
        _ => scope_model
            .map(|m| m.to_string())
            .unwrap_or_else(|| "Weekly".to_string()),
    }
}

/// `{"amount_minor":20000,"currency":"USD","exponent":2}` → `(200.0, "USD")`.
fn money(v: Option<&serde_json::Value>) -> Option<(f64, String)> {
    let v = v?;
    let minor = v.get("amount_minor")?.as_f64()?;
    let exponent = v.get("exponent").and_then(|e| e.as_i64()).unwrap_or(2);
    let currency = v
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    Some((minor / 10f64.powi(exponent as i32), currency))
}

/// Turn the `/api/oauth/usage` body into a source.
///
/// Split from the request so the shape — which is the part that changes under
/// us — is testable without a network.
pub fn parse_anthropic(status: u16, body: &str) -> UsageSource {
    let mut src = UsageSource::blank("anthropic", "Claude");
    if status == 401 || status == 403 {
        src.state = "rejected".into();
        src.detail =
            "Claude Code's saved login was refused. Run `claude` and sign in again.".into();
        return src;
    }
    if status == 429 {
        src.state = "limited".into();
        src.detail = "Anthropic rate-limited the usage request. It will retry in a minute.".into();
        return src;
    }
    if !(200..300).contains(&status) {
        src.state = "unreachable".into();
        src.detail = format!("api.anthropic.com answered HTTP {status}.");
        return src;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        src.state = "unreachable".into();
        src.detail = "api.anthropic.com did not answer with JSON.".into();
        return src;
    };

    if let Some(limits) = v.get("limits").and_then(|l| l.as_array()) {
        src.bars = limits
            .iter()
            .filter_map(|item| {
                let kind = item.get("kind")?.as_str()?.to_string();
                let percent = item.get("percent")?.as_f64()?;
                let severity = item
                    .get("severity")
                    .and_then(|s| s.as_str())
                    .unwrap_or("normal")
                    .to_string();
                let resets_at = item
                    .get("resets_at")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_string();
                let scope_model = item
                    .get("scope")
                    .and_then(|s| s.get("model"))
                    .and_then(|m| m.get("display_name"))
                    .and_then(|d| d.as_str());
                let label = label_for(&kind, scope_model);
                Some(UsageBar {
                    kind,
                    label,
                    percent,
                    severity,
                    resets_at,
                })
            })
            .collect();
    }

    // Extra usage: pay-as-you-go credit that covers you past the plan limits.
    // Amounts arrive as minor units with an explicit exponent, so a currency
    // with a different number of decimals still reads correctly. Only shown
    // when `enabled` — an account that has never turned it on still has a
    // `limit` in the payload, and printing one would promise a budget that
    // does not exist.
    if let Some(spend) = v.get("spend") {
        let enabled = spend
            .get("enabled")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);
        if enabled {
            if let Some((amount, currency)) = money(spend.get("used")) {
                src.amounts.push(UsageAmount {
                    label: "Extra usage".into(),
                    amount,
                    of: money(spend.get("limit")).map(|(a, _)| a),
                    currency,
                    sense: "used".into(),
                });
            }
        } else if let Some(reason) = spend
            .get("disabled_reason")
            .and_then(|d| d.as_str())
            .filter(|d| !d.is_empty())
        {
            src.notes.push(reason.to_string());
        }
    }

    if src.bars.is_empty() && src.amounts.is_empty() {
        src.state = "unreachable".into();
        src.detail = "api.anthropic.com answered, but with no limits in it.".into();
    }
    src
}

fn anthropic_source() -> UsageSource {
    let Some(token) = claude_oauth_token() else {
        return UsageSource::failed(
            "anthropic",
            "Claude",
            "signed_out",
            "No Claude Code login found (~/.claude/.credentials.json). Run `claude` to sign in.",
        );
    };
    let headers = vec![
        "anthropic-beta: oauth-2025-04-20".to_string(),
        "Content-Type: application/json".to_string(),
    ];
    match curl_get(
        "https://api.anthropic.com/api/oauth/usage",
        &token,
        &headers,
    ) {
        Err(e) => UsageSource::failed(
            "anthropic",
            "Claude",
            "unreachable",
            &format!("Could not reach api.anthropic.com — {e}"),
        ),
        Ok((status, body)) => {
            let mut src = parse_anthropic(status, &body);
            if let Some(email) = claude_account_email() {
                src.account = email;
            }
            src
        }
    }
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

/// `$CODEX_HOME`, or `~/.codex`. Codex honours the override — it is the
/// `CODEX_HOME` line in `codex doctor --json` — so respecting it here keeps the
/// panel pointed at whatever login codex itself would use.
fn codex_home() -> Option<std::path::PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(p) if !p.is_empty() => Some(std::path::PathBuf::from(p)),
        _ => Some(dirs::home_dir()?.join(".codex")),
    }
}

struct CodexAuth {
    access_token: String,
    account_id: String,
}

/// What `~/.codex/auth.json` says. `Ok(None)` means codex is installed but
/// signed in with an API key instead of a ChatGPT account — a real state with
/// no plan usage behind it, and one worth saying out loud rather than
/// reporting as a failure.
fn codex_auth() -> Result<Option<CodexAuth>, String> {
    let path = codex_home().ok_or("no home directory")?.join("auth.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("{} is not readable", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "auth.json is not JSON".to_string())?;
    let token = v
        .get("tokens")
        .and_then(|t| t.get("access_token"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if token.is_empty() {
        return Ok(None);
    }
    let account_id = v
        .get("tokens")
        .and_then(|t| t.get("account_id"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    Ok(Some(CodexAuth {
        access_token: token.to_string(),
        account_id,
    }))
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Turn the `/wham/usage` body into a source.
pub fn parse_codex(status: u16, body: &str) -> UsageSource {
    let mut src = UsageSource::blank("codex", "Codex");
    if status == 401 || status == 403 {
        src.state = "rejected".into();
        src.detail = "Codex's saved login was refused. Run `codex login`.".into();
        return src;
    }
    if status == 429 {
        src.state = "limited".into();
        src.detail = "ChatGPT rate-limited the usage request. It will retry in a minute.".into();
        return src;
    }
    if !(200..300).contains(&status) {
        src.state = "unreachable".into();
        src.detail = format!("chatgpt.com answered HTTP {status}.");
        return src;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        src.state = "unreachable".into();
        src.detail = "chatgpt.com did not answer with JSON.".into();
        return src;
    };

    if let Some(plan) = v.get("plan_type").and_then(|p| p.as_str()) {
        src.plan = title_case(plan);
    }
    if let Some(email) = v.get("email").and_then(|e| e.as_str()) {
        src.account = email.to_string();
    }

    if let Some(rl) = v.get("rate_limit").filter(|r| !r.is_null()) {
        for (key, kind) in [
            ("primary_window", "codex_primary"),
            ("secondary_window", "codex_secondary"),
        ] {
            let Some(w) = rl.get(key).filter(|w| !w.is_null()) else {
                continue;
            };
            let Some(percent) = w.get("used_percent").and_then(|p| p.as_f64()) else {
                continue;
            };
            let seconds = w
                .get("limit_window_seconds")
                .and_then(|s| s.as_i64())
                .unwrap_or(0);
            // `reset_at` is an absolute unix time and `reset_after_seconds` a
            // relative one. Prefer the absolute: a reading kept from an hour
            // ago would otherwise go on claiming the same countdown.
            let resets_at = match w.get("reset_at").and_then(|r| r.as_i64()) {
                Some(t) if t > 0 => iso_from_unix(t),
                _ => match w.get("reset_after_seconds").and_then(|r| r.as_i64()) {
                    Some(s) if s > 0 => iso_from_unix(now_unix() + s),
                    _ => String::new(),
                },
            };
            src.bars.push(UsageBar {
                kind: kind.to_string(),
                label: window_label(seconds),
                percent,
                severity: severity_for(percent).to_string(),
                resets_at,
            });
        }
        if rl.get("limit_reached").and_then(|l| l.as_bool()) == Some(true) {
            src.notes.push("Rate limit reached.".into());
        }
    }

    // Codex credits are what you fall back on past the plan limit. The payload
    // sends the balance as a *string* and names no currency, so it is parsed as
    // a number and printed as a bare one — see `UsageAmount::currency`.
    if let Some(c) = v.get("credits").filter(|c| !c.is_null()) {
        if c.get("unlimited").and_then(|u| u.as_bool()) == Some(true) {
            src.notes.push("Credits: unlimited.".into());
        } else if let Some(balance) = c.get("balance").and_then(|b| {
            b.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| b.as_f64())
        }) {
            src.amounts.push(UsageAmount {
                label: "Credits".into(),
                amount: balance,
                of: None,
                currency: String::new(),
                sense: "remaining".into(),
            });
        }
    }

    // The "reset my limit early" credits the codex TUI offers when you hit a
    // wall. `applicable_available_count` is how many apply right now, which is
    // 0 unless you are actually limited — so the total is the useful number.
    if let Some(n) = v
        .get("rate_limit_reset_credits")
        .and_then(|r| r.get("available_count"))
        .and_then(|n| n.as_i64())
        .filter(|n| *n > 0)
    {
        src.notes.push(format!(
            "{n} rate-limit reset credit{} available.",
            if n == 1 { "" } else { "s" }
        ));
    }

    if src.bars.is_empty() && src.amounts.is_empty() {
        src.state = "unreachable".into();
        src.detail = "chatgpt.com answered, but with no usage in it.".into();
    }
    src
}

/// `None` when codex is not installed on this machine — an absent tool gets no
/// row, because a permanently empty "Codex — signed out" line is noise, not
/// information.
fn codex_source() -> Option<UsageSource> {
    let home = codex_home()?;
    if !home.is_dir() {
        return None;
    }
    let auth = match codex_auth() {
        Err(_) => {
            return Some(UsageSource::failed(
                "codex",
                "Codex",
                "signed_out",
                "No Codex login found. Run `codex login`.",
            ))
        }
        Ok(None) => {
            return Some(UsageSource::failed(
                "codex",
                "Codex",
                "signed_out",
                "Codex is signed in with an API key, which has no plan usage to report.",
            ))
        }
        Ok(Some(a)) => a,
    };
    let mut headers = vec!["Content-Type: application/json".to_string()];
    if !auth.account_id.is_empty() {
        headers.push(format!("chatgpt-account-id: {}", auth.account_id));
    }
    Some(
        match curl_get(
            "https://chatgpt.com/backend-api/wham/usage",
            &auth.access_token,
            &headers,
        ) {
            Err(e) => UsageSource::failed(
                "codex",
                "Codex",
                "unreachable",
                &format!("Could not reach chatgpt.com — {e}"),
            ),
            Ok((status, body)) => parse_codex(status, &body),
        },
    )
}

// ---------------------------------------------------------------------------
// Grok
// ---------------------------------------------------------------------------

/// The one billing call the grok CLI makes — see the module note on how it
/// was found.
const GROK_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";

/// `~/.grok`, the same directory `grok.rs` reads sessions and models from.
fn grok_home() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".grok"))
}

struct GrokAuth {
    access_token: String,
    email: String,
}

/// What `~/.grok/auth.json` says.
///
/// The file is a map keyed by `"<issuer>::<client id>"`, one entry per way
/// grok has been signed in, each with an `auth_mode`. The `"oidc"` entry is
/// the grok.com account whose plan the billing endpoint reports on; an entry
/// for an API key would carry no plan. `Ok(None)` means the file is there but
/// holds no account login — grok is installed and signed in some other way.
fn grok_auth() -> Result<Option<GrokAuth>, String> {
    let path = grok_home().ok_or("no home directory")?.join("auth.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("{} is not readable", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "auth.json is not JSON".to_string())?;
    let Some(entries) = v.as_object() else {
        return Err("auth.json is not an object".into());
    };
    let account = entries
        .values()
        .find(|e| e.get("auth_mode").and_then(|m| m.as_str()) == Some("oidc"));
    let Some(account) = account else {
        return Ok(None);
    };
    let token = account.get("key").and_then(|k| k.as_str()).unwrap_or("");
    if token.is_empty() {
        return Ok(None);
    }
    Ok(Some(GrokAuth {
        access_token: token.to_string(),
        email: account
            .get("email")
            .and_then(|e| e.as_str())
            .unwrap_or("")
            .to_string(),
    }))
}

/// "USAGE_PERIOD_TYPE_WEEKLY" → "Weekly limit". Anything unrecognised keeps
/// its own last word rather than being called weekly on a guess.
fn grok_period_label(kind: &str) -> String {
    match kind {
        "USAGE_PERIOD_TYPE_WEEKLY" => "Weekly limit".to_string(),
        "USAGE_PERIOD_TYPE_DAILY" => "Daily limit".to_string(),
        "USAGE_PERIOD_TYPE_MONTHLY" => "Monthly limit".to_string(),
        other => match other.rsplit('_').next().filter(|w| !w.is_empty()) {
            Some(w) => format!("{} limit", title_case(&w.to_lowercase())),
            None => "Limit".to_string(),
        },
    }
}

/// "GrokBuild" → "Grok Build", "GrokAppBuilder" → "App Builder". The product
/// names arrive as one CamelCase word each with a "Grok" prefix that would
/// read four times over on one line; only Grok Build keeps it, because that is
/// the CLI's own name and "Build" alone is not one.
fn grok_product_label(product: &str) -> String {
    if product == "GrokBuild" {
        return "Grok Build".to_string();
    }
    let mut words: Vec<String> = vec![];
    for ch in product.chars() {
        if ch.is_uppercase() || words.is_empty() {
            words.push(ch.to_string());
        } else if let Some(last) = words.last_mut() {
            last.push(ch);
        }
    }
    if words.len() > 1 && words[0] == "Grok" {
        words.remove(0);
    }
    words.join(" ")
}

/// Turn the `/billing?format=credits` body into a source.
pub fn parse_grok(status: u16, body: &str) -> UsageSource {
    let mut src = UsageSource::blank("grok", "Grok");
    if status == 401 || status == 403 {
        src.state = "rejected".into();
        src.detail = "Grok's saved login was refused or has expired. Open grok once — it refreshes its own token — or run `grok login`.".into();
        return src;
    }
    if status == 429 {
        src.state = "limited".into();
        src.detail = "grok.com rate-limited the usage request. It will retry in a minute.".into();
        return src;
    }
    if !(200..300).contains(&status) {
        src.state = "unreachable".into();
        src.detail = format!("grok.com answered HTTP {status}.");
        return src;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        src.state = "unreachable".into();
        src.detail = "grok.com did not answer with JSON.".into();
        return src;
    };
    let Some(config) = v.get("config").filter(|c| c.is_object()) else {
        src.state = "unreachable".into();
        src.detail = "grok.com answered, but with no usage in it.".into();
        return src;
    };

    if let Some(percent) = config.get("creditUsagePercent").and_then(|p| p.as_f64()) {
        let period = config.get("currentPeriod");
        let kind = period
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        // The reply's `end` is already ISO-8601 (with a `+00:00` offset rather
        // than `Z`, which `Date` parses the same), so it goes through as is.
        let resets_at = period
            .and_then(|p| p.get("end"))
            .and_then(|e| e.as_str())
            .unwrap_or("")
            .to_string();
        src.bars.push(UsageBar {
            kind: "grok_period".into(),
            label: grok_period_label(kind),
            percent,
            severity: severity_for(percent).to_string(),
            resets_at,
        });
    }

    // Where the window went. Products with no `usagePercent` (Chat, on this
    // account) are left out rather than printed as 0%, since the field's
    // absence reads more like "not metered here" than "unused".
    if let Some(products) = config.get("productUsage").and_then(|p| p.as_array()) {
        let parts: Vec<String> = products
            .iter()
            .filter_map(|p| {
                let name = p.get("product").and_then(|n| n.as_str())?;
                let pct = p.get("usagePercent").and_then(|u| u.as_f64())?;
                Some(format!(
                    "{} {}%",
                    grok_product_label(name),
                    pct.round() as i64
                ))
            })
            .collect();
        if !parts.is_empty() {
            src.notes.push(parts.join(" · "));
        }
    }

    // `{"val":N}` wrappers. Neither names a currency, so neither gets one.
    let val = |key: &str| {
        config
            .get(key)
            .and_then(|o| o.get("val"))
            .and_then(|n| n.as_f64())
    };
    if let Some(cap) = val("onDemandCap").filter(|c| *c > 0.0) {
        src.amounts.push(UsageAmount {
            label: "On-demand".into(),
            amount: val("onDemandUsed").unwrap_or(0.0),
            of: Some(cap),
            currency: String::new(),
            sense: "used".into(),
        });
    }
    if let Some(balance) = val("prepaidBalance").filter(|b| *b > 0.0) {
        src.amounts.push(UsageAmount {
            label: "Prepaid balance".into(),
            amount: balance,
            of: None,
            currency: String::new(),
            sense: "remaining".into(),
        });
    }

    if src.bars.is_empty() && src.amounts.is_empty() {
        src.state = "unreachable".into();
        src.detail = "grok.com answered, but with no usage in it.".into();
    }
    src
}

/// `None` when grok is not installed here — same rule as Codex: an absent
/// tool gets no row.
fn grok_source() -> Option<UsageSource> {
    let home = grok_home()?;
    if !home.is_dir() {
        return None;
    }
    let auth = match grok_auth() {
        Err(_) => {
            return Some(UsageSource::failed(
                "grok",
                "Grok",
                "signed_out",
                "No grok login found. Run `grok login`.",
            ))
        }
        Ok(None) => {
            return Some(UsageSource::failed(
                "grok",
                "Grok",
                "signed_out",
                "grok is signed in with an API key, which has no plan usage to report.",
            ))
        }
        Ok(Some(a)) => a,
    };
    let headers = vec!["Accept: application/json".to_string()];
    let mut src = match curl_get(GROK_BILLING_URL, &auth.access_token, &headers) {
        Err(e) => UsageSource::failed(
            "grok",
            "Grok",
            "unreachable",
            &format!("Could not reach grok.com — {e}"),
        ),
        Ok((status, body)) => parse_grok(status, &body),
    };
    // The billing reply says nothing about who it is for; auth.json does.
    if src.state == "ok" {
        src.account = auth.email;
    }
    Some(src)
}

// ---------------------------------------------------------------------------
// Antigravity
// ---------------------------------------------------------------------------

/// The four buckets `/usage` reports, by their ids. [observed: agy 1.1.24]
/// The weekly kinds start with `weekly` so the phone's headline-bar rule
/// picks them without an arm of its own.
fn antigravity_bar_kind(bucket_id: &str) -> Option<(&'static str, &'static str)> {
    match bucket_id {
        "gemini-weekly" => Some(("weekly_gemini", "Gemini weekly")),
        "gemini-5h" => Some(("five_hour_gemini", "Gemini 5-hour")),
        "3p-weekly" => Some(("weekly_claude_gpt", "Claude & GPT weekly")),
        "3p-5h" => Some(("five_hour_claude_gpt", "Claude & GPT 5-hour")),
        _ => None,
    }
}

/// Turn `agy -p /usage --output-format json` — its exit code and stdout —
/// into a source. `remaining_fraction` is what is *left*; a bar's `percent`
/// is what is used, as every other source expresses it, so it is inverted
/// here. A non-zero exit is agy refusing (signed out, no network to sign in
/// with) and reads as rejected with "open agy", the way grok's expired token
/// does; output that is not the JSON shape is unreachable.
pub fn parse_antigravity(exit: i32, body: &str) -> UsageSource {
    let mut src = UsageSource::blank("antigravity", "Antigravity");
    if exit != 0 {
        src.state = "rejected".into();
        src.detail =
            format!("agy could not answer /usage (exit {exit}). Open agy once and sign in.");
        return src;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        src.state = "unreachable".into();
        src.detail = "agy did not answer /usage with JSON.".into();
        return src;
    };
    if v.get("status").and_then(|s| s.as_str()) != Some("SUCCESS") {
        src.state = "rejected".into();
        let err = v
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("no reason given");
        src.detail = format!("agy refused /usage — {err}. Open agy once and sign in.");
        return src;
    }
    let Some(groups) = v.pointer("/command/data/groups").and_then(|g| g.as_array()) else {
        src.state = "unreachable".into();
        src.detail = "agy answered, but with no usage in it.".into();
        return src;
    };
    for group in groups {
        let group_name = group.get("name").and_then(|n| n.as_str()).unwrap_or("");
        for bucket in group
            .get("buckets")
            .and_then(|b| b.as_array())
            .into_iter()
            .flatten()
        {
            let Some(remaining) = bucket.get("remaining_fraction").and_then(|r| r.as_f64()) else {
                continue;
            };
            let id = bucket.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let (kind, label) = match antigravity_bar_kind(id) {
                Some((k, l)) => (k.to_string(), l.to_string()),
                None => (
                    format!(
                        "{}_{}",
                        bucket
                            .get("window")
                            .and_then(|w| w.as_str())
                            .unwrap_or("window"),
                        id
                    ),
                    format!(
                        "{group_name} {}",
                        bucket.get("name").and_then(|n| n.as_str()).unwrap_or(id)
                    ),
                ),
            };
            let percent = ((1.0 - remaining) * 100.0).clamp(0.0, 100.0);
            src.bars.push(UsageBar {
                kind,
                label,
                percent,
                severity: severity_for(percent).to_string(),
                resets_at: bucket
                    .get("reset_time")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    if src.bars.is_empty() {
        src.state = "unreachable".into();
        src.detail = "agy answered, but with no usage in it.".into();
    }
    src
}

/// `agy -p /credits --output-format json` → a G1 credit balance, only when
/// there is one: `{"remaining_credits":0,…}` is the ordinary state of an
/// account not buying credits, and a zero row would read as "out".
/// [observed: agy 1.1.24]
pub fn parse_antigravity_credits(body: &str) -> Option<UsageAmount> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let credits = v.pointer("/command/data/remaining_credits")?.as_f64()?;
    (credits > 0.0).then(|| UsageAmount {
        label: "G1 credits".into(),
        amount: credits,
        of: None,
        // agy names no unit for these.
        currency: String::new(),
        sense: "remaining".into(),
    })
}

/// The signed-in account, out of the CLI's own log — the only place it is
/// written (`applyAuthResult: email=…, authMethod=consumer`); no config or
/// auth file under `~/.gemini` carries it, and the token in the keyring is
/// not looked at. `cli.log` at the store root is a symlink to the newest log;
/// the `log/` directory is walked newest-first when it is not there.
/// [observed: agy 1.1.24]
fn antigravity_account() -> Option<String> {
    let root = crate::antigravity::store_root()?;
    let mut candidates = vec![root.join("cli.log")];
    if let Ok(dir) = std::fs::read_dir(root.join("log")) {
        let mut logs: Vec<std::path::PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "log"))
            .collect();
        logs.sort();
        candidates.extend(logs.into_iter().rev().take(5));
    }
    for path in candidates {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > 4 * 1024 * 1024 {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(email) = antigravity_email_in(&text) {
            return Some(email);
        }
    }
    None
}

fn antigravity_email_in(log: &str) -> Option<String> {
    let i = log.find("applyAuthResult: email=")? + "applyAuthResult: email=".len();
    let rest = &log[i..];
    let end = rest
        .find(|c: char| c == ',' || c.is_whitespace())
        .unwrap_or(rest.len());
    let email = rest[..end].trim();
    (!email.is_empty()).then(|| email.to_string())
}

static ANTIGRAVITY_CACHE: std::sync::Mutex<Option<(std::time::Instant, UsageSource)>> =
    std::sync::Mutex::new(None);

/// `None` when agy is not installed here — same rule as Codex and Grok: an
/// absent tool gets no row. The answer is held for five minutes, and the
/// lock is held across the spawn so two pollers (the desktop strip and the
/// phone) cannot start two agy processes: the second waits and gets the
/// first's answer.
fn antigravity_source() -> Option<UsageSource> {
    let bin = crate::antigravity::agy_bin()?;
    let mut cache = ANTIGRAVITY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, src)) = cache.as_ref() {
        if at.elapsed().as_secs() < 300 {
            return Some(src.clone());
        }
    }
    let run = |slash: &str| {
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(["-p", slash, "--output-format", "json"]);
        if let Some(home) = dirs::home_dir() {
            cmd.current_dir(home);
        }
        crate::antigravity::run_capped(cmd, std::time::Duration::from_secs(8))
    };
    let mut src = match run("/usage") {
        Err(e) => UsageSource::failed(
            "antigravity",
            "Antigravity",
            "unreachable",
            &format!("agy did not answer /usage — {e}"),
        ),
        Ok((code, out)) => parse_antigravity(code, &out),
    };
    if src.state == "ok" {
        if let Ok((0, out)) = run("/credits") {
            src.amounts.extend(parse_antigravity_credits(&out));
        }
        src.account = antigravity_account().unwrap_or_default();
    }
    *cache = Some((std::time::Instant::now(), src.clone()));
    Some(src)
}

// ---------------------------------------------------------------------------
// Configured API providers
// ---------------------------------------------------------------------------

/// The provider store is `providers.rs`'s — it owns
/// `~/.config/aiterm/providers.json`, writes it, and has the tests for it.
/// `load_providers` is the one read this module makes of it.
fn configured_providers() -> Vec<crate::providers::Provider> {
    crate::providers::load_providers()
}

/// Turn a `GET {base}/credits` reply into a source.
///
/// The only shape verified against a live service is OpenRouter's
/// `{"data":{"total_credits":551.5936,"total_usage":550.667155136}}`, curled
/// with a real key. Both numbers are needed: `total_credits` is everything ever
/// added to the account, not what is left, so a panel showing it alone would
/// report $551 of headroom to someone with 93 cents.
///
/// Anything else — a 404, an HTML error page, JSON without those two keys — is
/// `no_balance`. Not an error: most OpenAI-compatible endpoints simply do not
/// publish one, and calling that a failure would put a red row next to a
/// perfectly healthy provider.
///
/// OpenRouter also serves `/auth/key`, which carries this key's own spend cap
/// (`limit`, `limit_remaining`) and rolling usage. It is not fetched: it is a
/// second request per provider per poll for a cap that is `null` unless you set
/// one, and the account balance is what "how much have I got left" means.
pub fn parse_provider_credits(id: &str, name: &str, status: u16, body: &str) -> UsageSource {
    let source_id = format!("provider:{id}");
    let mut src = UsageSource::blank(&source_id, name);
    if status == 401 || status == 403 {
        src.state = "rejected".into();
        src.detail = format!("{name} rejected the saved API key.");
        return src;
    }
    if !(200..300).contains(&status) {
        src.state = "no_balance".into();
        src.detail = format!("{name} publishes no credit balance.");
        return src;
    }
    let totals = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            let d = v.get("data")?;
            let credits = d.get("total_credits")?.as_f64()?;
            let used = d.get("total_usage")?.as_f64()?;
            Some((credits, used))
        });
    match totals {
        Some((credits, used)) => {
            src.amounts.push(UsageAmount {
                label: "Credits".into(),
                amount: credits - used,
                // No `of`, deliberately. `total_credits` is everything ever
                // added to the account, not a budget, so a bar drawn against
                // it would tell someone who has topped up ten times that they
                // are 99% of the way through their credits. The balance is the
                // number; the lifetime spend goes in a note where it cannot be
                // mistaken for a limit.
                of: None,
                // The reply names no currency, but this shape is OpenRouter's
                // and OpenRouter's credits are dollars — its docs define
                // `total_credits` and `total_usage` in USD and the dashboard
                // prints them with a `$`. A balance read "0.93" was being taken
                // for a count of something, so the unit is stated.
                currency: "USD".into(),
                sense: "remaining".into(),
            });
            src.notes
                .push(format!("{used:.2} spent of {credits:.2} ever added."));
            src
        }
        None => {
            src.state = "no_balance".into();
            src.detail = format!("{name} publishes no credit balance.");
            src
        }
    }
}

fn provider_source(p: &crate::providers::Provider) -> UsageSource {
    let id = format!("provider:{}", p.id);
    if p.api_key.is_empty() {
        return UsageSource::failed(
            &id,
            &p.name,
            "signed_out",
            "No API key saved for this provider.",
        );
    }
    let url = format!("{}/credits", p.base_url.trim_end_matches('/'));
    let headers = vec!["Content-Type: application/json".to_string()];
    match curl_get(&url, &p.api_key, &headers) {
        Err(e) => UsageSource::failed(
            &id,
            &p.name,
            "unreachable",
            &format!("Could not reach {} — {e}", p.base_url),
        ),
        Ok((status, body)) => parse_provider_credits(&p.id, &p.name, status, &body),
    }
}

// ---------------------------------------------------------------------------
// the command
// ---------------------------------------------------------------------------

/// Every source, read in one pass.
///
/// The sources are fetched on their own threads and joined, so the call costs
/// the slowest one rather than their sum. Sequentially, a laptop with three
/// providers configured and a flaky link would spend up to fifty seconds
/// finishing a poll that runs every sixty, and the reading would never settle.
/// Nothing is shared between the threads, so the concurrency is a `scope` and a
/// join, with no locking.
#[tauri::command(async)]
pub fn usage_report() -> Vec<UsageSource> {
    let providers = configured_providers();
    std::thread::scope(|s| {
        let claude = s.spawn(anthropic_source);
        let codex = s.spawn(codex_source);
        let grok = s.spawn(grok_source);
        let antigravity = s.spawn(antigravity_source);
        let provider_jobs: Vec<_> = providers
            .iter()
            .map(|p| s.spawn(move || provider_source(p)))
            .collect();

        let mut out = vec![];
        // A panicking source must not take the others down with it: the panel
        // showing two of three services beats it showing none.
        if let Ok(c) = claude.join() {
            out.push(c);
        }
        if let Ok(Some(c)) = codex.join() {
            out.push(c);
        }
        if let Ok(Some(g)) = grok.join() {
            out.push(g);
        }
        if let Ok(Some(a)) = antigravity.join() {
            out.push(a);
        }
        for j in provider_jobs {
            if let Ok(p) = j.join() {
                out.push(p);
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned against `date -u -d @N +%Y-%m-%dT%H:%M:%SZ`, including both kinds
    /// of leap year: 2000 is one (divisible by 400) and 2024 is the ordinary
    /// kind.
    #[test]
    fn unix_times_become_iso_strings() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_from_unix(1_709_164_800), "2024-02-29T00:00:00Z");
        // The reset_at that came back from /wham/usage while this was written.
        assert_eq!(iso_from_unix(1_785_764_869), "2026-08-03T13:47:49Z");
        // Before the epoch, so the floor-division is right rather than merely
        // untested.
        assert_eq!(iso_from_unix(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn the_status_line_splits_off_the_back() {
        assert_eq!(split_status("{\"a\":1}\n200"), (200, "{\"a\":1}"));
        // Bodies contain newlines; only the last line is the status.
        assert_eq!(split_status("line1\nline2\n404"), (404, "line1\nline2"));
        // No body at all.
        assert_eq!(split_status("204"), (204, ""));
    }

    #[test]
    fn windows_are_named_from_their_length() {
        assert_eq!(window_label(604_800), "Weekly limit");
        assert_eq!(window_label(86_400), "Daily limit");
        assert_eq!(window_label(18_000), "5-hour limit");
        assert_eq!(window_label(2_592_000), "30-day limit");
        assert_eq!(window_label(0), "Limit");
    }

    /// The real `/api/oauth/usage` body, trimmed to the parts that are read.
    const ANTHROPIC_BODY: &str = r#"{
      "five_hour":{"utilization":6.0},
      "limits":[
        {"kind":"session","group":"session","percent":6,"severity":"normal",
         "resets_at":"2026-07-28T03:49:59.328892+00:00","scope":null,"is_active":false},
        {"kind":"weekly_all","group":"weekly","percent":10,"severity":"normal",
         "resets_at":"2026-07-30T19:00:00.328914+00:00","scope":null,"is_active":true},
        {"kind":"weekly_scoped","group":"weekly","percent":7,"severity":"warning",
         "resets_at":"2026-07-30T19:00:00.329219+00:00",
         "scope":{"model":{"id":null,"display_name":"Fable"},"surface":null},"is_active":false}
      ],
      "spend":{"used":{"amount_minor":1234,"currency":"USD","exponent":2},
               "limit":{"amount_minor":20000,"currency":"USD","exponent":2},
               "percent":6,"severity":"normal","enabled":true,"disabled_reason":null}
    }"#;

    #[test]
    fn anthropic_limits_become_bars() {
        let src = parse_anthropic(200, ANTHROPIC_BODY);
        assert_eq!(src.state, "ok", "{}", src.detail);
        assert_eq!(src.bars.len(), 3);
        assert_eq!(src.bars[0].label, "Current session");
        assert_eq!(src.bars[1].label, "All models");
        // A scoped weekly limit is named after the model it scopes to.
        assert_eq!(src.bars[2].label, "Fable");
        // Severity comes from the API, never from our own thresholds.
        assert_eq!(src.bars[2].severity, "warning");
    }

    #[test]
    fn anthropic_extra_usage_is_money_not_a_percentage() {
        let src = parse_anthropic(200, ANTHROPIC_BODY);
        assert_eq!(src.amounts.len(), 1);
        let a = &src.amounts[0];
        assert_eq!(a.label, "Extra usage");
        // Minor units with an exponent, not dollars.
        assert_eq!(a.amount, 12.34);
        assert_eq!(a.of, Some(200.0));
        assert_eq!(a.currency, "USD");
        assert_eq!(a.sense, "used");
    }

    /// An account that never enabled extra usage still has a `limit` in the
    /// payload. Showing it would promise a budget that does not exist.
    #[test]
    fn extra_usage_is_hidden_when_it_is_switched_off() {
        let body = r#"{"limits":[{"kind":"session","percent":1,"severity":"normal"}],
          "spend":{"used":{"amount_minor":0,"currency":"USD","exponent":2},
                   "limit":{"amount_minor":20000,"currency":"USD","exponent":2},
                   "enabled":false}}"#;
        let src = parse_anthropic(200, body);
        assert!(
            src.amounts.is_empty(),
            "showed a budget that is switched off"
        );
    }

    /// Each failure needs its own sentence: "it didn't work" sends you to the
    /// wrong fix, and an empty source reads as "you have no usage".
    #[test]
    fn anthropic_failures_are_told_apart() {
        let rejected = parse_anthropic(401, "{}");
        assert_eq!(rejected.state, "rejected");
        assert!(rejected.detail.contains("sign in"), "{}", rejected.detail);

        let limited = parse_anthropic(429, "{}");
        assert_eq!(limited.state, "limited");
        assert!(
            limited.detail.contains("rate-limited"),
            "{}",
            limited.detail
        );

        let broken = parse_anthropic(200, "<html>nope</html>");
        assert_eq!(broken.state, "unreachable");

        // 200 with nothing usable in it is not "0% used".
        let empty = parse_anthropic(200, "{}");
        assert_eq!(empty.state, "unreachable");
        assert!(empty.bars.is_empty());
    }

    /// The real `/wham/usage` body, as returned by chatgpt.com.
    const CODEX_BODY: &str = r#"{
      "user_id":"user-x","account_id":"user-x","email":"someone@example.com",
      "plan_type":"plus",
      "rate_limit":{"allowed":true,"limit_reached":false,
        "primary_window":{"used_percent":1,"limit_window_seconds":604800,
                          "reset_after_seconds":563359,"reset_at":1785764869},
        "secondary_window":null},
      "code_review_rate_limit":null,"additional_rate_limits":null,
      "credits":{"has_credits":false,"unlimited":false,"overage_limit_reached":false,
                 "balance":"0","approx_local_messages":[0,0],"approx_cloud_messages":[0,0]},
      "spend_control":{"reached":false,"individual_limit":null},
      "rate_limit_reached_type":null,"promo":null,
      "rate_limit_reset_credits":{"available_count":2,"applicable_available_count":0}
    }"#;

    #[test]
    fn codex_windows_become_bars() {
        let src = parse_codex(200, CODEX_BODY);
        assert_eq!(src.state, "ok", "{}", src.detail);
        assert_eq!(src.plan, "Plus");
        assert_eq!(src.account, "someone@example.com");
        assert_eq!(src.bars.len(), 1, "a null secondary window is not a bar");
        assert_eq!(src.bars[0].label, "Weekly limit");
        assert_eq!(src.bars[0].percent, 1.0);
        // Codex sends no severity of its own; ours is derived from the percent.
        assert_eq!(src.bars[0].severity, "normal");
        assert_eq!(src.bars[0].resets_at, "2026-08-03T13:47:49Z");
    }

    #[test]
    fn codex_credits_arrive_as_a_string() {
        let src = parse_codex(200, CODEX_BODY);
        assert_eq!(src.amounts.len(), 1);
        assert_eq!(src.amounts[0].amount, 0.0);
        // No currency in the payload, so none is printed.
        assert_eq!(src.amounts[0].currency, "");
        assert!(
            src.notes
                .iter()
                .any(|n| n.contains("2 rate-limit reset credits")),
            "{:?}",
            src.notes
        );
    }

    #[test]
    fn codex_secondary_windows_are_read_when_present() {
        let body = r#"{"plan_type":"pro","rate_limit":{"limit_reached":true,
          "primary_window":{"used_percent":92.5,"limit_window_seconds":18000,"reset_at":1785764869},
          "secondary_window":{"used_percent":80,"limit_window_seconds":604800,"reset_at":1785764869}}}"#;
        let src = parse_codex(200, body);
        assert_eq!(src.bars.len(), 2);
        assert_eq!(src.bars[0].label, "5-hour limit");
        assert_eq!(src.bars[0].severity, "critical");
        assert_eq!(src.bars[1].label, "Weekly limit");
        assert_eq!(src.bars[1].severity, "warning");
        assert!(src.notes.iter().any(|n| n.contains("Rate limit reached")));
    }

    #[test]
    fn codex_failures_are_told_apart() {
        let rejected = parse_codex(401, "{}");
        assert_eq!(rejected.state, "rejected");
        assert!(
            rejected.detail.contains("codex login"),
            "{}",
            rejected.detail
        );
        assert_eq!(parse_codex(500, "").state, "unreachable");
        // 200 with no usage in it is not "0% used".
        assert_eq!(parse_codex(200, "{}").state, "unreachable");
    }

    /// The live `/billing?format=credits` reply, curled with the token from
    /// `~/.grok/auth.json` while this was written.
    const GROK_BODY: &str = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-08-21T09:08:48.910873+00:00","end":"2026-08-28T09:08:48.910873+00:00"},"creditUsagePercent":22.0,"onDemandCap":{"val":0},"onDemandUsed":{"val":0},"productUsage":[{"product":"GrokBuild","usagePercent":19.0},{"product":"GrokImagine","usagePercent":2.0},{"product":"GrokAppBuilder","usagePercent":1.0},{"product":"GrokChat"}],"isUnifiedBillingUser":true,"prepaidBalance":{"val":0},"topUpMethod":"TOP_UP_METHOD_SAVED_PAYMENT_METHOD","billingPeriodStart":"2026-08-21T09:08:48.910873+00:00","billingPeriodEnd":"2026-08-28T09:08:48.910873+00:00"}}"#;

    #[test]
    fn grok_period_becomes_a_bar() {
        let src = parse_grok(200, GROK_BODY);
        assert_eq!(src.state, "ok", "{}", src.detail);
        assert_eq!(src.id, "grok");
        assert_eq!(src.bars.len(), 1);
        let b = &src.bars[0];
        assert_eq!(b.kind, "grok_period");
        assert_eq!(b.label, "Weekly limit");
        assert_eq!(b.percent, 22.0);
        assert_eq!(b.severity, "normal");
        assert_eq!(b.resets_at, "2026-08-28T09:08:48.910873+00:00");
        // Chat has no percent and is left out rather than shown as 0%.
        assert_eq!(
            src.notes,
            vec!["Grok Build 19% · Imagine 2% · App Builder 1%"]
        );
        // Zero balances and a zero cap are not worth a row.
        assert!(src.amounts.is_empty());
    }

    #[test]
    fn grok_balances_show_only_when_there_are_any() {
        let body = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_DAILY","end":"2026-09-01T00:00:00+00:00"},"creditUsagePercent":91.5,"onDemandCap":{"val":50},"onDemandUsed":{"val":12.5},"prepaidBalance":{"val":3}}}"#;
        let src = parse_grok(200, body);
        assert_eq!(src.bars[0].label, "Daily limit");
        assert_eq!(src.bars[0].severity, "critical");
        assert_eq!(src.amounts.len(), 2);
        assert_eq!(src.amounts[0].label, "On-demand");
        assert_eq!(src.amounts[0].amount, 12.5);
        assert_eq!(src.amounts[0].of, Some(50.0));
        assert_eq!(src.amounts[0].sense, "used");
        assert_eq!(src.amounts[1].label, "Prepaid balance");
        assert_eq!(src.amounts[1].amount, 3.0);
        // Nothing in the payload names a currency, so none is claimed.
        assert_eq!(src.amounts[1].currency, "");
    }

    #[test]
    fn grok_labels_are_read_not_guessed() {
        assert_eq!(
            grok_period_label("USAGE_PERIOD_TYPE_MONTHLY"),
            "Monthly limit"
        );
        assert_eq!(
            grok_period_label("USAGE_PERIOD_TYPE_FORTNIGHTLY"),
            "Fortnightly limit"
        );
        assert_eq!(grok_period_label(""), "Limit");
        assert_eq!(grok_product_label("GrokBuild"), "Grok Build");
        assert_eq!(grok_product_label("GrokAppBuilder"), "App Builder");
        assert_eq!(grok_product_label("GrokImagine"), "Imagine");
        assert_eq!(grok_product_label("Voice"), "Voice");
    }

    #[test]
    fn grok_failures_are_told_apart() {
        let rejected = parse_grok(
            401,
            r#"{"error":"Invalid or expired credentials (auth_kind=bearer, upstream=Unauthenticated)"}"#,
        );
        assert_eq!(rejected.state, "rejected");
        assert!(rejected.detail.contains("Open grok"), "{}", rejected.detail);
        assert_eq!(parse_grok(502, "").state, "unreachable");
        assert_eq!(parse_grok(200, "{}").state, "unreachable");
        assert_eq!(parse_grok(200, r#"{"config":{}}"#).state, "unreachable");
    }

    /// `agy -p /usage --output-format json` 1.1.24, verbatim, captured
    /// 2026-09-02.
    const ANTIGRAVITY_BODY: &str = r#"{"conversation_id":"","status":"SUCCESS","response":"Gemini Models\tWeekly Limit Remaining\t98%\t2026-09-09T15:55:56Z\nGemini Models\tFive Hour Limit Remaining\t96%\t2026-09-02T20:55:56Z\nClaude and GPT models\tWeekly Limit Remaining\t100%\t2026-09-09T16:04:32Z\nClaude and GPT models\tFive Hour Limit Remaining\t100%\t2026-09-02T21:04:32Z\n","duration_seconds":0,"num_turns":0,"usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":0},"command":{"name":"usage","data":{"description":"Within each group, models share a weekly limit and a 5-hour limit. Quota is consumed proportionally to the cost of the tokens. Thus, limits will last longer with shorter tasks or using more cost-effective models. The 5-hour limit smooths out aggregate demand to fairly distribute global capacity across all users, while your weekly limit is tied directly to your individual tier.","groups":[{"name":"Gemini Models","description":"Models within this group: Gemini Flash, Gemini Pro","buckets":[{"id":"gemini-weekly","name":"Weekly Limit Remaining","description":"You have used some of your weekly limit, it will fully refresh in 6 days, 23 hours.","window":"weekly","remaining_fraction":0.9805102944374084,"reset_time":"2026-09-09T15:55:56Z"},{"id":"gemini-5h","name":"Five Hour Limit Remaining","description":"You have used some of your 5-hour limit, it will fully refresh in 4 hours, 51 minutes.","window":"5h","remaining_fraction":0.9574214220046997,"reset_time":"2026-09-02T20:55:56Z"}]},{"name":"Claude and GPT models","description":"Models within this group: Claude Opus, Claude Sonnet, GPT-OSS","buckets":[{"id":"3p-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":1,"reset_time":"2026-09-09T16:04:32Z"},{"id":"3p-5h","name":"Five Hour Limit Remaining","window":"5h","remaining_fraction":1,"reset_time":"2026-09-02T21:04:32Z"}]}]}}}"#;

    /// `agy -p /credits --output-format json` 1.1.24, verbatim.
    const ANTIGRAVITY_CREDITS: &str = r#"{"conversation_id":"","status":"SUCCESS","response":"Remaining credits\t0\nUpgrade\thttps://antigravity.google/g1-upgrade\n","duration_seconds":0,"num_turns":0,"usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":0},"command":{"name":"credits","data":{"remaining_credits":0,"upgrade_uri":"https://antigravity.google/g1-upgrade"}}}"#;

    #[test]
    fn antigravity_buckets_become_four_bars_of_used_percent() {
        let src = parse_antigravity(0, ANTIGRAVITY_BODY);
        assert_eq!(src.state, "ok", "{}", src.detail);
        assert_eq!(src.id, "antigravity");
        assert_eq!(src.name, "Antigravity");
        let kinds: Vec<&str> = src.bars.iter().map(|b| b.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "weekly_gemini",
                "five_hour_gemini",
                "weekly_claude_gpt",
                "five_hour_claude_gpt"
            ]
        );
        let labels: Vec<&str> = src.bars.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Gemini weekly",
                "Gemini 5-hour",
                "Claude & GPT weekly",
                "Claude & GPT 5-hour"
            ]
        );
        // remaining 0.9805 → 1.95% used, never 98%.
        assert!(
            (src.bars[0].percent - 1.9489705562).abs() < 1e-6,
            "got {}",
            src.bars[0].percent
        );
        assert!(
            (src.bars[1].percent - 4.2578577995).abs() < 1e-6,
            "got {}",
            src.bars[1].percent
        );
        assert_eq!(src.bars[2].percent, 0.0);
        assert_eq!(src.bars[3].percent, 0.0);
        assert_eq!(src.bars[0].resets_at, "2026-09-09T15:55:56Z");
        assert_eq!(src.bars[1].resets_at, "2026-09-02T20:55:56Z");
        assert!(src.bars.iter().all(|b| b.severity == "normal"));
        assert!(src.amounts.is_empty());
        assert_eq!(
            src.account, "",
            "the parser does not know who; the source fills that in"
        );
    }

    #[test]
    fn antigravity_severity_follows_what_is_used() {
        let body = r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"name":"Gemini Models","buckets":[{"id":"gemini-weekly","window":"weekly","remaining_fraction":0.05,"reset_time":"2026-09-09T15:55:56Z"},{"id":"gemini-5h","window":"5h","remaining_fraction":0.2,"reset_time":""},{"id":"new-bucket","name":"Daily Limit Remaining","window":"daily","remaining_fraction":0.5}]}]}}}"#;
        let src = parse_antigravity(0, body);
        assert_eq!(src.bars[0].severity, "critical");
        assert_eq!(src.bars[1].severity, "warning");
        assert_eq!(src.bars[1].resets_at, "");
        // A bucket agy has not named yet is still shown, under its own id.
        assert_eq!(src.bars[2].kind, "daily_new-bucket");
        assert_eq!(src.bars[2].label, "Gemini Models Daily Limit Remaining");
        assert_eq!(src.bars[2].percent, 50.0);
    }

    #[test]
    fn antigravity_credits_show_only_when_there_are_any() {
        assert_eq!(
            parse_antigravity_credits(ANTIGRAVITY_CREDITS),
            None,
            "0 is not a balance"
        );
        let some =
            ANTIGRAVITY_CREDITS.replace(r#""remaining_credits":0"#, r#""remaining_credits":250"#);
        let a = parse_antigravity_credits(&some).unwrap();
        assert_eq!(a.label, "G1 credits");
        assert_eq!(a.amount, 250.0);
        assert_eq!(a.sense, "remaining");
        assert_eq!(a.currency, "");
        assert_eq!(parse_antigravity_credits("nope"), None);
    }

    #[test]
    fn antigravity_failures_are_told_apart() {
        let rejected = parse_antigravity(1, "");
        assert_eq!(rejected.state, "rejected");
        assert!(rejected.detail.contains("Open agy"), "{}", rejected.detail);
        let refused = parse_antigravity(
            0,
            r#"{"conversation_id":"","status":"ERROR","response":"","error":"not authenticated"}"#,
        );
        assert_eq!(refused.state, "rejected");
        assert!(
            refused.detail.contains("not authenticated"),
            "{}",
            refused.detail
        );
        assert_eq!(parse_antigravity(0, "Fetching...").state, "unreachable");
        // 200-shaped with no usage in it is not "0% used".
        assert_eq!(
            parse_antigravity(
                0,
                r#"{"status":"SUCCESS","command":{"name":"usage","data":{}}}"#
            )
            .state,
            "unreachable"
        );
        assert_eq!(
            parse_antigravity(
                0,
                r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[]}}}"#
            )
            .state,
            "unreachable"
        );
    }

    /// The log line, verbatim from `log/cli-20260902_121215.log` with the
    /// glog prefix kept.
    #[test]
    fn antigravity_account_is_read_off_the_log() {
        let log = "ERROR: logging before google.Init: I0902 12:12:16.101 60 auth.go:212] applyAuthResult: email=john.m.allison@gmail.com, authMethod=consumer, quotaProject=\nnext line\n";
        assert_eq!(
            antigravity_email_in(log).as_deref(),
            Some("john.m.allison@gmail.com")
        );
        assert_eq!(antigravity_email_in("nothing here"), None);
    }

    /// Runs the real `agy`. Ignored so `cargo test` stays offline; run with
    /// `cargo test --lib -- --ignored --nocapture usage::tests::antigravity_live`.
    /// Prints, never fails: an absent or signed-out agy is a fact, not a bug.
    #[test]
    #[ignore = "runs agy"]
    fn antigravity_live_answers() {
        match antigravity_source() {
            None => println!("agy is not installed here"),
            Some(src) => {
                println!(
                    "antigravity: state={} account={} detail={}",
                    src.state, src.account, src.detail
                );
                for b in &src.bars {
                    println!(
                        "  {} ({}) {:.2}% used, {}, resets {}",
                        b.label, b.kind, b.percent, b.severity, b.resets_at
                    );
                }
                for a in &src.amounts {
                    println!("  {} {} {}", a.label, a.amount, a.sense);
                }
            }
        }
    }

    /// The live OpenRouter reply, curled with a real key while this was written.
    #[test]
    fn openrouter_credits_are_the_difference_not_the_total() {
        let body = r#"{"data":{"total_credits":551.5936,"total_usage":550.667155136}}"#;
        let src = parse_provider_credits("openrouter", "OpenRouter", 200, body);
        assert_eq!(src.state, "ok", "{}", src.detail);
        assert_eq!(src.id, "provider:openrouter");
        let a = &src.amounts[0];
        assert!((a.amount - 0.926444864).abs() < 1e-9, "got {}", a.amount);
        assert_eq!(a.sense, "remaining");
        // OpenRouter credits are dollars, so the chip prints a `$`.
        assert_eq!(a.currency, "USD");
        // The lifetime total is not a budget, so it must not become one.
        assert_eq!(a.of, None, "drew a bar against lifetime top-ups");
        assert_eq!(src.notes, vec!["550.67 spent of 551.59 ever added."]);
    }

    /// A provider with no balance endpoint is healthy, not broken — the row
    /// must not go red, and it must not claim a balance of zero either.
    #[test]
    fn a_provider_without_credits_is_not_an_error() {
        let missing =
            parse_provider_credits("openai", "OpenAI", 404, r#"{"error":{"message":"x"}}"#);
        assert_eq!(missing.state, "no_balance");
        assert!(missing.amounts.is_empty());

        let wrong_shape = parse_provider_credits("local", "llama.cpp", 200, r#"{"ok":true}"#);
        assert_eq!(wrong_shape.state, "no_balance");
        assert!(wrong_shape.amounts.is_empty());
    }

    #[test]
    fn a_bad_provider_key_is_reported_as_one() {
        let src = parse_provider_credits("openrouter", "OpenRouter", 401, "{}");
        assert_eq!(src.state, "rejected");
        assert!(
            src.detail.contains("rejected the saved API key"),
            "{}",
            src.detail
        );
    }

    /// Hits the real services. Ignored by default so `cargo test` stays offline
    /// and deterministic; run it with
    /// `cargo test --lib -- --ignored --nocapture usage::tests::live` after
    /// changing anything about the requests, and read the output. Set
    /// `OPENROUTER_API_KEY` to include the provider path.
    #[test]
    #[ignore = "makes real network calls"]
    fn live_sources_answer() {
        // Every source is tried before anything fails, because they fail
        // independently and stopping at the first one hides the other two —
        // /api/oauth/usage rate limits hard enough that it is regularly the
        // one that is down.
        let mut bad = vec![];
        let mut check = |src: UsageSource| {
            println!("{src:#?}");
            if src.state != "ok" {
                bad.push(format!("{}: {} — {}", src.id, src.state, src.detail));
            }
        };

        check(anthropic_source());
        match codex_source() {
            None => println!("codex is not installed here"),
            Some(c) => check(c),
        }
        match grok_source() {
            None => println!("grok is not installed here"),
            Some(g) => check(g),
        }
        match antigravity_source() {
            None => println!("agy is not installed here"),
            Some(a) => check(a),
        }
        match std::env::var("OPENROUTER_API_KEY") {
            Err(_) => println!("OPENROUTER_API_KEY unset — skipped the provider path"),
            Ok(key) => check(provider_source(
                &serde_json::from_value(serde_json::json!({
                    "id": "openrouter",
                    "name": "OpenRouter",
                    "base_url": "https://openrouter.ai/api/v1",
                    "api_key": key,
                }))
                .expect("a provider record"),
            )),
        }

        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// No source may ever come back as a silently empty "ok" — that renders
    /// identically to "you have used nothing", which is the one thing it must
    /// never be mistaken for.
    #[test]
    fn an_ok_source_always_carries_something() {
        for src in [
            parse_anthropic(200, ANTHROPIC_BODY),
            parse_codex(200, CODEX_BODY),
            parse_grok(200, GROK_BODY),
            parse_antigravity(0, ANTIGRAVITY_BODY),
            parse_provider_credits(
                "openrouter",
                "OpenRouter",
                200,
                r#"{"data":{"total_credits":1.0,"total_usage":0.5}}"#,
            ),
        ] {
            if src.state == "ok" {
                assert!(
                    !src.bars.is_empty() || !src.amounts.is_empty(),
                    "{} reported ok with nothing in it",
                    src.id
                );
            }
        }
    }
}
