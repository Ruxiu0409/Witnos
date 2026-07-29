//! The unix backend: a client for the `witnos pty-serve` daemon, which owns the
//! shells so they outlive the app. One control connection carries the verbs, one
//! data connection per attached pane carries raw bytes both ways, and the daemon
//! is started on demand — it does not background itself, so starting it means
//! spawning it detached and then polling the socket.
//!
//! The protocol is specified in the daemon's own module header (`pty_serve`);
//! this file is a client of it and adds no dialect of its own.
//!
//! Nothing here knows about Tauri — pane traffic goes to a `Sink` — which is
//! what lets the socket plumbing be tested against a real daemon with no GUI
//! around it. What it does know is that the sessions on the other end outlive
//! this process: `detach` is the ordinary way to let go of a pane, and `kill` is
//! reserved for the human closing one.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{PaneEvent, PaneInfo, Sink};

/// Both live in `$WITNOS_HOME`: the daemon's protocol names the socket there,
/// and its log is where its own startup failures go — the ones a human left
/// without terminals needs to read.
const SOCK: &str = "pty.sock";
const LOG: &str = "pty-serve.log";

/// A control round trip is a local socket call answered by an in-memory lookup,
/// so anything slower than this means the daemon is wedged — and a wedged daemon
/// must cost the UI a refusal, never a freeze.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait for a daemon we just spawned to bind its socket: process
/// start plus a bind, not work.
const START_TIMEOUT: Duration = Duration::from_secs(10);
const START_POLL: Duration = Duration::from_millis(25);

pub struct Terminals {
    inner: Arc<Inner>,
}

struct Inner {
    home: PathBuf,
    sock: PathBuf,
    /// Where the daemon binary is. `None` = resolve it the way the app resolves
    /// the bundled CLI at run time; tests hand in a path instead of touching the
    /// process environment.
    cli: Option<PathBuf>,
    /// The one control connection the app is expected to keep for the whole run.
    /// Holding it is also what keeps an idle daemon from exiting between the
    /// window opening and the first pane.
    control: Mutex<Option<Control>>,
    panes: Mutex<HashMap<u32, Attachment>>,
    next_token: AtomicU64,
}

/// One attached pane's end of the data connection.
struct Attachment {
    /// Keystrokes go in here; shutting it down is what ends the pump thread.
    socket: UnixStream,
    /// Set before the socket is shut down on purpose, so the pump can tell "the
    /// app let go of this pane" from "the session ended".
    released: Arc<AtomicBool>,
    /// Which attachment this is. A pump whose token no longer matches the map
    /// has been replaced, and must not evict its successor on the way out.
    token: u64,
}

impl Attachment {
    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

impl Default for Terminals {
    fn default() -> Self {
        Terminals::new(crate::witnos_home(), None)
    }
}

impl Terminals {
    fn new(home: PathBuf, cli: Option<PathBuf>) -> Terminals {
        Terminals {
            inner: Arc::new(Inner {
                sock: home.join(SOCK),
                home,
                cli,
                control: Mutex::new(None),
                panes: Mutex::new(HashMap::new()),
                next_token: AtomicU64::new(1),
            }),
        }
    }

    /// The panes that survived the last run — asked WITHOUT starting a daemon.
    /// If none is listening then nothing survived, which is both the honest
    /// answer and the fast one: the app must not pay a daemon start-up before
    /// its window appears, and the answer would be the empty list anyway.
    ///
    /// `None` means the question could not be answered (a socket that is there
    /// but unusable). Whoever asks must then treat every pane as unknown rather
    /// than dead — "I don't know" and "nothing survived" are opposite claims,
    /// and only one of them is safe to act on.
    pub fn surviving(&self) -> Option<Vec<PaneInfo>> {
        let mut control = match Control::connect(&self.inner.sock) {
            Ok(c) => c,
            Err(ConnectError::NotRunning) => return Some(Vec::new()),
            Err(ConnectError::Failed(_)) => return None,
        };
        let panes = match control.round_trip(&json!({"op": "list"})) {
            Rpc::Reply(reply) => sessions_from(reply).ok()?,
            Rpc::NotSent(_) | Rpc::Unanswered(_) => return None,
        };
        // Keep it: the app is a client for the rest of the run.
        *self.inner.control.lock().unwrap() = Some(control);
        Some(panes)
    }

