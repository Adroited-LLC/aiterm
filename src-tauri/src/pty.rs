use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

pub struct PtyInstance {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Default)]
pub struct PtyManager {
    ptys: Mutex<HashMap<u32, PtyInstance>>,
    next_id: AtomicU32,
}

#[derive(Clone, Serialize)]
struct PtyExit {
    id: u32,
}

#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    state: State<'_, PtyManager>,
    cwd: Option<String>,
    command: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<u32, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let mut cmd = match &command {
        // Run through the login shell so PATH/aliases resolve like a normal terminal.
        Some(c) => {
            let mut b = CommandBuilder::new(&shell);
            b.args(["-i", "-c", c]);
            b
        }
        None => CommandBuilder::new(&shell),
    };
    cmd.env("TERM", "xterm-256color");
    if let Some(dir) = cwd.filter(|d| std::path::Path::new(d).is_dir()) {
        cmd.cwd(dir);
    }

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    let killer = child.clone_killer();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let id = state.next_id.fetch_add(1, Ordering::SeqCst);
    state.ptys.lock().unwrap().insert(
        id,
        PtyInstance {
            master: pair.master,
            writer,
            killer,
        },
    );

    let app_reader = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Lossy is fine: xterm.js handles the byte stream as utf8 text.
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app_reader.emit(&format!("pty://output/{id}"), data);
                }
            }
        }
        let _ = app_reader.emit("pty://exit", PtyExit { id });
    });

    Ok(id)
}

#[tauri::command]
pub fn pty_write(state: State<'_, PtyManager>, id: u32, data: String) -> Result<(), String> {
    let mut ptys = state.ptys.lock().unwrap();
    let pty = ptys.get_mut(&id).ok_or("no such pty")?;
    pty.writer
        .write_all(data.as_bytes())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_resize(state: State<'_, PtyManager>, id: u32, cols: u16, rows: u16) -> Result<(), String> {
    let ptys = state.ptys.lock().unwrap();
    let pty = ptys.get(&id).ok_or("no such pty")?;
    pty.master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_kill(state: State<'_, PtyManager>, id: u32) -> Result<(), String> {
    let mut ptys = state.ptys.lock().unwrap();
    if let Some(mut pty) = ptys.remove(&id) {
        let _ = pty.killer.kill();
    }
    Ok(())
}
