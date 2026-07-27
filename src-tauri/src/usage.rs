//! Plan usage limits, mirrored from Claude Code's own `/usage` view. Calls the
//! same endpoint (`/api/oauth/usage`) with the OAuth token Claude Code stores
//! in `~/.claude/.credentials.json`, and returns the `limits` array as compact
//! bars for the top-bar widget. Uses `curl` so we pull in no HTTP/TLS crates.

use serde::Serialize;

#[derive(Serialize)]
pub struct UsageBar {
    /// "session" | "weekly_all" | "weekly_scoped" (from the API `kind`).
    pub kind: String,
    /// Human label for the tooltip ("Current session", "All models", "Fable").
    pub label: String,
    /// 0–100 percent used.
    pub percent: f64,
    /// "normal" | "warning" | … — drives the bar colour.
    pub severity: String,
    /// ISO-8601 reset timestamp (for the "resets in …" tooltip line).
    pub resets_at: String,
}

fn read_oauth_token() -> Option<String> {
    let path = dirs::home_dir()?.join(".claude/.credentials.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("claudeAiOauth")?
        .get("accessToken")?
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

/// Fetch current plan usage limits. Returns an empty vec on any failure (no
/// token, offline, expired token, unexpected shape) so the UI simply hides the
/// widget rather than erroring.
///
/// `(async)` is load-bearing: a plain `#[tauri::command]` body runs on the main
/// thread, so with no network this curl froze the whole window — every minute,
/// for as long as the connect took — and the freeze that landed on startup read
/// as aiterm hanging on open. Off the main thread, an offline machine just
/// keeps the cached bars.
#[tauri::command(async)]
pub fn usage_limits() -> Vec<UsageBar> {
    let Some(token) = read_oauth_token() else {
        return vec![];
    };
    let output = std::process::Command::new("curl")
        .args([
            "-sS",
            // Give up on an unreachable host quickly rather than sitting on
            // the full budget; the reply itself still gets the longer window.
            "--connect-timeout",
            "3",
            "--max-time",
            "10",
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            "anthropic-beta: oauth-2025-04-20",
            "-H",
            "Content-Type: application/json",
            "https://api.anthropic.com/api/oauth/usage",
        ])
        .output();
    let Ok(output) = output else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return vec![];
    };
    let Some(limits) = v.get("limits").and_then(|l| l.as_array()) else {
        return vec![];
    };
    limits
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
        .collect()
}
