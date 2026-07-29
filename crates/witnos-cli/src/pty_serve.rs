//! `witnos pty-serve` — the PTY daemon: a long-lived process that owns the
//! agent's terminals and outlives every client of them.
//!
//! Why it exists: a Witnos goal is bound to one Claude Code session id, and a
//! session id never comes back. While the GUI owned the PTYs in-process,
//! quitting the app hung up the shells and the agent session died with them,
//! orphaning the contract the human had spent the whole run building up. This is
//! tmux's one load-bearing trick — the shells must not belong to the process
//! you close — taken without taking tmux as a dependency, because the
//! load-bearing path stays one language and one repo.
//!
//! Unix only; see `run` for what Windows gets and why.
//!
//! # Protocol
//!
//! One Unix socket at `$WITNOS_HOME/pty.sock` (mode 0600) carries two kinds of
//! connection. Every connection opens with exactly one line of JSON — a hello —
//! and the daemon answers with exactly one line of JSON. What happens after
//! that depends on the kind.
//!
//! ## Control connection — low-frequency verbs
//!
//! ```text
//! →  {"hello":"control"}
//! ←  {"ok":true,"hello":"control","pid":41231,"sock":"…/pty.sock"}
//! ```
//!
//! Then newline-delimited JSON, one response line per request line, in order.
//! Several control connections may be open at once; the app is expected to keep
//! one. Every response carries `ok`; a failure carries `error` and nothing else.
//!
//! - `{"op":"open","cwd":"/path","env":{"K":"V"},"cols":80,"rows":24}`
//!   → `{"ok":true,"id":7}`
//!   Allocates a session and spawns the user's `$SHELL` as a login shell in it.
//!   All fields optional (`cwd` defaults to `$HOME`, 80x24). The daemon sets
//!   only `TERM`; `env` is the client's, and any `{id}` in a VALUE is replaced
//!   with the allocated id — that is how `WITNOS_PANE` gets stamped without the
//!   daemon knowing what it is.
//! - `{"op":"list"}`
//!   → `{"ok":true,"sessions":[{"id":7,"cwd":"…","cols":80,"rows":24,
//!      "alive":true,"clients":1}]}`, ascending by id.
//! - `{"op":"resize","id":7,"cols":100,"rows":30}` → `{"ok":true}`
//! - `{"op":"kill","id":7}` → `{"ok":true}` — SIGHUP to the pane's foreground
//!   process group, then the shell. The only way a session ever ends early.
//! - `{"op":"foreground","id":7}`
//!   → `{"ok":true,"at_prompt":false,"program_running":true}`
//!   Both false means "the PTY did not answer" — refuse, do not guess.
//!
//! ## Data connection — one per session, raw bytes both ways
//!
//! ```text
//! →  {"hello":"attach","id":7}
//! ←  {"ok":true,"id":7,"cols":80,"rows":24,"replay_bytes":4096}
//! ```
//!
//! …and from the byte after that newline the connection is raw PTY traffic in
//! both directions, with no framing at all: an agent printing megabytes must not
//! pay for base64 or per-chunk headers. The first `replay_bytes` bytes are the
//! scrollback replay, then live output continues with no gap and no duplication
//! (see `Out` in `pty_session`); `replay_bytes` is informational — a terminal
//! emulator does not need to know where the seam is.
//!
//! Client note: read the ack line and the raw stream through ONE buffered
//! reader. A fresh reader after a buffered `read_line` would lose whatever
//! replay bytes the first read pulled in alongside the ack.
//!
//! Several data connections may attach to one session (two windows on the same
//! pane); each gets its own replay. Dropping one never touches the child —
//! that is the entire point. End-of-stream on the socket means the session
//! ended.
//!
//! # Lifetime
//!
//! Start is idempotent: an advisory lock on `$WITNOS_HOME/pty.lock` decides who
//! serves, so a second `pty-serve` prints who is already running and exits 0.
//! Spawn it detached and retry-connect to the socket; it does not background
//! itself. It exits on its own once it has no sessions and no connected client
//! (a grace period covers the gap between spawn and first connect), and never
//! while a session is alive.

#[cfg(unix)]
pub use unix::run;

/// On Windows there is no daemon at all. The two things this design rests on —
/// a PTY whose foreground process group can be read, and a filesystem socket to
/// carry it — are exactly what ConPTY does not offer, and pretending otherwise
/// would silently produce panes that cannot answer the safety question that
/// decides whether anything may be typed into them. So the app keeps owning its
/// terminals in-process there, and the honest cost is that they still die with
/// it.
#[cfg(not(unix))]
pub fn run() -> std::process::ExitCode {
    eprintln!(
        "witnos pty-serve: unix only.\n\
         Detached terminals need a PTY whose foreground process group can be\n\
         read and a filesystem socket to carry it; ConPTY offers neither, so on\n\
         Windows the app owns its terminals in-process."
    );
    std::process::ExitCode::FAILURE
}

