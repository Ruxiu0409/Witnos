//! The Windows backend: the app owns its PTYs in process.
//!
//! Why it exists at all: detached terminals rest on a PTY whose foreground
//! process group can be read and a filesystem socket to carry it, and ConPTY
//! offers neither (the daemon says so itself, in `pty_serve`). Pretending
//! otherwise would produce panes that cannot answer the safety question deciding
//! whether anything may be typed into them. So here the shells belong to the app,
//! and the honest costs are stated rather than papered over: nothing survives a
//! restart (`surviving` is the empty list), output printed while no view is
//! attached is gone (there is no ring to keep it in), and `foreground` answers
//! neither way — nothing is typed into a pane we cannot read, not a `cd` and not
//! a correction. The human still has the explicit restart-here button, and a
//! Windows agent simply never gets typed into.
#![cfg_attr(all(unix, test), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use super::{PaneEvent, PaneInfo, Sink};

pub struct Terminals {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    sessions: Mutex<HashMap<u32, Session>>,
    next_token: AtomicU64,
}

/// One view's claim on a pane. The token is what lets a `detach` say which
/// attachment it is letting go of: a view can be unmounted while its own attach
/// is still in flight, and a late "I'm done" must not cut off the view that has
/// meanwhile taken the pane over.
struct Attached {
    token: u64,
    sink: Sink,
}

struct Session {
    /// Where the shell was started. There is no OSC-7 tracking here; the webview
    /// does that for the header, and this is only what `list` reports.
    cwd: String,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Where this pane's output goes right now, or `None` while nothing is
    /// attached. One pump thread per session for its whole life: a PTY's read
    /// side cannot be handed to a second reader later, so attaching swaps the
    /// SINK rather than the reader.
    attached: Arc<Mutex<Option<Attached>>>,
}

impl Default for Terminals {
    fn default() -> Self {
        Terminals {
            inner: Arc::new(Inner::default()),
        }
    }
}

impl Terminals {
    /// Nothing here can outlive the app that owns it, so this is the empty list
    /// and never `None`: it is a certainty, not an unanswered question.
    pub fn surviving(&self) -> Option<Vec<PaneInfo>> {
        Some(Vec::new())
    }

