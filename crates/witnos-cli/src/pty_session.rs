//! One detached PTY session: the shell, a bounded tail of what it printed, and
//! the fan-out to whichever clients happen to be attached right now.
//!
//! Everything here is deliberately client-agnostic. The daemon spawns the
//! user's `$SHELL` the same shape the app used to in-process — login shell,
//! `TERM`, the caller's cwd — and adds not one Witnos-specific variable of its
//! own: the scope stamps are the client's business and arrive on `open`.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

/// Recent output kept per session and replayed to a client on attach.
///
/// 256 KiB. A dense 80x24 screenful is under 2 KiB, so this is on the order of
/// a hundred screens — enough that reopening the app shows what the agent was
/// doing rather than a blank pane, while ten panes still cost only ~2.5 MB
/// resident. Going bigger buys little: what the human needs on attach is the
/// tail, and the full history of a run is the agent transcript's job, not a
/// terminal's.
const RING_BYTES: usize = 256 * 1024;

/// How much undelivered output one attached client may pile up before the
/// daemon disconnects it. A stalled reader must never stall the PTY itself
/// (that would freeze the agent for every other viewer) and must never grow the
/// daemon without bound, so the third option is taken: drop the client. That is
/// the recoverable one — reconnecting replays the ring.
const CLIENT_BACKLOG_BYTES: usize = 4 * 1024 * 1024;

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/bash".into()
            }
        })
}

fn resolve_cwd(cwd: Option<&str>) -> String {
    cwd.filter(|d| !d.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".into())
}

/// The shell a pane runs. Kept the same shape the app used in-process because
/// two properties depend on it: the login flag (a GUI-launched app otherwise
/// has no user PATH, so no agent CLIs) and its being interactive — job control
/// is what makes `Session::foreground` able to tell a waiting prompt from a
/// program running in it.
///
/// The only variable the daemon sets on its own is `TERM`, and it is set first
/// so a client that wants another one can simply pass it. Everything Witnos
/// needs (`WITNOS_TERMINAL`, `WITNOS_PANE`, a PATH carrying the bundled CLI)
/// comes in through `env`, from the half that knows about Witnos.
///
/// `{id}` inside an env VALUE is replaced with the session id. The client needs
/// that: `WITNOS_PANE` has to name the pane, only the daemon knows the id it is
/// about to allocate, and an environment is fixed at spawn — there is no
/// "set it afterwards". A placeholder keeps the daemon from having to know what
/// `WITNOS_PANE` means.
fn shell_command(cwd: &str, env: &BTreeMap<String, String>, id: u32) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(default_shell());
    cmd.arg("-l");
    cmd.env("TERM", "xterm-256color");
    for (k, v) in env {
        cmd.env(k, v.replace("{id}", &id.to_string()));
    }
    cmd.cwd(cwd);
    cmd
}

/// The bounded tail of a session's output.
struct Ring {
    buf: VecDeque<u8>,
    /// Has anything been evicted yet? Only then does a replay need realigning
    /// to a line boundary — see `snapshot`.
    truncated: bool,
}