#[cfg(unix)]
mod unix {
    use std::collections::{BTreeMap, HashMap};
    use std::fs::{File, OpenOptions, Permissions};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use serde::Deserialize;
    use serde_json::{json, Value};

    use crate::paths;
    use crate::pty_session::Session;

    const SOCK: &str = "pty.sock";
    const LOCK: &str = "pty.lock";
    const IDS: &str = "pty-ids.json";
    const LOG: &str = "pty-serve.log";

    /// How long an empty daemon waits before exiting. Long enough that a client
    /// which just spawned one always wins the race to connect, short enough
    /// that a machine does not collect daemons. Tunable for tests, which cannot
    /// afford to wait out the default.
    const IDLE_EXIT_SECS: u64 = 20;

    /// Read all at once so a hello that arrives split cannot pin a thread, and
    /// so a connection that says nothing at all cannot keep the daemon from
    /// ever going idle. Applies to the hello only.
    const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

    // ---------- entry ----------

    pub fn run() -> ExitCode {
        let home = paths::witnos_home();
        if let Err(e) = std::fs::create_dir_all(&home) {
            eprintln!("witnos pty-serve: cannot create {}: {e}", home.display());
            return ExitCode::FAILURE;
        }

        let _lock = match take_lock(&home.join(LOCK)) {
            Lock::Held(f) => f,
            Lock::Busy(who) => {
                // Idempotent by design: the app's "make sure one is running"
                // path spawns this blindly and then connects to the socket.
                println!("witnos pty-serve: already running{who}");
                return ExitCode::SUCCESS;
            }
            Lock::Failed(e) => {
                eprintln!("witnos pty-serve: {e}");
                return ExitCode::FAILURE;
            }
        };

        // Detach from whoever started us: the app spawns this as a child, and a
        // signal aimed at the app's process group (or a dev's Ctrl-C) must not
        // take the terminals down with it — that is the failure this whole
        // daemon exists to remove.
        // SAFETY: setsid on ourselves; it fails harmlessly if we already lead a
        // session, which is the state we wanted anyway.
        unsafe { libc::setsid() };

        let sock = home.join(SOCK);
        // We hold the lock, so no other process can be serving this path:
        // anything here is a corpse left by a daemon that was killed before it
        // could clean up. Unlinking it is what keeps one `kill -9` from wedging
        // every later start.
        let stale = sock.exists();
        if stale {
            let _ = std::fs::remove_file(&sock);
        }
        // Bind under a private umask rather than chmod-ing afterwards: the
        // window between the two would be a socket anyone could talk to.
        // SAFETY: umask is a per-process value; it is restored below.
        let prev_umask = unsafe { libc::umask(0o077) };
        let listener = UnixListener::bind(&sock);
        // SAFETY: as above.
        unsafe { libc::umask(prev_umask) };
        let listener = match listener {
            Ok(l) => l,
            Err(e) => {
                eprintln!("witnos pty-serve: cannot bind {}: {e}", sock.display());
                // The one failure whose message the OS makes unreadable, and
                // it is reachable from a user setting: `sockaddr_un` holds
                // ~104 bytes of path, so a deep $WITNOS_HOME cannot host a
                // socket at all.
                let len = sock.as_os_str().len();
                if len > 100 {
                    eprintln!(
                        "  that path is {len} bytes long and a unix socket has \
                         to fit in about 104 — point $WITNOS_HOME somewhere shorter."
                    );
                }
                return ExitCode::FAILURE;
            }
        };
        let _ = std::fs::set_permissions(&sock, Permissions::from_mode(0o600));

        // Only now: everything above can still fail, and whoever started this
        // deserves to see why on the stderr they were watching rather than in a
        // file they do not know about yet.
        redirect_stderr(&home.join(LOG));
        let daemon = Arc::new(Daemon::new(home));
        if stale {
            log("removed a stale socket left by a daemon that was killed");
        }
        log(format!("listening on {}", sock.display()));
        spawn_idle_monitor(daemon.clone(), sock);

        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let d = daemon.clone();
                    thread::spawn(move || serve(d, s));
                }
                Err(e) => log(format!("accept failed: {e}")),
            }
        }
        ExitCode::SUCCESS
    }

    // ---------- the daemon slot ----------

    enum Lock {
        Held(File),
        Busy(String),
        Failed(String),
    }

    /// Own the daemon slot, or discover who already does.
    ///
    /// An advisory lock rather than a pidfile: the kernel drops it when the
    /// holder dies, however it dies, so "is one already running" is answered by
    /// trying to take it instead of by trusting a file a `kill -9` would have
    /// left behind. That is also what makes a stale socket harmless — whoever
    /// wins the lock is by definition the only process that could be serving
    /// the socket, so it may unlink whatever is lying there.
    fn take_lock(path: &Path) -> Lock {
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
        {
            Ok(f) => f,
            Err(e) => return Lock::Failed(format!("cannot open {}: {e}", path.display())),
        };
        // SAFETY: flock on an fd we own; LOCK_NB means it cannot block.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                let who = std::fs::read_to_string(path)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .map(|pid| format!(" (pid {pid})"))
                    .unwrap_or_default();
                return Lock::Busy(who);
            }
            return Lock::Failed(format!("cannot lock {}: {e}", path.display()));
        }
        // Our pid, purely so a human reading the file knows who to look at.
        let _ = file.set_len(0);
        let _ = (&file).write_all(format!("{}\n", std::process::id()).as_bytes());
        Lock::Held(file)
    }

    /// Point stderr at the log file. fd 2 rather than a logger, so a panic
    /// message lands there too — a daemon nobody can see is undebuggable.
    /// Append, never rotated: the only things logged are session lifecycle and
    /// errors, a handful of lines per session.
    fn redirect_stderr(path: &Path) {
        if let Ok(f) = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
        {
            // SAFETY: dup2 onto fd 2 from an fd we own. `f` closing afterwards
            // leaves fd 2 pointing at the same file.
            unsafe { libc::dup2(f.as_raw_fd(), 2) };
        }
    }

    fn log(msg: impl std::fmt::Display) {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let d = secs % 86_400;
        eprintln!(
            "[{:02}:{:02}:{:02}Z pid {}] {msg}",
            d / 3600,
            (d % 3600) / 60,
            d % 60,
            std::process::id()
        );
    }

    // ---------- state ----------

    struct Daemon {
        home: PathBuf,
        sessions: Mutex<HashMap<u32, Arc<Session>>>,
        next_id: Mutex<u32>,
        clients: AtomicUsize,
        start: Instant,
        /// Millis since `start` at the last moment the daemon had a reason to
        /// live. The idle clock runs from here.
        touched_ms: AtomicU64,
    }

    impl Daemon {
        fn new(home: PathBuf) -> Self {
            let next_id = load_next_id(&home);
            Daemon {
                home,
                sessions: Mutex::new(HashMap::new()),
                next_id: Mutex::new(next_id),
                clients: AtomicUsize::new(0),
                start: Instant::now(),
                touched_ms: AtomicU64::new(0),
            }
        }

        fn touch(&self) {
            let ms = self.start.elapsed().as_millis() as u64;
            self.touched_ms.store(ms, Ordering::SeqCst);
        }

        fn idle_for(&self) -> Duration {
            let now = self.start.elapsed().as_millis() as u64;
            Duration::from_millis(now.saturating_sub(self.touched_ms.load(Ordering::SeqCst)))
        }

        /// Session ids must be monotonic across daemon restarts, not per-process
        /// counters: `WITNOS_PANE` is how a goal names the terminal its agent
        /// lives in, so handing a new session an id a dead one used would aim a
        /// human's correction at somebody else's pane. Hence a file — and the
        /// NEXT id is persisted BEFORE the current one is handed out, so a
        /// daemon killed mid-open loses an id rather than reusing one.
        fn alloc_id(&self) -> Result<u32, String> {
            let mut next = self.next_id.lock().unwrap();
            let id = *next;
            let following = id.checked_add(1).ok_or("session id space exhausted")?;
            let path = self.home.join(IDS);
            paths::write_atomic_checked(&path, &format!("{{\"next_id\":{following}}}\n")).map_err(
                |e| format!("cannot persist the id allocator ({}): {e}", path.display()),
            )?;
            *next = following;
            Ok(id)
        }

        fn open(
            self: &Arc<Self>,
            cwd: Option<&str>,
            env: &BTreeMap<String, String>,
            cols: u16,
            rows: u16,
        ) -> Result<u32, String> {
            let id = self.alloc_id()?;
            let (session, reader) = Session::open(id, cwd, env, cols, rows)?;
            let out = session.out.clone();
            log(format!(
                "session {id} opened in {} ({cols}x{rows})",
                session.cwd
            ));
            // Registered before the pump starts: a shell that dies instantly
            // must not have its reap race the insert.
            self.sessions.lock().unwrap().insert(id, session);
            self.touch();
            let d = self.clone();
            thread::spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => out.push(&buf[..n]),
                    }
                }
                d.finish(id, "shell exited");
            });
            Ok(id)
        }

        fn get(&self, id: u32) -> Option<Arc<Session>> {
            self.sessions.lock().unwrap().get(&id).cloned()
        }

        /// Drop a session: out of the registry, its listeners given their last
        /// bytes and then end-of-stream, its child reaped.
        fn finish(&self, id: u32, why: &str) -> bool {
            let Some(session) = self.sessions.lock().unwrap().remove(&id) else {
                return false;
            };
            session.out.close_all();
            session.reap();
            log(format!("session {id} ended ({why})"));
            self.touch();
            true
        }

        fn kill(&self, id: u32) -> bool {
            let Some(session) = self.get(id) else {
                return false;
            };
            session.hangup();
            self.finish(id, "killed")
        }

        fn list(&self) -> Vec<Value> {
            let mut sessions: Vec<Arc<Session>> =
                self.sessions.lock().unwrap().values().cloned().collect();
            sessions.sort_by_key(|s| s.id);
            sessions
                .iter()
                .map(|s| {
                    let (cols, rows) = s.size();
                    json!({
                        "id": s.id,
                        "cwd": s.cwd,
                        "cols": cols,
                        "rows": rows,
                        "alive": s.alive(),
                        "clients": s.out.listeners(),
                    })
                })
                .collect()
        }
    }

    fn load_next_id(home: &Path) -> u32 {
        std::fs::read_to_string(home.join(IDS))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.get("next_id").and_then(Value::as_u64))
            .and_then(|n| u32::try_from(n).ok())
            // Ids start at 1: 0 reads like "unset" everywhere a pane id is
            // carried in an environment variable.
            .filter(|n| *n >= 1)
            .unwrap_or(1)
    }

    // ---------- idle exit ----------

    /// Exit once nothing is left to hold. The daemon must never exit while a
    /// session is alive — that is the entire point of it — but an empty one
    /// must go away, or a machine collects daemons nobody can see. The grace
    /// period is what keeps a freshly spawned daemon alive long enough for the
    /// client that spawned it to connect.
    fn spawn_idle_monitor(d: Arc<Daemon>, sock: PathBuf) {
        let grace = Duration::from_secs(
            std::env::var("WITNOS_PTY_IDLE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(IDLE_EXIT_SECS),
        );
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(200));
            let busy =
                !d.sessions.lock().unwrap().is_empty() || d.clients.load(Ordering::SeqCst) > 0;
            if busy {
                d.touch();
                continue;
            }
            if d.idle_for() >= grace {
                log("no sessions and no clients — exiting");
                let _ = std::fs::remove_file(&sock);
                std::process::exit(0);
            }
        });
    }

    // ---------- connections ----------

    /// Decrements on every exit path, panics included: a leaked client count
    /// would keep the daemon alive forever.
    struct ClientGuard(Arc<Daemon>);

    impl Drop for ClientGuard {
        fn drop(&mut self) {
            self.0.clients.fetch_sub(1, Ordering::SeqCst);
            self.0.touch();
        }
    }

    #[derive(Deserialize)]
    struct Hello {
        hello: String,
        #[serde(default)]
        id: Option<u32>,
    }

    #[derive(Deserialize)]
    #[serde(tag = "op", rename_all = "kebab-case")]
    enum Request {
        Open {
            #[serde(default)]
            cwd: Option<String>,
            #[serde(default)]
            env: BTreeMap<String, String>,
            #[serde(default = "default_cols")]
            cols: u16,
            #[serde(default = "default_rows")]
            rows: u16,
        },
        List,
        Resize {
            id: u32,
            cols: u16,
            rows: u16,
        },
        Kill {
            id: u32,
        },
        Foreground {
            id: u32,
        },
    }

    fn default_cols() -> u16 {
        80
    }

    fn default_rows() -> u16 {
        24
    }

    fn serve(d: Arc<Daemon>, stream: UnixStream) {
        d.clients.fetch_add(1, Ordering::SeqCst);
        d.touch();
        let _guard = ClientGuard(d.clone());

        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(read_half);
        let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            return;
        }
        let _ = stream.set_read_timeout(None);

        let mut writer = stream;
        match serde_json::from_str::<Hello>(&line) {
            Ok(h) if h.hello == "control" => control(d, reader, writer),
            Ok(h) if h.hello == "attach" => attach(d, h.id, reader, writer),
            Ok(h) => {
                let _ = reply(
                    &mut writer,
                    &json!({"ok": false, "error": format!("unknown hello: {}", h.hello)}),
                );
            }
            Err(e) => {
                let _ = reply(
                    &mut writer,
                    &json!({"ok": false, "error": format!("bad hello: {e}")}),
                );
            }
        }
    }

    fn reply(w: &mut UnixStream, v: &Value) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(v).unwrap_or_else(|_| br#"{"ok":false}"#.to_vec());
        line.push(b'\n');
        w.write_all(&line)?;
        w.flush()
    }

    fn control(d: Arc<Daemon>, mut reader: BufReader<UnixStream>, mut writer: UnixStream) {
        let ack = json!({
            "ok": true,
            "hello": "control",
            "pid": std::process::id(),
            "sock": d.home.join(SOCK).display().to_string(),
        });
        if reply(&mut writer, &ack).is_err() {
            return;
        }
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => handle(&d, req),
                Err(e) => json!({"ok": false, "error": format!("bad request: {e}")}),
            };
            if reply(&mut writer, &response).is_err() {
                break;
            }
            d.touch();
        }
    }

    fn handle(d: &Arc<Daemon>, req: Request) -> Value {
        match req {
            // A zero-column PTY is not a thing; clamp rather than fail, so a
            // pane that has not been laid out yet still opens.
            Request::Open {
                cwd,
                env,
                cols,
                rows,
            } => match d.open(cwd.as_deref(), &env, cols.max(1), rows.max(1)) {
                Ok(id) => json!({"ok": true, "id": id}),
                Err(e) => json!({"ok": false, "error": e}),
            },
            Request::List => json!({"ok": true, "sessions": d.list()}),
            Request::Resize { id, cols, rows } => match d.get(id) {
                Some(s) => match s.resize(cols.max(1), rows.max(1)) {
                    Ok(()) => json!({"ok": true}),
                    Err(e) => json!({"ok": false, "error": e}),
                },
                None => no_session(id),
            },
            Request::Kill { id } => {
                if d.kill(id) {
                    json!({"ok": true})
                } else {
                    no_session(id)
                }
            }
            Request::Foreground { id } => match d.get(id) {
                Some(s) => {
                    let (at_prompt, program_running) = s.foreground();
                    json!({"ok": true, "at_prompt": at_prompt, "program_running": program_running})
                }
                None => no_session(id),
            },
        }
    }

    fn no_session(id: u32) -> Value {
        json!({"ok": false, "error": format!("no such session: {id}")})
    }

    /// Attach a data connection: ack, replay, live — and raw client bytes back
    /// into the PTY.
    fn attach(
        d: Arc<Daemon>,
        id: Option<u32>,
        mut reader: BufReader<UnixStream>,
        mut writer: UnixStream,
    ) {
        let Some(id) = id else {
            let _ = reply(
                &mut writer,
                &json!({"ok": false, "error": "attach needs an id"}),
            );
            return;
        };
        let Some(session) = d.get(id) else {
            let _ = reply(&mut writer, &no_session(id));
            return;
        };
        let (replay, backlog) = session.out.attach();
        let (cols, rows) = session.size();
        let ack = json!({
            "ok": true,
            "id": id,
            "cols": cols,
            "rows": rows,
            "replay_bytes": replay.len(),
        });
        if reply(&mut writer, &ack).is_err() {
            session.out.detach(&backlog);
            return;
        }

        // The writer half: the replay first, the live backlog after, and it is
        // the ONLY thing that writes to this socket once the ack is out. That is
        // what makes the seam an ordering property of a single thread instead of
        // a race between two.
        let Ok(mut out_socket) = writer.try_clone() else {
            session.out.detach(&backlog);
            return;
        };
        let feed = backlog.clone();
        let pump = thread::spawn(move || {
            if out_socket.write_all(&replay).is_ok() {
                while let Some(chunk) = feed.next_chunk() {
                    if out_socket.write_all(&chunk).is_err() {
                        break;
                    }
                }
            }
            // Whether the client left or the session ended, the socket is done:
            // shutting down BOTH halves is also what wakes the input pump below
            // when it was the session that ended.
            let _ = out_socket.shutdown(Shutdown::Both);
        });

        // Input: raw bytes from the client into the PTY. Read through the same
        // buffered reader the hello came from — a fresh one would drop whatever
        // it had already pulled in behind that newline.
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if session.write_input(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        // The client is gone. Note what does NOT happen here: nothing is
        // signalled to the child. Sessions outliving their clients is the reason
        // this daemon exists.
        session.out.detach(&backlog);
        let _ = writer.shutdown(Shutdown::Both);
        let _ = pump.join();
    }
}
