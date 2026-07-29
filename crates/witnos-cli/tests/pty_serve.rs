//! End-to-end tests of the real `witnos pty-serve` daemon: a temp
//! `$WITNOS_HOME`, the actual binary, real PTYs running the user's own `$SHELL`.
//!
//! The property the whole daemon exists for is `a_session_and_its_scrollback_
//! survive_the_client_going_away`. Everything else guards a way that property
//! could be true in a test and false in the app: ids that a restart reuses, a
//! socket a `kill -9` wedged, a daemon that exits while a shell is still live —
//! or one that never exits at all.
//!
//! Every test kills its own daemon on the way out (`Daemon`'s `Drop`), so a
//! failing assertion cannot leave one running.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Deliberately short names: `$WITNOS_HOME/pty.sock` has to fit in a
/// `sockaddr_un` (104 bytes on macOS) and the platform temp dir already eats
/// half of that.
fn temp_home(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "wt-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn deadline(secs: u64) -> Instant {
    Instant::now() + Duration::from_secs(secs)
}

/// Poll rather than sleep a guessed interval: shells claim the terminal, run
/// their rc files, and print on their own schedule.
fn poll_until(secs: u64, mut what: impl FnMut() -> bool) -> bool {
    let end = deadline(secs);
    while Instant::now() < end {
        if what() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    what()
}

// ---------- the daemon under test ----------

struct Daemon {
    home: PathBuf,
    child: Option<Child>,
}

impl Daemon {
    /// `idle_secs` is the daemon's own grace period before an empty one exits.
    /// Tests cannot wait out the 20s default, so they set their own.
    fn start(home: &Path, idle_secs: u64) -> Daemon {
        let child = Command::new(env!("CARGO_BIN_EXE_witnos"))
            .arg("pty-serve")
            .env("WITNOS_HOME", home)
            .env("WITNOS_PTY_IDLE_SECS", idle_secs.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let d = Daemon {
            home: home.to_path_buf(),
            child: Some(child),
        };
        assert!(
            poll_until(15, || Control::connect(&d.sock()).is_some()),
            "the daemon never came up. log:\n{}",
            d.log()
        );
        d
    }

    fn sock(&self) -> PathBuf {
        self.home.join("pty.sock")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.home.join("pty-serve.log")).unwrap_or_default()
    }

    fn control(&self) -> Control {
        Control::connect(&self.sock())
            .unwrap_or_else(|| panic!("cannot reach the daemon. log:\n{}", self.log()))
    }

    fn attach(&self, id: u32) -> Data {
        Data::attach(&self.sock(), id)
    }

    /// Has it exited on its own?
    fn exited(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => c.try_wait().unwrap().is_some(),
            None => true,
        }
    }

    /// `kill -9`, the way a crash would leave things: no cleanup runs.
    fn kill9(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Daemon {
    /// Kill first, THEN remove the home — a daemon whose socket and id file
    /// vanished underneath it would be a second failure on top of whatever the
    /// test was already failing at. Restart-in-place tests call `kill9` on their
    /// own and rely on this not touching the directory until the very end.
    fn drop(&mut self) {
        self.kill9();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

// ---------- control connection ----------

struct Control {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Control {
    fn connect(sock: &Path) -> Option<Control> {
        let s = UnixStream::connect(sock).ok()?;
        let mut c = Control {
            reader: BufReader::new(s.try_clone().ok()?),
            writer: s,
        };
        let ack = c.line(r#"{"hello":"control"}"#)?;
        (ack["ok"] == Value::Bool(true)).then_some(c)
    }

    fn line(&mut self, request: &str) -> Option<Value> {
        self.writer.write_all(request.as_bytes()).ok()?;
        self.writer.write_all(b"\n").ok()?;
        let mut line = String::new();
        self.reader.read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }

    fn req(&mut self, request: &str) -> Value {
        self.line(request)
            .unwrap_or_else(|| panic!("no reply to {request}"))
    }

    fn open(&mut self, cwd: &Path, env: &str) -> u32 {
        let r = self.req(&format!(
            r#"{{"op":"open","cwd":"{}","env":{env},"cols":80,"rows":24}}"#,
            cwd.display()
        ));
        assert_eq!(r["ok"], Value::Bool(true), "open failed: {r}");
        r["id"].as_u64().expect("open must return an id") as u32
    }

    fn sessions(&mut self) -> Vec<Value> {
        let r = self.req(r#"{"op":"list"}"#);
        assert_eq!(r["ok"], Value::Bool(true), "list failed: {r}");
        r["sessions"].as_array().cloned().unwrap_or_default()
    }

    /// (at_prompt, program_running) — both false is the daemon's refusal to
    /// guess, and callers on the app side treat it as such.
    fn foreground(&mut self, id: u32) -> (bool, bool) {
        let r = self.req(&format!(r#"{{"op":"foreground","id":{id}}}"#));
        assert_eq!(r["ok"], Value::Bool(true), "foreground failed: {r}");
        (
            r["at_prompt"] == Value::Bool(true),
            r["program_running"] == Value::Bool(true),
        )
    }
}

// ---------- data connection ----------

/// One attached client, collecting what the pane prints the way a terminal
/// front end would.
struct Data {
    ack: Value,
    writer: UnixStream,
    got: Arc<Mutex<Vec<u8>>>,
}

impl Data {
    fn attach(sock: &Path, id: u32) -> Data {
        let s = UnixStream::connect(sock).unwrap();
        // ONE buffered reader for the ack line AND the raw stream after it.
        // A second reader would silently drop whatever replay bytes the first
        // read pulled in behind that newline — the client rule the protocol
        // doc states, exercised here rather than merely documented.
        let mut reader = BufReader::new(s.try_clone().unwrap());
        (&s).write_all(format!("{{\"hello\":\"attach\",\"id\":{id}}}\n").as_bytes())
            .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let ack: Value = serde_json::from_str(&line).unwrap_or(Value::Null);

        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Data {
            ack,
            writer: s,
            got,
        }
    }

    fn type_in(&mut self, keys: &str) {
        self.writer.write_all(keys.as_bytes()).unwrap();
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.got.lock().unwrap()).into_owned()
    }

    fn wait_until(&self, what: impl Fn(&str) -> bool) -> bool {
        poll_until(15, || what(&self.text()))
    }

    fn wait_for(&self, needle: &str) -> bool {
        self.wait_until(|t| t.contains(needle))
    }
}

impl Drop for Data {
    /// The client goes away completely: shutting the socket down rather than
    /// just dropping it is what makes the collector thread's dup'd fd let go
    /// too, so the daemon really does see the client leave.
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

/// Pull the number out of `PREFIX<digits>-END`, skipping the terminal's echo of
/// the command itself (where `$$` is still unexpanded).
fn stamped(text: &str, prefix: &str) -> Option<u32> {
    text.split(prefix).skip(1).find_map(|part| {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || !part[digits.len()..].starts_with("-END") {
            return None;
        }
        digits.parse().ok()
    })
}

// ---------- the property the daemon exists for ----------

/// Drop the client connection entirely and the shell is still there, still the
/// same process, with everything it printed waiting to be replayed. This is the
/// whole change: an agent session survives the GUI being closed, so the goal
/// bound to it — and the nine verification items the human built up — is not
/// orphaned by quitting an app.
#[test]
fn a_session_and_its_scrollback_survive_the_client_going_away() {
    let home = temp_home("live");
    let daemon = Daemon::start(&home, 60);
    let mut control = daemon.control();
    let id = control.open(&home, "{}");

    let first_pid = {
        let mut client = daemon.attach(id);
        assert_eq!(client.ack["ok"], Value::Bool(true), "ack: {}", client.ack);
        client.type_in("echo SH-$$-END\r");
        assert!(
            client.wait_until(|t| stamped(t, "SH-").is_some()),
            "the shell never answered:\n{}",
            client.text()
        );
        // The quotes are load-bearing: they make the sentinel something only
        // the command's OUTPUT can spell. Counting occurrences instead would
        // prove nothing — a shell with a line editor echoes the typed line more
        // than once as it redraws its prompt.
        client.type_in("echo MARK-O''NE\r");
        assert!(
            client.wait_for("MARK-ONE"),
            "expected the shell to run the echo, got:\n{}",
            client.text()
        );
        stamped(&client.text(), "SH-").unwrap()
        // …and here the client connection is dropped completely.
    };

    // The daemon still has the session, and the child is still alive.
    let sessions = control.sessions();
    assert_eq!(sessions.len(), 1, "the session was lost: {sessions:?}");
    assert_eq!(sessions[0]["id"].as_u64(), Some(u64::from(id)));
    assert_eq!(
        sessions[0]["alive"],
        Value::Bool(true),
        "the child was signalled when its client left: {}",
        sessions[0]
    );
    assert!(
        poll_until(5, || control.sessions()[0]["clients"].as_u64() == Some(0)),
        "the daemon never noticed the client leaving"
    );

    // Reopening the app: the replay carries what came before, and the shell it
    // reaches is the very same process.
    let mut client = daemon.attach(id);
    assert!(
        client.ack["replay_bytes"].as_u64().unwrap_or(0) > 0,
        "nothing to replay: {}",
        client.ack
    );
    assert!(
        client.wait_for("MARK-ONE"),
        "the replay lost the earlier output:\n{}",
        client.text()
    );
    client.type_in("echo AGAIN-$$-END\r");
    assert!(
        client.wait_until(|t| stamped(t, "AGAIN-").is_some()),
        "the reattached pane is not responding:\n{}",
        client.text()
    );
    assert_eq!(
        stamped(&client.text(), "AGAIN-"),
        Some(first_pid),
        "a different shell answered — the original was not kept alive"
    );
}

/// The hot path's two claims at once: an attached client gets a burst whole
/// (raw bytes, no framing, no chunk the fan-out silently drops), and the replay
/// a later client is handed stays bounded by the ring no matter how much came
/// through it — an agent that prints a megabyte must not turn every reattach
/// into a megabyte.
#[test]
fn a_burst_of_output_arrives_whole_and_the_replay_stays_bounded() {
    const RING_BYTES: u64 = 256 * 1024;
    let home = temp_home("burst");
    let daemon = Daemon::start(&home, 60);
    let mut control = daemon.control();
    let id = control.open(&home, "{}");

    {
        let mut client = daemon.attach(id);
        // ~290 KB, comfortably more than the ring holds. The quotes in the
        // sentinel keep the shell's echo of the typed line from spelling it —
        // only the output can, which is what makes the wait mean "it ran".
        client.type_in("seq 1 50000; echo SEQ-DO''NE-END\r");
        assert!(
            client.wait_for("SEQ-DONE-END"),
            "the burst never finished:\n{}",
            &client.text()[..400.min(client.text().len())]
        );
        let text = client.text();
        assert!(
            text.contains("\r\n50000\r\n"),
            "the live stream lost the end of the burst. tail:\n{:?}",
            &text[text.len().saturating_sub(200)..]
        );
        assert!(
            text.contains("\r\n25000\r\n") && text.contains("\r\n1\r\n"),
            "the live stream lost the middle or the start of the burst"
        );
        assert!(
            text.len() as u64 > RING_BYTES,
            "this test is pointless unless more came through than the ring holds"
        );
    }

    let client = daemon.attach(id);
    let replayed = client.ack["replay_bytes"].as_u64().unwrap();
    assert!(
        replayed <= RING_BYTES,
        "the replay is unbounded: {replayed} bytes"
    );
    assert!(
        client.wait_for("SEQ-DONE-END"),
        "the replay must carry the tail, which is the part worth seeing:\n{}",
        client.text()
    );
    assert!(
        !client.text().contains("\r\n1\r\n"),
        "the ring kept the whole burst — it is supposed to be a tail"
    );
}

// ---------- ids ----------

/// `WITNOS_PANE` is how a goal names the terminal its agent lives in, so an id
/// handed out twice would aim a human's correction at somebody else's pane. The
/// allocator therefore lives in `$WITNOS_HOME`, not in a process.
#[test]
fn session_ids_are_monotonic_across_a_restart_and_never_reused() {
    let home = temp_home("ids");
    let mut first = Daemon::start(&home, 60);
    let a = first.control().open(&home, "{}");
    let b = first.control().open(&home, "{}");
    assert!(b > a, "ids must increase within a daemon: {a} then {b}");
    first.kill9();

    let second = Daemon::start(&home, 60);
    let c = second.control().open(&home, "{}");
    assert!(
        c > b,
        "a restarted daemon reused live ids: {b} then {c} — a correction would \
         land in the wrong pane"
    );
    // Persisted BEFORE the id is handed out, so a daemon killed mid-open loses
    // an id rather than repeating one.
    let ids: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("pty-ids.json")).unwrap()).unwrap();
    assert!(
        ids["next_id"].as_u64().unwrap() > u64::from(c),
        "the allocator was not advanced past the id it handed out: {ids}"
    );
}

// ---------- the safety signal ----------

/// The load-bearing signal for two app-side features with opposite needs:
/// typing `cd` needs a shell at its prompt, typing a correction to the agent
/// needs a program to be there. Checked against a real PTY, because job control
/// is what makes the answer exist at all.
#[test]
fn foreground_tells_an_idle_shell_from_one_running_a_program() {
    let home = temp_home("fg");
    let daemon = Daemon::start(&home, 60);
    let mut control = daemon.control();
    let id = control.open(&home, "{}");
    let mut client = daemon.attach(id);

    assert!(
        poll_until(15, || control.foreground(id) == (true, false)),
        "a fresh shell should read as at its prompt, got {:?}\n{}",
        control.foreground(id),
        client.text()
    );

    client.type_in("sleep 30\r");
    assert!(
        poll_until(15, || control.foreground(id) == (false, true)),
        "`sleep` should read as a program holding the pane, got {:?}\n{}",
        control.foreground(id),
        client.text()
    );

    client.type_in("\x03"); // ^C: back to the prompt
    assert!(
        poll_until(15, || control.foreground(id) == (true, false)),
        "the prompt should read as idle again, got {:?}\n{}",
        control.foreground(id),
        client.text()
    );

    // A pane that is gone answers neither way — and says so as an error, not as
    // a guess a caller could mistake for "idle".
    let r = control.req(r#"{"op":"foreground","id":99999}"#);
    assert_eq!(r["ok"], Value::Bool(false), "got: {r}");
}

// ---------- start-up discipline ----------

/// A `kill -9`'d daemon leaves its socket file behind. The next start must not
/// be wedged by it — the advisory lock, not the socket's existence, is what
/// answers "is one already running", so the winner of the lock is free to
/// unlink the corpse.
#[test]
fn a_stale_socket_from_a_killed_daemon_does_not_block_the_next_start() {
    let home = temp_home("stale");
    let mut first = Daemon::start(&home, 60);
    first.kill9();
    assert!(
        home.join("pty.sock").exists(),
        "this test is pointless unless a killed daemon really leaves its socket"
    );

    let second = Daemon::start(&home, 60);
    assert!(second.control().sessions().is_empty());

    // And while one IS running, a second start says so and exits cleanly rather
    // than stealing the socket — that is what makes "spawn it blindly, then
    // connect" safe for the app.
    let out = Command::new(env!("CARGO_BIN_EXE_witnos"))
        .arg("pty-serve")
        .env("WITNOS_HOME", &home)
        .output()
        .unwrap();
    assert!(out.status.success(), "a redundant start must not fail");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("already running"),
        "got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        second.control().sessions().is_empty(),
        "the redundant start disturbed the live daemon"
    );
}

// ---------- lifetime ----------

/// Both halves of the rule, in one test because they are one rule: an empty
/// daemon must go away so a machine does not collect them, and a daemon with a
/// live shell must not, no matter that every client has left.
#[test]
fn the_daemon_outlives_its_clients_and_exits_once_the_last_session_is_gone() {
    let home = temp_home("life");
    let mut daemon = Daemon::start(&home, 2);
    let id = {
        let mut control = daemon.control();
        control.open(&home, "{}")
        // …and every client is gone from here on.
    };

    // Well past the idle grace with nobody connected: the session is what keeps
    // it alive.
    thread::sleep(Duration::from_secs(5));
    assert!(
        !daemon.exited(),
        "the daemon exited while a shell was still live. log:\n{}",
        daemon.log()
    );
    let mut control = daemon.control();
    assert_eq!(
        control.sessions().len(),
        1,
        "the surviving daemon lost its session"
    );

    let r = control.req(&format!(r#"{{"op":"kill","id":{id}}}"#));
    assert_eq!(r["ok"], Value::Bool(true), "kill failed: {r}");
    assert!(control.sessions().is_empty());
    drop(control);

    assert!(
        poll_until(20, || daemon.exited()),
        "an empty daemon never exited. log:\n{}",
        daemon.log()
    );
    assert!(
        !home.join("pty.sock").exists(),
        "a daemon that exits on its own must take its socket with it"
    );
}

// ---------- the client's environment ----------

/// The daemon knows nothing about Witnos: the scope stamps arrive from the
/// client. `WITNOS_PANE` is the one that cannot, because only the daemon knows
/// the id it is about to allocate — hence the `{id}` placeholder, checked here
/// through the shell that actually receives it.
#[test]
fn the_clients_env_reaches_the_shell_with_the_session_id_stamped_in() {
    let home = temp_home("env");
    let daemon = Daemon::start(&home, 60);
    let mut control = daemon.control();
    let id = control.open(
        &home,
        r#"{"WITNOS_TERMINAL":"1","WITNOS_PANE":"{id}","WITNOS_MARK":"stamped"}"#,
    );
    let mut client = daemon.attach(id);
    client.type_in("echo PANE-$WITNOS_PANE-END T-$TERM-$WITNOS_TERMINAL-$WITNOS_MARK\r");
    assert!(
        client.wait_until(|t| stamped(t, "PANE-").is_some()),
        "the shell never answered:\n{}",
        client.text()
    );
    assert_eq!(
        stamped(&client.text(), "PANE-"),
        Some(id),
        "the pane stamp must name this session:\n{}",
        client.text()
    );
    assert!(
        client.wait_for("T-xterm-256color-1-stamped"),
        "TERM and the client's own variables must both reach the shell:\n{}",
        client.text()
    );
}

// ---------- protocol hygiene ----------

/// A bad line is an answer, not a hangup: the app keeps one control connection
/// for the whole run, so a typo in one request must not cost it every later one.
#[test]
fn a_refused_request_does_not_take_the_control_connection_down() {
    let home = temp_home("bad");
    let daemon = Daemon::start(&home, 60);
    let mut control = daemon.control();

    for bad in [
        r#"{"op":"fly"}"#,
        r#"{"op":"resize","id":4242,"cols":80,"rows":24}"#,
        r#"{"op":"kill","id":4242}"#,
        "not json at all",
    ] {
        let r = control.req(bad);
        assert_eq!(r["ok"], Value::Bool(false), "{bad} → {r}");
        assert!(r["error"].is_string(), "{bad} → {r}");
    }
    let id = control.open(&home, "{}");
    assert_eq!(control.sessions().len(), 1);

    // Attaching to a session that does not exist is refused the same way,
    // before any byte of the stream can be mistaken for protocol.
    let bad = daemon.attach(id + 1000);
    assert_eq!(bad.ack["ok"], Value::Bool(false), "ack: {}", bad.ack);

    // Resize is accepted and remembered, so a reattaching client is told the
    // size the pane actually has.
    let r = control.req(&format!(
        r#"{{"op":"resize","id":{id},"cols":120,"rows":40}}"#
    ));
    assert_eq!(r["ok"], Value::Bool(true), "resize failed: {r}");
    let client = daemon.attach(id);
    assert_eq!(
        client.ack["cols"].as_u64(),
        Some(120),
        "ack: {}",
        client.ack
    );
    assert_eq!(client.ack["rows"].as_u64(), Some(40), "ack: {}", client.ack);
}
