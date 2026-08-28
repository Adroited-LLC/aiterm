//! Where you stand with every service aiterm can see: plan limits for the
//! agents it launches, and credit balances for the API providers it is
//! configured with.
//!
//! Three sources, each read the way its own tool reads it:
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
    /// "codex_primary" | "codex_secondary".
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
    /// ISO currency code, **only when the payload actually says one**.
    /// Anthropic's `spend` object carries `"currency":"USD"`; OpenRouter's
    /// `/credits` and Codex's `credits.balance` do not name a currency at all,
    /// so this is empty for them and the UI prints a bare number rather than
    /// inventing a dollar sign.
    pub currency: String,
    /// "remaining" — `amount` is what is left of `of`.
    /// "used" — `amount` is what has been spent out of `of`.
    pub sense: String,
}

/// One service's answer. Always present, even when it could not be reached —
/// see the module note on why silence is not an acceptable way to say "no".
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct UsageSource {
    /// "anthropic" | "codex" | "provider:<provider id>".
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
        src.state = "rejected".into();
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
    match curl_get("https://api.anthropic.com/api/oauth/usage", &token, &headers) {
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
        src.state = "rejected".into();
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
        match curl_get("https://chatgpt.com/backend-api/wham/usage", &auth.access_token, &headers) {
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
                // OpenRouter's reply names no currency. It is dollars in
                // practice, but the payload does not say so and this file does
                // not print what the payload did not say.
                currency: String::new(),
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
        assert_eq!(limited.state, "rejected");
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