    pub fn ensure(
        &self,
        id: Option<u32>,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        env: &BTreeMap<String, String>,
    ) -> Result<u32, String> {
        if let Some(id) = id {
            // The daemon's rule, kept: an id names a session or it names nothing.
            // A pane must never be handed a different shell under the id a goal's
            // session binding points at. Since nothing survives a restart here,
            // this only ever succeeds within one run.
            self.resize(id, cols, rows)?;
            return Ok(id);
        }
        let id = next_id();
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|e| e.to_string())?;
        let dir = cwd
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .or_else(|| std::env::var("USERPROFILE").ok())
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| ".".into());
        let child = pair
            .slave
            .spawn_command(shell_command(&dir, env, id))
            .map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let attached: Arc<Mutex<Option<Attached>>> = Arc::new(Mutex::new(None));
        self.inner.sessions.lock().unwrap().insert(
            id,
            Session {
                cwd: dir,
                master: pair.master,
                writer,
                child,
                attached: attached.clone(),
            },
        );

        let inner = Arc::downgrade(&self.inner);
        std::thread::spawn(move || pump(inner, id, attached, &mut reader));
        Ok(id)
    }

    /// Point this pane's output at `sink`, and answer with the token naming this
    /// attachment. There is no replay: whatever the pane printed while nothing
    /// was attached is gone, the same bargain as the shells not surviving the
    /// app, and for the same reason.
    pub fn attach(&self, id: u32, sink: Sink) -> Result<u64, String> {
        let sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.get(&id).ok_or("no such terminal")?;
        let token = self.inner.next_token.fetch_add(1, Ordering::SeqCst) + 1;
        *session.attached.lock().unwrap() = Some(Attached { token, sink });
        Ok(token)
    }

    /// Stop reporting a pane's output, and leave the shell alone. A token that no
    /// longer matches is a no-op — see `Attached`.
    pub fn detach(&self, id: u32, token: Option<u64>) {
        if let Some(session) = self.inner.sessions.lock().unwrap().get(&id) {
            let mut attached = session.attached.lock().unwrap();
            if token.is_some_and(|t| attached.as_ref().is_some_and(|a| a.token != t)) {
                return;
            }
            *attached = None;
        }
    }

    pub fn list(&self) -> Result<Vec<PaneInfo>, String> {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let mut panes: Vec<PaneInfo> = sessions
            .iter_mut()
            .map(|(id, session)| PaneInfo {
                id: *id,
                cwd: session.cwd.clone(),
                alive: matches!(session.child.try_wait(), Ok(None)),
            })
            .collect();
        panes.sort_by_key(|p| p.id);
        Ok(panes)
    }

    pub fn write(&self, id: u32, bytes: &[u8]) -> Result<(), String> {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.get_mut(&id).ok_or("no such terminal")?;
        session.writer.write_all(bytes).map_err(|e| e.to_string())
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        let sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.get(&id).ok_or("no such terminal")?;
        session
            .master
            .resize(size(cols, rows))
            .map_err(|e| e.to_string())
    }

    pub fn kill(&self, id: u32) {
        // Dropping the session closes the PTY master (HUP to the foreground
        // process group); kill the shell itself for good measure.
        if let Some(mut session) = self.inner.sessions.lock().unwrap().remove(&id) {
            let _ = session.child.kill();
        }
    }

    /// ConPTY exposes no foreground-process signal, so neither question has an
    /// answer here — and both answers are needed for their own direction of
    /// safety. So a live pane answers NEITHER way, which every caller reads as
    /// "do not type into it", and a pane that is gone answers `None`.
    pub fn foreground(&self, id: u32) -> Result<Option<(bool, bool)>, String> {
        Ok(self
            .inner
            .sessions
            .lock()
            .unwrap()
            .contains_key(&id)
            .then_some((false, false)))
    }
}

fn pump(
    inner: Weak<Inner>,
    id: u32,
    attached: Arc<Mutex<Option<Attached>>>,
    reader: &mut dyn Read,
) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Some(a) = attached.lock().unwrap().as_ref() {
                    (a.sink)(id, PaneEvent::Output(buf[..n].to_vec()));
                }
            }
        }
    }
    if let Some(inner) = inner.upgrade() {
        inner.sessions.lock().unwrap().remove(&id);
    }
    if let Some(a) = attached.lock().unwrap().as_ref() {
        (a.sink)(id, PaneEvent::Ended);
    }
}

/// Pane ids start from a fresh base each run, and they have to: `WITNOS_PANE` is
/// the address a human's correction is typed to, and a goal recorded in a
/// previous run still names a pane by number — handing that number to a new
/// shell would aim the correction at somebody else's terminal. Where there is a
/// daemon, persisting the allocator is its job; here nothing survives to persist
/// it in, so a clock-seeded base is what keeps two runs apart.
fn next_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    if id == 0 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as u32)
            .unwrap_or(1);
        // Ids start at 1: 0 reads like "unset" everywhere a pane id is carried in
        // an environment variable.
        NEXT.store((seed % 0x4000_0000) + 2, Ordering::SeqCst);
        return (seed % 0x4000_0000) + 1;
    }
    id
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
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

/// The shell a pane runs, in the same shape the daemon gives it: a login shell
/// (a GUI-launched app otherwise has no user PATH, so no agent CLIs) and `TERM`
/// set first, so a client that wants another one can simply pass it.
fn shell_command(cwd: &str, env: &BTreeMap<String, String>, id: u32) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(default_shell());
    if cfg!(unix) {
        cmd.arg("-l");
    }
    cmd.env("TERM", "xterm-256color");
    for (key, value) in env {
        // `{id}` inside a VALUE is the pane id, exactly as in the daemon
        // protocol: whoever allocates the id substitutes it, so the half that
        // knows what `WITNOS_PANE` means never has to know the id.
        cmd.env(key, value.replace(super::PANE_ID, &id.to_string()));
    }
    cmd.cwd(cwd);
    cmd
}