    /// Open a pane's shell, or adopt the one it already had (see `term_spawn`).
    pub fn ensure(
        &self,
        id: Option<u32>,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        env: &BTreeMap<String, String>,
    ) -> Result<u32, String> {
        if let Some(id) = id {
            // `resize` is both the existence check a restored pane needs — a
            // session that ended must not be silently swapped for a new one
            // under the same id — and the thing it needs anyway: the window it
            // is coming back into is rarely the size it left.
            self.answered(json!({"op": "resize", "id": id, "cols": cols, "rows": rows}))?;
            return Ok(id);
        }
        let reply = self.answered(json!({
            "op": "open",
            "cwd": cwd,
            "env": env,
            "cols": cols,
            "rows": rows,
        }))?;
        reply["id"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("the daemon opened a pane without an id: {reply}"))
    }

    /// Stream a pane here: the daemon replays its scrollback and then continues
    /// live, on one raw connection that carries keystrokes back the other way.
    /// Answers with the token that names this attachment, which is what lets a
    /// later `detach` say WHICH one it is letting go of.
    ///
    /// Attaching twice replaces the earlier attachment (a remount is exactly
    /// that), and the replaced one is stopped BEFORE the new connection asks for
    /// its replay — otherwise the same bytes could arrive down two sockets and
    /// the pane would render its own tail twice.
    pub fn attach(&self, id: u32, sink: Sink) -> Result<u64, String> {
        self.detach(id, None);
        let socket = UnixStream::connect(&self.inner.sock)
            .map_err(|e| format!("cannot reach the terminal daemon: {e}"))?;
        // The ack must not be able to pin a thread; the raw stream after it must
        // not have a deadline at all — a quiet agent is not a broken one.
        let _ = socket.set_read_timeout(Some(REPLY_TIMEOUT));
        let mut reader = BufReader::new(socket.try_clone().map_err(|e| e.to_string())?);
        let hello = format!("{}\n", json!({"hello": "attach", "id": id}));
        (&socket)
            .write_all(hello.as_bytes())
            .map_err(|e| format!("cannot attach to pane {id}: {e}"))?;
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("pane {id} never acknowledged the attach: {e}"))?;
        let ack: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        if ack["ok"] != Value::Bool(true) {
            return Err(error_text(&ack, "attach refused"));
        }
        let _ = socket.set_read_timeout(None);

        let released = Arc::new(AtomicBool::new(false));
        let token = self.inner.next_token.fetch_add(1, Ordering::SeqCst);
        let pump = Pump {
            inner: Arc::downgrade(&self.inner),
            id,
            token,
            released: released.clone(),
        };
        // Registered before the pump starts, so a session that ends instantly
        // cannot have its `forget` race this insert and leave an entry behind
        // that nothing will ever remove.
        self.inner.panes.lock().unwrap().insert(
            id,
            Attachment {
                socket,
                released,
                token,
            },
        );
        // The SAME buffered reader carries on into the raw stream: reading the
        // ack line has already pulled replay bytes in behind that newline, and a
        // fresh reader would drop them (the protocol's client rule).
        std::thread::spawn(move || pump.run(reader, sink));
        Ok(token)
    }

    /// Let go of a pane without touching its shell. This is the whole feature:
    /// quitting the app ends every attachment and not one session.
    ///
    /// `token` names the attachment being let go of, and a mismatch is a no-op.
    /// That is not paranoia: a view can be unmounted while its own attach is
    /// still in flight, so a late "I'm done with this" must not be able to cut
    /// off the view that has meanwhile taken the pane over. `None` means "release
    /// whatever is there" and belongs to `attach`, which is the one caller
    /// entitled to displace somebody.
    pub fn detach(&self, id: u32, token: Option<u64>) {
        let mut panes = self.inner.panes.lock().unwrap();
        if token.is_some_and(|t| panes.get(&id).is_some_and(|p| p.token != t)) {
            return;
        }
        if let Some(previous) = panes.remove(&id) {
            previous.release();
        }
    }

    pub fn list(&self) -> Result<Vec<PaneInfo>, String> {
        sessions_from(self.answered(json!({"op": "list"}))?)
    }

