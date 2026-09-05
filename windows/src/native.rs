//! Operations that belong to the Windows desktop, rather than its Linux workspace.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tauri::Manager;

pub fn dispatch(
    command: &str,
    args: &Value,
    app: &tauri::AppHandle,
) -> Option<Result<Value, String>> {
    Some(match command {
        "open_path" => crate::bridge::rpc(command.into(), args.clone()),
        "list_fonts" => {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                db.load_fonts_dir(std::path::Path::new(&local).join("Microsoft/Windows/Fonts"));
            }
            let mut families = BTreeMap::<String, bool>::new();
            for face in db.faces() {
                for (name, _) in &face.families {
                    *families.entry(name.clone()).or_default() |= face.monospaced;
                }
            }
            Ok(json!(families
                .into_iter()
                .map(|(name, mono)| json!({"name":name,"mono":mono}))
                .collect::<Vec<_>>()))
        }
        "font_packages" => Ok(json!([])),
        "install_font_files" => install_fonts(args).map(|n| json!(n)),
        "install_font_package" => Err("Install Windows fonts using Choose files.".into()),
        "renderer_probe" => Ok(json!({"cpuMs":0,"gpuMs":null,"ok":false})),
        "tray_alerts" => serde_json::from_value(args["alerts"].clone())
            .map_err(|e| e.to_string())
            .and_then(|alerts| crate::tray::tray_alerts(app.clone(), alerts))
            .map(|_| Value::Null),
        "taskbar_badge" => badge(app, args["count"].as_u64().unwrap_or(0)).map(|_| Value::Null),
        "desktop_notify" => toast(args, app).map(|id| json!(id)),
        "desktop_notify_close" => close_toast(args).map(|_| Value::Null),
        _ => return None,
    })
}

fn badge(app: &tauri::AppHandle, count: u64) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("No desktop window")?;
    // A small amber waiting indicator; the tray menu carries the exact list.
    let icon = if count == 0 {
        None
    } else {
        let mut pixels = vec![0u8; 16 * 16 * 4];
        for y in 0i32..16 {
            for x in 0i32..16 {
                if (x - 8).pow(2) + (y - 8).pow(2) <= 49 {
                    pixels[((y * 16 + x) * 4) as usize..((y * 16 + x) * 4 + 4) as usize]
                        .copy_from_slice(&[245, 180, 65, 255]);
                }
            }
        }
        Some(tauri::image::Image::new_owned(pixels, 16, 16))
    };
    window.set_overlay_icon(icon).map_err(|e| e.to_string())
}

fn install_fonts(args: &Value) -> Result<usize, String> {
    use windows::{core::PCWSTR, Win32::Graphics::Gdi::AddFontResourceW};
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let paths: Vec<String> =
        serde_json::from_value(args["paths"].clone()).map_err(|e| e.to_string())?;
    let dir = std::path::PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("No local app data directory")?,
    )
    .join("Microsoft/Windows/Fonts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(r"Software\Microsoft\Windows NT\CurrentVersion\Fonts")
        .map_err(|e| e.to_string())?;
    let mut count = 0;
    for path in paths {
        let path = std::path::Path::new(&path);
        if !matches!(
            path.extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("ttf" | "otf" | "ttc" | "otc")
        ) {
            return Err("Choose a TrueType or OpenType font file.".into());
        }
        let mut db = fontdb::Database::new();
        db.load_font_file(path).map_err(|e| e.to_string())?;
        if db.faces().next().is_none() {
            return Err("The file contains no supported fonts.".into());
        }
        let destination = dir.join(path.file_name().ok_or("Invalid font filename")?);
        if path != destination {
            // Never overwrite a different installed font silently.
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    file.write_all(&bytes).map_err(|e| e.to_string())?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if std::fs::read(&destination).map_err(|e| e.to_string())? != bytes {
                        return Err(format!(
                            "{} is already installed with different contents.",
                            destination.display()
                        ));
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        let wide: Vec<u16> = destination
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        if unsafe { AddFontResourceW(PCWSTR(wide.as_ptr())) } == 0 {
            return Err("Windows could not activate this font.".into());
        }
        key.set_value(
            destination.file_name().unwrap().to_string_lossy().as_ref(),
            &destination.to_string_lossy().as_ref(),
        )
        .map_err(|e| e.to_string())?;
        count += 1;
    }
    Ok(count)
}

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Mutex, OnceLock,
};
use windows::{
    core::HSTRING,
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier},
};
static TOASTS: OnceLock<Mutex<BTreeMap<u32, (ToastNotifier, ToastNotification)>>> = OnceLock::new();
fn toast(args: &Value, app: &tauri::AppHandle) -> Result<u32, String> {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    let xml = format!("<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>", escape(args["summary"].as_str().unwrap_or("aiterm")), escape(args["body"].as_str().unwrap_or("")));
    let document = XmlDocument::new().map_err(|e| e.to_string())?;
    document
        .LoadXml(&HSTRING::from(xml))
        .map_err(|e| e.to_string())?;
    let notification =
        ToastNotification::CreateToastNotification(&document).map_err(|e| e.to_string())?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
        &app.config().identifier,
    ))
    .map_err(|e| e.to_string())?;
    let replaces = args["replaces"].as_u64().unwrap_or(0) as u32;
    close_toast(&json!({"id":replaces}))?;
    notifier.Show(&notification).map_err(|e| e.to_string())?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut toasts = TOASTS.get_or_init(Default::default).lock().unwrap();
    if toasts.len() >= 128 {
        if let Some((_, (n, t))) = toasts.pop_first() {
            let _ = n.Hide(&t);
        }
    }
    toasts.insert(id, (notifier, notification));
    Ok(id)
}
fn close_toast(args: &Value) -> Result<(), String> {
    let id = args["id"].as_u64().unwrap_or(0) as u32;
    if let Some((notifier, notification)) = TOASTS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&id)
    {
        notifier.Hide(&notification).map_err(|e| e.to_string())?;
    }
    Ok(())
}
