//! Native updater shared by Linux and Windows. The feed and package origin are fixed;
//! renderer input can select a version, never a URL or executable path.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::Manager;

const API: &str = "https://control.34-23-107-73.sslip.io:8443/updates";

static INSTALLING: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    version: String,
    asset: String,
    #[serde(skip)]
    url: String,
    sha256: String,
    size: u64,
    #[serde(default)]
    notes: String,
}
#[derive(Deserialize)]
struct Manifest {
    schema: u32,
    platforms: HashMap<String, Package>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    current_version: String,
    connected: bool,
    available: Option<Package>,
}
fn version(s: &str) -> Result<[u64; 3], String> {
    let parts = s
        .split('.')
        .map(|p| {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err("Invalid update version".into());
            }
            p.parse::<u64>()
                .map_err(|_| "Invalid update version".into())
        })
        .collect::<Result<Vec<_>, String>>()?;
    parts
        .try_into()
        .map_err(|_| "Invalid update version".into())
}
fn platform() -> Result<&'static str, String> {
    if !cfg!(target_arch = "x86_64") {
        return Err("Updates are not available for this architecture".into());
    }
    if cfg!(windows) {
        Ok("windows-x86_64")
    } else if cfg!(target_os = "linux") {
        Ok("linux-x86_64")
    } else {
        Err("Updates are not available for this platform".into())
    }
}
fn validate(p: &Package, target: &str) -> Result<(), String> {
    version(&p.version)?;
    if !p
        .asset
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err("Invalid asset name".into());
    }
    let suffix = if target == "windows-x86_64" {
        ".exe"
    } else {
        ".rpm"
    };
    if !p.asset.ends_with(suffix)
        || p.asset.contains(['/', '\\'])
        || p.asset.is_empty()
        || p.size == 0
        || p.size > 1024 * 1024 * 1024
        || p.sha256.len() != 64
        || !p.sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Invalid update package".into());
    }
    Ok(())
}
fn client() -> Result<reqwest::Client, String> {
    // Other networking modules may have already installed the same provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("AITerm updater")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
}
fn valid_invite(value: &str) -> bool {
    value.strip_prefix("aiterm_").is_some_and(|v| {
        v.len() == 43
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}
fn credential_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("update-credential"))
}
#[cfg(windows)]
fn protect_credential(value: &[u8], decrypt: bool) -> Result<Vec<u8>, String> {
    use base64::Engine;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let operation = if decrypt { "Unprotect" } else { "Protect" };
    let script = format!("Add-Type -AssemblyName System.Security; [Convert]::ToBase64String([Security.Cryptography.ProtectedData]::{operation}([Convert]::FromBase64String([Console]::In.ReadToEnd()), $null, [Security.Cryptography.DataProtectionScope]::CurrentUser))");
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not access Windows credential encryption")?;
    child
        .stdin
        .take()
        .ok_or("Credential input unavailable")?
        .write_all(
            base64::engine::general_purpose::STANDARD
                .encode(value)
                .as_bytes(),
        )
        .map_err(|_| "Could not protect credential")?;
    let output = child
        .wait_with_output()
        .map_err(|_| "Credential encryption failed")?;
    if !output.status.success() {
        return Err("Windows credential encryption failed".into());
    }
    base64::engine::general_purpose::STANDARD
        .decode(String::from_utf8_lossy(&output.stdout).trim())
        .map_err(|_| "Invalid encrypted credential".into())
}
fn credential(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    let path = credential_path(app)?;
    match std::fs::read(path) {
        Ok(value) => {
            #[cfg(windows)]
            let value = protect_credential(&value, true)?;
            let value = String::from_utf8(value).map_err(|_| "Invalid stored credential")?;
            Ok(valid_invite(value.trim()).then(|| value.trim().to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Could not read update credential".into()),
    }
}
async fn get(token: &str, url: &str, binary: bool, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = client()?
        .get(url)
        .bearer_auth(token)
        .header(
            "Accept",
            if binary {
                "application/octet-stream"
            } else {
                "application/json"
            },
        )
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| "Could not reach the update service. Try again.")?;
    if !response.status().is_success() {
        return Err(format!(
            "Update service returned {}. Check that your invite code is still active.",
            response.status().as_u16()
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Update response interrupted")?
    {
        if body.len() + chunk.len() > limit {
            return Err("Update response is too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
async fn package(token: &str, current: &str) -> Result<Option<Package>, String> {
    let manifest: Manifest =
        serde_json::from_slice(&get(token, &format!("{API}/latest"), false, 256 * 1024).await?)
            .map_err(|_| "Invalid update feed")?;
    if manifest.schema != 1 {
        return Err("Unsupported update feed".into());
    }
    let target = platform()?;
    let Some(mut p) = manifest.platforms.get(target).cloned() else {
        return Ok(None);
    };
    validate(&p, target)?;
    p.url = format!("{API}/assets/{}", p.asset);
    Ok((version(&p.version)? > version(current)?).then_some(p))
}
#[tauri::command]
pub async fn app_update_connect(app: tauri::AppHandle, token: String) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() {
        let path = credential_path(&app)?;
        if path.exists() {
            std::fs::remove_file(path).map_err(|_| "Could not disconnect updates")?;
        }
        return Ok(());
    }
    if !valid_invite(token) {
        return Err("Enter the invite code supplied by your AITerm administrator".into());
    }
    get(token, &format!("{API}/access"), false, 1024).await?;
    let path = credential_path(&app)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|_| "Could not save credential")?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    let data = protect_credential(token.as_bytes(), false)?;
    #[cfg(not(windows))]
    let data = token.as_bytes();
    options
        .open(path)
        .and_then(|mut f| f.write_all(&data))
        .map_err(|_| "Could not save credential")?;
    Ok(())
}
#[tauri::command]
pub fn app_update_settings(app: tauri::AppHandle, automatic: Option<bool>) -> Result<bool, String> {
    let path = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("automatic-updates");
    if let Some(value) = automatic {
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&path, if value { "true" } else { "false" }).map_err(|e| e.to_string())?;
        Ok(value)
    } else {
        Ok(std::fs::read_to_string(path)
            .ok()
            .is_some_and(|v| v.trim() == "true"))
    }
}
#[tauri::command]
pub async fn app_update_check(app: tauri::AppHandle) -> Result<UpdateStatus, String> {
    let current_version = app.package_info().version.to_string();
    let token = credential(&app)?;
    let available = match token.as_ref() {
        Some(t) => package(t, &current_version).await?,
        None => None,
    };
    Ok(UpdateStatus {
        current_version,
        connected: token.is_some(),
        available,
    })
}
struct InstallGuard;
impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::Release);
    }
}
#[tauri::command]
pub async fn app_update_install(
    app: tauri::AppHandle,
    version: String,
    automatic: Option<bool>,
) -> Result<String, String> {
    if INSTALLING.swap(true, Ordering::AcqRel) {
        return Err("An update is already downloading".into());
    }
    let _guard = InstallGuard;
    let token = credential(&app)?.ok_or("Enter your invite code first")?;
    let p = package(&token, &app.package_info().version.to_string())
        .await?
        .ok_or("No update available")?;
    if p.version != version {
        return Err("The release changed. Check for updates again.".into());
    }
    let automatic = automatic.unwrap_or(false);
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("updates");
    let marker = dir.join("automatic-update");
    let marker_value = format!("{}:{}", std::process::id(), p.version);
    if automatic && std::fs::read_to_string(&marker).ok().as_deref() == Some(&marker_value) {
        return Ok(
            "The update is already prepared. It will apply when you next close AITerm.".into(),
        );
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ext = if cfg!(windows) { "exe" } else { "rpm" };
    let path = dir.join(format!(
        "aiterm-{}-{}.{}",
        p.version,
        uuid::Uuid::new_v4(),
        ext
    ));
    let result = async {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let mut response = client()?
            .get(&p.url)
            .bearer_auth(&token)
            .header("Accept", "application/octet-stream")
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let mut hash = Sha256::new();
        let mut size = 0u64;
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            size += chunk.len() as u64;
            if size > p.size {
                return Err("Update download exceeds expected size".to_string());
            }
            hash.update(&chunk);
            file.write_all(&chunk).map_err(|e| e.to_string())?;
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        if size != p.size || format!("{:x}", hash.finalize()) != p.sha256.to_lowercase() {
            return Err("Update verification failed. Please try again.".into());
        }
        let install_path = path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            #[cfg(windows)] {
                if automatic {
                    use base64::Engine;
                    use std::os::windows::process::CommandExt;
                    let quoted = install_path.to_string_lossy().replace('\'', "''");
                    let script = format!("$ErrorActionPreference='Stop'; Wait-Process -Id {} -ErrorAction SilentlyContinue; Start-Process -FilePath '{}' -ArgumentList '/S' -Wait", std::process::id(), quoted);
                    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
                    std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &base64::engine::general_purpose::STANDARD.encode(bytes)])
                        .creation_flags(0x08000000).spawn().map_err(|e| e.to_string())?;
                    Ok("Update downloaded. It will install automatically when you close AITerm.".to_string())
                } else {
                    std::process::Command::new(&install_path).spawn().map_err(|e| e.to_string())?;
                    Ok("Continue in the Windows installer.".to_string())
                }
            }
            #[cfg(not(windows))] {
                let status = if automatic {
                    {
                        let unattended = std::process::Command::new("sudo").args(["-n", "true"])
                            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
                        let mut command = std::process::Command::new(if unattended { "sudo" } else { "pkexec" });
                        if unattended { command.arg("-n"); }
                        command.args(["/usr/bin/dnf", "install", "-y"]).arg(&install_path).status()
                    }
                } else {
                    std::process::Command::new("xdg-open").arg(&install_path).status()
                }.map_err(|e| e.to_string())?;
                if !status.success() { return Err("Installation was cancelled or could not start. Try again from App updates.".into()); }
                Ok(if automatic { "Update installed. It will take effect the next time you launch AITerm." } else { "Continue in the system package installer." }.to_string())
            }
        }).await.map_err(|e| e.to_string())?

    }
    .await;
    if result.is_ok() && automatic {
        let _ = std::fs::write(marker, marker_value);
    }
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_invites_are_sent_to_the_update_service() {
        assert!(valid_invite(&format!("aiterm_{}", "x".repeat(43))));
        for value in ["github_pat_secret", "ghp_secret", "aiterm_short", ""] {
            assert!(!valid_invite(value));
        }
    }
    #[test]
    fn version_order_and_invalid_versions() {
        assert!(version("0.10.100").unwrap() > version("0.10.99").unwrap());
        for bad in ["1.0", "1.2.3-beta", "1.2.3.4", "../1", "-1.0.0"] {
            assert!(version(bad).is_err());
        }
    }
    #[test]
    fn reject_wrong_origin_type_and_unbounded_packages() {
        let mut p = Package {
            version: "1.0.0".into(),
            asset: "app.rpm".into(),
            url: String::new(),
            sha256: "a".repeat(64),
            size: 123,
            notes: String::new(),
        };
        assert!(validate(&p, "linux-x86_64").is_ok());
        assert!(validate(&p, "windows-x86_64").is_err());
        p.asset = "../app.rpm".into();
        assert!(validate(&p, "linux-x86_64").is_err());
        p.asset = "app.rpm".into();
        p.size = u64::MAX;
        assert!(validate(&p, "linux-x86_64").is_err());
    }
}