    pub fn write(&self, id: u32, bytes: &[u8]) -> Result<(), String> {
        let panes = self.inner.panes.lock().unwrap();
        let pane = panes.get(&id).ok_or("no such terminal")?;
        (&pane.socket).write_all(bytes).map_err(|e| e.to_string())
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        self.answered(json!({"op": "resize", "id": id, "cols": cols, "rows": rows}))?;
        Ok(())
    }

    /// End a pane for good. The pane's own listeners are closed by the daemon, so
    /// the pump ends on its own and reports the session finished — there is
    /// nothing to release here.
    pub fn kill(&self, id: u32) {
        let _ = self.answered(json!({"op": "kill", "id": id}));
    }

    /// `(at_prompt, program_running)`, or `None` when there is no such pane.
    ///
    /// Both false is the PTY refusing to answer, and it is passed through
    /// unchanged: the two callers depend on OPPOSITE answers, so collapsing this
    /// into one boolean either way would type into a pane that must be left
    /// alone. A refusal from the daemon (there is only one — no such session)
    /// becomes `None`, which both callers also treat as "do not type".
    pub fn foreground(&self, id: u32) -> Result<Option<(bool, bool)>, String> {
        let reply = self.request(json!({"op": "foreground", "id": id}))?;
        if reply["ok"] != Value::Bool(true) {
            return Ok(None);
        }
        Ok(Some((
            reply["at_prompt"] == Value::Bool(true),
            reply["program_running"] == Value::Bool(true),
        )))
    }

    /// One control verb, with the daemon started if it is not there. The
    /// connection is kept for the whole run, and a broken one is replaced: a
    /// daemon that was killed between two clicks should heal rather than turn
    /// into an error the human has to interpret.
    ///
    /// The retry is deliberately only for a request that never reached the
    /// daemon. One that WAS sent and not answered is reported as it happened —
    /// resending `open` because a reply went missing would leave a shell running
    /// that nobody is attached to and nobody knows about.
    fn request(&self, req: Value) -> Result<Value, String> {
        let mut guard = self.inner.control.lock().unwrap();
        if let Some(control) = guard.as_mut() {
            match control.round_trip(&req) {
                Rpc::Reply(reply) => return Ok(reply),
                Rpc::Unanswered(e) => {
                    *guard = None;
                    return Err(e);
                }
                // Never left the app: safe to open a fresh connection and retry.
                Rpc::NotSent(_) => *guard = None,
            }
        }
        let mut control = self.inner.connect_or_start()?;
        match control.round_trip(&req) {
            Rpc::Reply(reply) => {
                *guard = Some(control);
                Ok(reply)
            }
            Rpc::NotSent(e) | Rpc::Unanswered(e) => Err(e),
        }
    }

    /// The same, for verbs whose refusal is nothing but a failure: `{"ok":false}`
    /// becomes the error it reads as.
    fn answered(&self, req: Value) -> Result<Value, String> {
        let reply = self.request(req)?;
        if reply["ok"] != Value::Bool(true) {
            return Err(error_text(&reply, "the terminal daemon refused"));
        }
        Ok(reply)
    }
}

impl Drop for Inner {
    /// Let go of every pane on the way out, so no pump thread outlives the
    /// client it reports to and the daemon stops counting a client that is gone.
    /// Reachable at all only because the pumps hold a `Weak` — an `Arc` there
    /// would keep this from ever running, and the release below is what ends
    /// them.
    fn drop(&mut self) {
        for (_, pane) in self.panes.lock().unwrap().drain() {
            pane.release();
        }
    }
}

