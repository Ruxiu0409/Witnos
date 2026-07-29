//! Embedded terminal: real PTYs owned by the Tauri shell, rendered by
//! xterm.js in the webview. This is pure UX shell — the store, the gate,
//! and the contract never touch it. The point is that opening Witnos is
//! enough to drive your agent; no external terminal needed.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

/// All live PTY sessions, keyed by a webview-chosen id (fresh per mount, so
/// React StrictMode's double-mount can't cross-wire two shells).
#[derive(Default)]
pub struct Terminals(Mutex<HashMap<u32, Session>>);

#[derive(Clone, Serialize)]
struct Output {
    id: u32,
    data: Vec<u8>,
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn default_shell() -> String {
    if cfg!(windows) {
        return std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    }
    std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/bin/zsh".into()
        } else {
            "/bin/bash".into()
        }
    })
}

#[tauri::command]
pub fn term_spawn(
    app: AppHandle,
    state: State<'_, Terminals>,
    id: u32,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<(), String> {
    let pair = native_pty_system()
        .openpty(size(cols, rows))
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(default_shell());
    // Login shell so a GUI-launched app still gets the user's real PATH
    // (agent CLIs, cargo, …).
    if cfg!(unix) {
        cmd.arg("-l");
    }
    cmd.env("TERM", "xterm-256color");
    // Make the bundled `witnos` CLI reachable by name for the human and for
    // agents launched from this shell (agent-facing instructions carry the
    // absolute path anyway — this is convenience, not a load-bearing link).
    if let Some(bin_dir) = crate::bundled_cli().as_deref().and_then(Path::parent) {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let inherited = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}{sep}{inherited}", bin_dir.display()));
    }
    let dir = cwd
        .filter(|d| !d.is_empty())
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".into());
    cmd.cwd(dir);

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    state.0.lock().unwrap().insert(
        id,
        Session {
            master: pair.master,
            writer,
            child,
        },
    );

    // Raw bytes forwarded as-is; xterm.js does its own UTF-8 decoding, so a
    // multibyte character split across read chunks still renders correctly.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = app.emit(
                        "term:output",
                        Output {
                            id,
                            data: buf[..n].to_vec(),
                        },
                    );
                }
            }
        }
        if let Some(state) = app.try_state::<Terminals>() {
            state.0.lock().unwrap().remove(&id);
        }
        let _ = app.emit("term:exit", id);
    });

    Ok(())
}

#[tauri::command]
pub fn term_write(state: State<'_, Terminals>, id: u32, data: String) -> Result<(), String> {
    let mut sessions = state.0.lock().unwrap();
    let s = sessions.get_mut(&id).ok_or("no such terminal")?;
    s.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn term_resize(
    state: State<'_, Terminals>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let sessions = state.0.lock().unwrap();
    let s = sessions.get(&id).ok_or("no such terminal")?;
    s.master.resize(size(cols, rows)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn term_kill(state: State<'_, Terminals>, id: u32) {
    // Dropping the session closes the PTY master (HUP to the foreground
    // process group); kill the shell itself for good measure.
    if let Some(mut s) = state.0.lock().unwrap().remove(&id) {
        let _ = s.child.kill();
    }
}