impl Ring {
    fn new() -> Self {
        Ring {
            buf: VecDeque::new(),
            truncated: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend(bytes);
        let over = self.buf.len().saturating_sub(RING_BYTES);
        if over > 0 {
            self.buf.drain(..over);
            self.truncated = true;
        }
    }

    /// The replay handed to a newly attached client.
    ///
    /// Once the ring has evicted anything its first byte is wherever the
    /// eviction happened to land — possibly the middle of an escape sequence or
    /// of a multibyte character, which a terminal emulator would render as
    /// garbage or, worse, as a mode change nobody asked for. So a truncated
    /// replay starts just after its first newline: `\n` (0x0A) cannot appear
    /// inside a CSI/DCS sequence (whose parameter, intermediate and final bytes
    /// are all >= 0x20) and cannot appear inside a UTF-8 multibyte character
    /// (whose bytes are all >= 0x80), which makes a newline a guaranteed-safe
    /// resume point. It costs at most one partial line.
    fn snapshot(&self) -> Vec<u8> {
        let (a, b) = self.buf.as_slices();
        let mut v = Vec::with_capacity(a.len() + b.len());
        v.extend_from_slice(a);
        v.extend_from_slice(b);
        if self.truncated {
            if let Some(i) = v.iter().position(|&c| c == b'\n') {
                v.drain(..=i);
            }
        }
        v
    }
}

/// One attached client's outbound backlog. The PTY reader never blocks on a
/// socket: it appends here and moves on, and a thread per client drains this
/// into the socket.
pub struct Backlog {
    state: Mutex<BacklogState>,
    ready: Condvar,
}

struct BacklogState {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
    closed: bool,
}

impl Backlog {
    fn new() -> Self {
        Backlog {
            state: Mutex::new(BacklogState {
                chunks: VecDeque::new(),
                bytes: 0,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    /// `false` means this client is finished with — closed, or past its cap —
    /// and the caller should stop fanning out to it.
    fn push(&self, bytes: &[u8]) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.closed {
            return false;
        }
        if s.bytes + bytes.len() > CLIENT_BACKLOG_BYTES {
            s.closed = true;
            self.ready.notify_all();
            return false;
        }
        s.bytes += bytes.len();
        s.chunks.push_back(bytes.to_vec());
        self.ready.notify_all();
        true
    }

    /// Block until there is something to write. `None` means closed AND
    /// drained: a client whose session just ended still gets its last bytes
    /// before the socket is shut down.
    pub fn next_chunk(&self) -> Option<Vec<u8>> {
        let mut s = self.state.lock().unwrap();
        loop {
            if let Some(c) = s.chunks.pop_front() {
                s.bytes -= c.len();
                return Some(c);
            }
            if s.closed {
                return None;
            }
            s = self.ready.wait(s).unwrap();
        }
    }

    fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.closed = true;
        self.ready.notify_all();
    }
}

/// A session's output side: the ring and everyone currently listening.
///
/// One mutex covers both, and that is the whole reason the replay/live seam
/// cannot tear: `attach` takes the snapshot and registers the new listener
/// under the same lock that `push` must hold to append. So every byte is either
/// already in the snapshot or still to come through the backlog — never both,
/// never neither, and never out of order, because one thread writes the replay
/// and then that same thread drains that backlog. No timing involved.
pub struct Out {
    state: Mutex<OutState>,
}

struct OutState {
    ring: Ring,
    listeners: Vec<Arc<Backlog>>,
}

impl Out {
    fn new() -> Self {
        Out {
            state: Mutex::new(OutState {
                ring: Ring::new(),
                listeners: Vec::new(),
            }),
        }
    }

    pub fn push(&self, bytes: &[u8]) {
        let mut s = self.state.lock().unwrap();
        s.ring.push(bytes);
        s.listeners.retain(|b| b.push(bytes));
    }

    /// Snapshot + register, atomically. See the type's doc comment.
    pub fn attach(&self) -> (Vec<u8>, Arc<Backlog>) {
        let mut s = self.state.lock().unwrap();
        let replay = s.ring.snapshot();
        let backlog = Arc::new(Backlog::new());
        s.listeners.push(backlog.clone());
        (replay, backlog)
    }

    /// This client is gone. Note what this does NOT do: touch the child.
    pub fn detach(&self, backlog: &Arc<Backlog>) {
        backlog.close();
        self.state
            .lock()
            .unwrap()
            .listeners
            .retain(|b| !Arc::ptr_eq(b, backlog));
    }

    /// The session ended: everyone still listening gets their remaining bytes
    /// and then end-of-stream.
    pub fn close_all(&self) {
        let mut s = self.state.lock().unwrap();
        for b in s.listeners.drain(..) {
            b.close();
        }
    }

    pub fn listeners(&self) -> usize {
        self.state.lock().unwrap().listeners.len()
    }
}

pub struct Session {
    pub id: u32,
    pub cwd: String,
    size: Mutex<(u16, u16)>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Held behind its own `Arc` on purpose: the output pump thread keeps this
    /// and NOT the session, so dropping the session (closing the PTY master,
    /// the child, the writer) does not wait on a thread blocked in `read`.
    pub out: Arc<Out>,
}

impl Session {
    /// Allocate a PTY, spawn the shell in it, and hand back the session plus
    /// the master's read side for the caller to pump.
    pub fn open(
        id: u32,
        cwd: Option<&str>,
        env: &BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> Result<(Arc<Session>, Box<dyn Read + Send>), String> {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|e| e.to_string())?;
        let dir = resolve_cwd(cwd);
        let child = pair
            .slave
            .spawn_command(shell_command(&dir, env, id))
            .map_err(|e| e.to_string())?;
        // The daemon must not hold a slave handle: the output pump's EOF is how
        // a finished session is noticed, and that only arrives once every slave
        // fd is closed.
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let input = pair.master.take_writer().map_err(|e| e.to_string())?;
        let killer = child.clone_killer();
        let session = Arc::new(Session {
            id,
            cwd: dir,
            size: Mutex::new((cols, rows)),
            master: Mutex::new(pair.master),
            input: Mutex::new(input),
            child: Mutex::new(child),
            killer: Mutex::new(killer),
            out: Arc::new(Out::new()),
        });
        Ok((session, reader))
    }

    pub fn write_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut w = self.input.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.master
            .lock()
            .unwrap()
            .resize(size(cols, rows))
            .map_err(|e| e.to_string())?;
        *self.size.lock().unwrap() = (cols, rows);
        Ok(())
    }

    pub fn size(&self) -> (u16, u16) {
        *self.size.lock().unwrap()
    }

    /// Is the shell still running? Dead sessions are dropped as soon as their
    /// output pump sees EOF, so a live session normally answers yes; the honest
    /// `false` is the window where the shell has exited but buffered output is
    /// still draining, which is exactly when a client wants to be told.
    pub fn alive(&self) -> bool {
        match self.child.try_lock() {
            Ok(mut c) => matches!(c.try_wait(), Ok(None)),
            // The only long hold on this lock is the reap of a child whose PTY
            // already went to EOF, so contention means "on its way out".
            Err(_) => false,
        }
    }

    /// Two questions about the pane, answered as `(at_prompt, program_running)`
    /// — copied wholesale from the in-process implementation, because two
    /// features depend on OPPOSITE answers: typing a `cd` needs the shell to be
    /// at its prompt (only a shell runs a command), and typing a correction to
    /// the agent needs a program to be there (prose handed to zsh would be
    /// executed as a command).
    ///
    /// Deliberately not each other's negation: when the PTY answers neither way
    /// — no foreground process group, no child pid — BOTH are false, so both
    /// callers refuse rather than guess. The child lock is taken with `try_lock`
    /// for the same reason: not knowing must cost a refusal, never a wait.
    pub fn foreground(&self) -> (bool, bool) {
        let fg = self.master.lock().unwrap().process_group_leader();
        let pid = self.child.try_lock().ok().and_then(|c| c.process_id());
        match (fg, pid) {
            (Some(fg), Some(pid)) => (fg as u32 == pid, fg as u32 != pid),
            _ => (false, false),
        }
    }

    /// End the pane the way closing a terminal window does: SIGHUP to the PTY's
    /// foreground process group first, then the shell itself.
    ///
    /// The order matters. Killing only the shell would leave whatever it was
    /// running — the agent — attached to a PTY nobody owns, because the master
    /// fd cannot be forced shut from here while the output pump sits blocked
    /// reading a clone of it. The HUP is what actually ends the pane's
    /// processes. A backgrounded job is not in the foreground group and so
    /// survives, holding the PTY open until it exits; that resolves itself (the
    /// pump then sees EOF) and is the same bargain a terminal emulator makes.
    pub fn hangup(&self) {
        if let Some(pgrp) = self.master.lock().unwrap().process_group_leader() {
            // SAFETY: killpg on a pid the kernel just handed us; any error
            // (the group is already gone) is the outcome we wanted anyway.
            unsafe { libc::killpg(pgrp, libc::SIGHUP) };
        }
        let _ = self.killer.lock().unwrap().kill();
    }

    /// Collect the exit status so a long-lived daemon does not accumulate
    /// zombies. Called only once the session is out of the registry, and only
    /// after EOF or a kill, so the wait returns immediately.
    pub fn reap(&self) {
        let _ = self.child.lock().unwrap().wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// The seam the whole replay feature rests on. Bytes pushed before the
    /// attach are in the snapshot, bytes pushed after are in the backlog, and no
    /// byte is in both or in neither — and it is one lock, not one interval,
    /// that makes that so.
    #[test]
    fn attach_splits_the_stream_with_no_gap_and_no_duplication() {
        let out = Out::new();
        out.push(b"before-the-attach\n");
        let (replay, backlog) = out.attach();
        out.push(b"after-the-attach\n");
        out.close_all();

        assert_eq!(text(&replay), "before-the-attach\n");
        let mut live = Vec::new();
        while let Some(chunk) = backlog.next_chunk() {
            live.extend_from_slice(&chunk);
        }
        assert_eq!(text(&live), "after-the-attach\n");
    }

    /// A second window on the same pane gets its own replay, and the first one
    /// leaving does not disturb it.
    #[test]
    fn two_clients_each_get_their_own_replay() {
        let out = Out::new();
        out.push(b"shared\n");
        let (first_replay, first) = out.attach();
        let (second_replay, second) = out.attach();
        assert_eq!(text(&first_replay), text(&second_replay));
        out.detach(&first);
        assert_eq!(out.listeners(), 1);
        out.push(b"still-live\n");
        assert_eq!(text(&second.next_chunk().unwrap()), "still-live\n");
    }

    /// Evicting from the front can land inside an escape sequence or a multibyte
    /// character, so a truncated replay resumes at the first newline instead —
    /// the one byte that can never appear inside either.
    #[test]
    fn a_truncated_ring_keeps_the_tail_and_resumes_on_a_line_boundary() {
        let mut ring = Ring::new();
        ring.push(b"oldest\n");
        assert_eq!(text(&ring.snapshot()), "oldest\n", "nothing dropped yet");

        // Overflow it, then leave a recognisable tail with a clean line break.
        ring.push(&vec![b'x'; RING_BYTES]);
        ring.push(b"\x1b[31mcut-me");
        ring.push(b"\nkept: \xe4\xb8\xad\n");
        let snap = ring.snapshot();
        assert!(
            snap.starts_with(b"kept: "),
            "a truncated replay must resume after a newline, got: {:?}",
            text(&snap[..snap.len().min(40)])
        );
        assert!(snap.len() <= RING_BYTES);
        assert!(text(&snap).contains('中'), "the tail must survive intact");
    }

    /// A client that stops reading must not be able to stall the pane (that
    /// would freeze the agent for everyone) nor grow the daemon without bound.
    /// So it is dropped — the recoverable failure, since reconnecting replays.
    #[test]
    fn a_client_that_never_reads_is_dropped_rather_than_allowed_to_stall_the_pane() {
        let out = Out::new();
        let (_replay, backlog) = out.attach();
        let chunk = vec![b'.'; 64 * 1024];
        while out.listeners() > 0 {
            out.push(&chunk);
            assert!(
                out.state.lock().unwrap().ring.buf.len() <= RING_BYTES,
                "the ring is what stays bounded; the client is what gets dropped"
            );
        }
        assert!(
            backlog.next_chunk().is_some(),
            "what it did receive is still there to drain"
        );
    }
}