impl Inner {
    fn connect_or_start(&self) -> Result<Control, String> {
        match Control::connect(&self.sock) {
            Ok(c) => return Ok(c),
            Err(ConnectError::NotRunning) => {}
            Err(ConnectError::Failed(e)) => return Err(e),
        }
        self.start_daemon()?;
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            std::thread::sleep(START_POLL);
            match Control::connect(&self.sock) {
                Ok(c) => return Ok(c),
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "the terminal daemon did not come up within {}s ({}) — see {}",
                            START_TIMEOUT.as_secs(),
                            match e {
                                ConnectError::NotRunning => "nothing is listening".to_string(),
                                ConnectError::Failed(e) => e,
                            },
                            self.home.join(LOG).display()
                        ));
                    }
                }
            }
        }
    }

    /// Start the daemon and let go of it. Detached twice over: it `setsid`s
    /// itself, so a signal aimed at the app's process group cannot take the
    /// shells down with the app, and nothing here waits for it to be ready —
    /// `connect_or_start` polls the socket, because it does not background
    /// itself and there is nothing else to wait on.
    fn start_daemon(&self) -> Result<(), String> {
        let cli = match &self.cli {
            Some(p) => p.clone(),
            None => crate::bundled_cli()
                .ok_or("witnos CLI not found — reinstall the app (or set WITNOS_CLI_BIN in dev)")?,
        };
        let _ = std::fs::create_dir_all(&self.home);
        // The daemon points its own stderr at this log once it is up. The
        // failures it can hit BEFORE that (a home it cannot create, a socket
        // path too long for `sockaddr_un`) would otherwise be lost, and they are
        // exactly the ones worth reading.
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.home.join(LOG))
            .map(Stdio::from)
            .unwrap_or_else(|_| Stdio::null());
        let child = Command::new(&cli)
            .arg("pty-serve")
            // Explicit rather than inherited: the daemon must serve the same home
            // this client computed, or they would be talking about two sockets.
            .env("WITNOS_HOME", &self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .map_err(|e| format!("cannot start {} pty-serve: {e}", cli.display()))?;
        // However detached it is, it stays our child until it exits, so somebody
        // has to collect it — otherwise a daemon that idle-exits while the app
        // runs becomes a zombie in the app's own process table.
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        Ok(())
    }

    /// Drop an attachment that has ended, unless it has already been replaced.
    fn forget(&self, id: u32, token: u64) {
        let mut panes = self.panes.lock().unwrap();
        if panes.get(&id).is_some_and(|p| p.token == token) {
            panes.remove(&id);
        }
    }
}

/// The reading half of one attachment.
struct Pump {
    inner: Weak<Inner>,
    id: u32,
    token: u64,
    released: Arc<AtomicBool>,
}

impl Pump {
    fn run(self, mut reader: BufReader<UnixStream>, sink: Sink) {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink(self.id, PaneEvent::Output(buf[..n].to_vec())),
            }
        }
        // End of stream is two very different events, and only this flag tells
        // them apart: the app let go of the pane (a remount, a window closing —
        // the shell is still there and must NOT be reported as finished), or the
        // session itself ended.
        if self.released.load(Ordering::SeqCst) {
            return;
        }
        if let Some(inner) = self.inner.upgrade() {
            inner.forget(self.id, self.token);
        }
        sink(self.id, PaneEvent::Ended);
    }
}

// ---------- the control connection ----------

enum ConnectError {
    /// Nothing is listening: there is no daemon. The ordinary answer on a first
    /// launch, and not a failure.
    NotRunning,
    Failed(String),
}

/// What came back from one request — split by whether the request could still
/// have taken effect, because that is what decides whether retrying is safe.
enum Rpc {
    Reply(Value),
    NotSent(String),
    Unanswered(String),
}

struct Control {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Control {
    fn connect(sock: &Path) -> Result<Control, ConnectError> {
        let stream = UnixStream::connect(sock).map_err(|e| match e.kind() {
            ErrorKind::NotFound | ErrorKind::ConnectionRefused => ConnectError::NotRunning,
            _ => ConnectError::Failed(format!("cannot reach {}: {e}", sock.display())),
        })?;
        let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
        let reader = stream
            .try_clone()
            .map_err(|e| ConnectError::Failed(e.to_string()))?;
        let mut control = Control {
            reader: BufReader::new(reader),
            writer: stream,
        };
        match control.round_trip(&json!({"hello": "control"})) {
            Rpc::Reply(ack) if ack["ok"] == Value::Bool(true) => Ok(control),
            Rpc::Reply(ack) => Err(ConnectError::Failed(error_text(
                &ack,
                "the terminal daemon refused a control connection",
            ))),
            Rpc::NotSent(e) | Rpc::Unanswered(e) => Err(ConnectError::Failed(e)),
        }
    }

    fn round_trip(&mut self, req: &Value) -> Rpc {
        let mut line = req.to_string();
        line.push('\n');
        if let Err(e) = self
            .writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.flush())
        {
            return Rpc::NotSent(format!("cannot reach the terminal daemon: {e}"));
        }
        let mut reply = String::new();
        match self.reader.read_line(&mut reply) {
            // A daemon that hung up mid-request answered nothing, but the
            // request may well have landed — so this is not retryable either.
            Ok(0) => Rpc::Unanswered("the terminal daemon closed the connection".to_string()),
            Ok(_) => match serde_json::from_str(&reply) {
                Ok(v) => Rpc::Reply(v),
                Err(e) => Rpc::Unanswered(format!("the terminal daemon answered nonsense: {e}")),
            },
            Err(e) => Rpc::Unanswered(format!("the terminal daemon did not answer: {e}")),
        }
    }
}

fn sessions_from(reply: Value) -> Result<Vec<PaneInfo>, String> {
    serde_json::from_value(reply["sessions"].clone())
        .map_err(|e| format!("cannot read the pane list: {e}"))
}

fn error_text(reply: &Value, fallback: &str) -> String {
    reply["error"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    //! The client against the real daemon: the actual `witnos` binary, a temp
    //! `$WITNOS_HOME`, real PTYs running the user's own `$SHELL`. What is checked
    //! here is what the app now depends on the socket for and cannot check in
    //! process: that a pane outlives the client, that the two opposite guards
    //! still read the pane correctly through the `foreground` verb, and that the
    //! scope stamps survive the trip.
    //!
    //! Every test kills its own daemon on the way out (`Home`'s `Drop`), so a
    //! failing assertion cannot leave one running.

    use super::super::{pane_env, prompt_pane, try_cd};
    use super::*;
    use std::sync::atomic::AtomicU32;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// The daemon binary these tests drive. `cargo test --workspace` builds it
    /// (it is the CLI crate's bin); running this package alone needs a
    /// `cargo build -p witnos-cli` first, and says so rather than skipping —
    /// a test that quietly passes without a daemon would be worse than red.
    fn witnos_bin() -> PathBuf {
        let exe = std::env::current_exe().expect("the test binary has a path");
        // target/<profile>/deps/<test binary> → target/<profile>/witnos
        let bin = exe
            .parent()
            .and_then(Path::parent)
            .expect("the test binary lives under target/<profile>/deps")
            .join("witnos");
        assert!(
            bin.is_file(),
            "these tests drive the real daemon and need {} — run `cargo test --workspace`, \
             or `cargo build -p witnos-cli` first",
            bin.display()
        );
        bin
    }

    /// A throwaway `$WITNOS_HOME`, cleaned up with its daemon. Deliberately short
    /// names: `$WITNOS_HOME/pty.sock` has to fit in a `sockaddr_un` (104 bytes on
    /// macOS) and the platform temp dir already eats half of that.
    struct Home(PathBuf);

    impl Home {
        fn new(tag: &str) -> Home {
            let dir = std::env::temp_dir().join(format!(
                "wa-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Home(dir)
        }

        fn app(&self) -> Terminals {
            Terminals::new(self.0.clone(), Some(witnos_bin()))
        }

        fn as_str(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }

        fn log(&self) -> String {
            std::fs::read_to_string(self.0.join(LOG)).unwrap_or_default()
        }
    }

    impl Drop for Home {
        /// Kill the daemon first, THEN remove the directory: one whose socket and
        /// id file vanished underneath it would be a second failure on top of
        /// whatever the test was already failing at. The shells go with it — the
        /// PTY masters close, so every pane gets its SIGHUP.
        fn drop(&mut self) {
            if let Some(pid) = std::fs::read_to_string(self.0.join("pty.lock"))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
            {
                let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A pane's output, collected the way the webview would receive it.
    #[derive(Clone, Default)]
    struct Screen(Arc<Mutex<Vec<u8>>>);

    impl Screen {
        fn sink(&self) -> Sink {
            let bytes = self.0.clone();
            Box::new(move |_id, event| {
                if let PaneEvent::Output(chunk) = event {
                    bytes.lock().unwrap().extend_from_slice(&chunk);
                }
            })
        }

        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }

        fn wait_for(&self, needle: &str) -> bool {
            self.wait_until(|t| t.contains(needle))
        }

        fn wait_until(&self, what: impl Fn(&str) -> bool) -> bool {
            poll_until(15, || what(&self.text()))
        }
    }

    /// Poll rather than sleep a guessed interval: shells claim the terminal, run
    /// their rc files, and print on their own schedule.
    fn poll_until(secs: u64, mut what: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if what() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        what()
    }

    /// Pull the number out of `PREFIX<digits>-END`, skipping the terminal's echo
    /// of the command itself (where `$$` is still unexpanded).
    fn stamped(text: &str, prefix: &str) -> Option<u32> {
        text.split(prefix).skip(1).find_map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() || !part[digits.len()..].starts_with("-END") {
                return None;
            }
            digits.parse().ok()
        })
    }

    /// The property the whole change exists for, from the app's side: the app
    /// lets go of a pane and goes away, a new one asks what survived, rebuilds
    /// that pane under the SAME id — the id a goal's session binding points at —
    /// and finds the same shell process with its scrollback replayed. Then the
    /// one thing that does end a pane: closing it.
    #[test]
    fn a_pane_outlives_the_app_and_comes_back_under_its_own_id() {
        let home = Home::new("live");
        let first_pid;
        let id;
        {
            let app = home.app();
            assert!(
                app.surviving().is_some_and(|panes| panes.is_empty()),
                "a home with no daemon has no surviving panes, and asking must not start one"
            );
            id = app
                .ensure(None, 80, 24, Some(&home.as_str()), &pane_env())
                .unwrap_or_else(|e| panic!("open failed: {e}\nlog:\n{}", home.log()));
            let screen = Screen::default();
            let token = app.attach(id, screen.sink()).unwrap();
            app.write(id, b"echo SH-$$-END\r").unwrap();
            assert!(
                screen.wait_until(|t| stamped(t, "SH-").is_some()),
                "the shell never answered:\n{}",
                screen.text()
            );
            // The quotes are load-bearing: they make the sentinel something only
            // the command's OUTPUT can spell, so finding it proves the shell ran
            // the line rather than merely echoed our keystrokes.
            app.write(id, b"echo MARK-O''NE\r").unwrap();
            assert!(screen.wait_for("MARK-ONE"), "got:\n{}", screen.text());
            first_pid = stamped(&screen.text(), "SH-").unwrap();
            // Unmounting the view, then the app going away.
            app.detach(id, Some(token));
        }

        let app = home.app();
        let panes = app.surviving().expect("the daemon is still there");
        assert_eq!(panes.len(), 1, "the pane was lost: {panes:?}");
        assert_eq!(panes[0].id, id, "a restored pane must keep its own id");
        assert!(panes[0].alive, "the shell was signalled when its app quit");

        let screen = Screen::default();
        let token = app.attach(id, screen.sink()).unwrap();
        assert!(
            screen.wait_for("MARK-ONE"),
            "the replay lost what the pane had printed:\n{}",
            screen.text()
        );
        app.write(id, b"echo AGAIN-$$-END\r").unwrap();
        assert!(
            screen.wait_until(|t| stamped(t, "AGAIN-").is_some()),
            "the restored pane is not responding:\n{}",
            screen.text()
        );
        assert_eq!(
            stamped(&screen.text(), "AGAIN-"),
            Some(first_pid),
            "a different shell answered — the original was not kept alive"
        );

        // A view that has been replaced can still be on its way out, and its late
        // "I'm done with this" must not cut off the one that took the pane over.
        app.detach(id, Some(token - 1));
        app.write(id, b"echo STILL-A''TTACHED\r")
            .expect("a stale detach must not have taken the attachment away");
        assert!(
            screen.wait_for("STILL-ATTACHED"),
            "a stale detach cut off the live attachment:\n{}",
            screen.text()
        );

        // …and the one path that does end a pane.
        app.kill(id);
        assert!(
            poll_until(5, || app.list().map(|l| l.is_empty()).unwrap_or(false)),
            "closing a pane must actually end it: {:?}",
            app.list()
        );
    }

    /// The two guards, through the socket, against a real shell: `cd` is typed
    /// only into a pane at its prompt, a correction only into a pane a program is
    /// holding, and a pane that no longer exists gets neither — the daemon
    /// refusing to answer must read as "do not type", never as "idle".
    #[test]
    fn the_two_guards_still_point_in_opposite_directions() {
        let home = Home::new("guard");
        let app = home.app();
        let start = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let shown = start.to_string_lossy().into_owned();
        let id = app
            .ensure(None, 80, 24, Some(&shown), &pane_env())
            .unwrap_or_else(|e| panic!("open failed: {e}\nlog:\n{}", home.log()));
        let screen = Screen::default();
        app.attach(id, screen.sink()).unwrap();
        assert!(
            poll_until(15, || app.foreground(id).unwrap() == Some((true, false))),
            "a fresh shell should read as at its prompt, got {:?}\n{}",
            app.foreground(id),
            screen.text()
        );

        // At a bare prompt: prose is refused (zsh would try to run it), a cd is
        // typed — and the half-typed line the human left does not ride along.
        assert_eq!(
            prompt_pane(&app, id, "the palette is still NOT-A-COMMAND").unwrap(),
            Some(false)
        );
        app.write(id, b"echo MARKER").unwrap();
        let target = start.join("witnos-daemon-cd (1)");
        std::fs::create_dir_all(&target).unwrap();
        let target = std::fs::canonicalize(&target).unwrap();
        let target = target.to_string_lossy().into_owned();
        assert!(try_cd(&app, id, &target).unwrap());
        app.write(id, b"pwd\r").unwrap();
        assert!(
            screen.wait_for(&format!("{target}\r\n")),
            "expected pwd to print {target}, got:\n{}",
            screen.text()
        );
        assert!(
            !screen.text().contains("NOT-A-COMMAND"),
            "prose reached a bare shell, which would run it:\n{}",
            screen.text()
        );
        assert!(
            !screen.text().contains("MARKERcd"),
            "the half-typed line was concatenated into the cd:\n{}",
            screen.text()
        );
        let _ = std::fs::remove_dir_all(&target);

        // With a program holding the pane the guards swap: `cat` stands in for
        // the agent, and it is the correction that goes through now.
        app.write(id, b"cat\r").unwrap();
        assert!(
            poll_until(15, || app.foreground(id).unwrap() == Some((false, true))),
            "`cat` should read as a program holding the pane, got {:?}\n{}",
            app.foreground(id),
            screen.text()
        );
        assert!(!try_cd(&app, id, "/usr").unwrap());
        assert_eq!(
            prompt_pane(&app, id, "the-contract-moved").unwrap(),
            Some(true)
        );
        // Twice on screen is the proof of delivery: once as the tty's echo of the
        // keystrokes, once written back out by the program that read them.
        assert!(
            screen.wait_until(|t| t.matches("the-contract-moved").count() >= 2),
            "the line should have reached the program itself:\n{}",
            screen.text()
        );
        assert!(
            !screen.text().contains("cd '/usr'"),
            "a cd reached the running program:\n{}",
            screen.text()
        );

        // A pane that is gone answers neither way, and both callers refuse.
        app.kill(id);
        assert!(poll_until(5, || app.foreground(id).unwrap().is_none()));
        assert!(try_cd(&app, id, &shown).is_err());
        assert_eq!(prompt_pane(&app, id, "anyone there").unwrap(), None);
    }

    /// The stamps the hooks read: the app sends them, the daemon fills in the one
    /// only it can know, and the shell that comes up carries both. Without this
    /// the session would be invisible to Witnos and a correction would have no
    /// address to go to.
    #[test]
    fn the_scope_stamps_reach_the_shell_with_its_own_pane_id() {
        let home = Home::new("stamp");
        let app = home.app();
        let id = app
            .ensure(None, 80, 24, Some(&home.as_str()), &pane_env())
            .unwrap_or_else(|e| panic!("open failed: {e}\nlog:\n{}", home.log()));
        let screen = Screen::default();
        app.attach(id, screen.sink()).unwrap();
        app.write(id, b"echo PANE-$WITNOS_PANE-END W-$WITNOS_TERMINAL-$TERM\r")
            .unwrap();
        assert!(
            screen.wait_until(|t| stamped(t, "PANE-").is_some()),
            "the shell never answered:\n{}",
            screen.text()
        );
        assert_eq!(
            stamped(&screen.text(), "PANE-"),
            Some(id),
            "the pane stamp must name this session:\n{}",
            screen.text()
        );
        assert!(
            screen.wait_for("W-1-xterm-256color"),
            "the scope stamp and TERM must both reach the shell:\n{}",
            screen.text()
        );
        app.kill(id);
    }
}
