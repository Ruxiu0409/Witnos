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

/// The shell a pane runs. Kept in one place because two properties depend on
/// its exact shape: the login flags (the user's real PATH) and its being
/// interactive — job control is what makes `at_prompt` below able to tell a
/// waiting prompt from a program running in it.
fn shell_command(cwd: Option<String>, pane: u32) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(default_shell());
    // Login shell so a GUI-launched app still gets the user's real PATH
    // (agent CLIs, cargo, …).
    if cfg!(unix) {
        cmd.arg("-l");
    }
    cmd.env("TERM", "xterm-256color");
    // The scope stamp: agents started in this shell inherit it, and so do the
    // hook processes they run, which is how the hooks tell "launched from
    // Witnos" (gets a goal, gets gated) from any other terminal (left alone).
    cmd.env("WITNOS_TERMINAL", "1");
    // Which pane this is, inherited the same way. The binding hook forwards it
    // to the core, so a goal knows the terminal its agent session lives in —
    // that is the address the human's correction gets typed back to.
    cmd.env("WITNOS_PANE", pane.to_string());
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
    cmd
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

    let child = pair
        .slave
        .spawn_command(shell_command(cwd, id))
        .map_err(|e| e.to_string())?;
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

/// Quote a path for a POSIX shell: single quotes, with any embedded single
/// quote closed-escaped-reopened. Spaces, `$`, parens and the rest survive.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Is the shell sitting at its own prompt? The PTY's foreground process group
/// is the shell itself exactly when nothing else is running in it; an agent, a
/// build, or an editor owns its own group, and keystrokes sent then would land
/// in *that* program's input — for Claude Code, they'd become a prompt.
///
/// This is the guard for typing a *command* (`try_cd`): only a shell can run
/// one. The opposite question — is a program there to be talked to — is
/// `program_running` below.
#[cfg(unix)]
fn at_prompt(s: &Session) -> bool {
    match (s.master.process_group_leader(), s.child.process_id()) {
        (Some(fg), Some(pid)) => fg as u32 == pid,
        _ => false,
    }
}

/// Is a foreground program (the agent, a build, an editor) holding the pane?
/// The guard for typing a *message* (`try_prompt`): a correction aimed at the
/// agent only means anything when the agent is the one reading the keystrokes.
///
/// Deliberately not written as `!at_prompt`: when the PTY answers neither way —
/// no foreground group, no child pid — both predicates are false, so both
/// callers refuse instead of guessing.
#[cfg(unix)]
fn program_running(s: &Session) -> bool {
    match (s.master.process_group_leader(), s.child.process_id()) {
        (Some(fg), Some(pid)) => fg as u32 != pid,
        _ => false,
    }
}

/// ConPTY exposes no foreground-process signal, so neither question has an
/// answer on Windows — and both answers are needed for their own direction of
/// safety. So both say no: nothing is typed into a pane we cannot read, not a
/// `cd` and not a correction. The human still has the explicit restart-here
/// button, and a Windows agent simply never gets typed into.
#[cfg(not(unix))]
fn at_prompt(_s: &Session) -> bool {
    false
}

#[cfg(not(unix))]
fn program_running(_s: &Session) -> bool {
    false
}

/// Walk an idle shell over to `dir` the way the human would — the pane keeps
/// its scrollback and its process. Returns false when the pane is busy, having
/// sent nothing at all: that refusal is the whole safety property, so there is
/// no path here that writes anyway.
fn try_cd(s: &mut Session, dir: &str) -> Result<bool, String> {
    if !at_prompt(s) {
        return Ok(false);
    }
    // Ctrl-U first: an idle prompt may still hold a half-typed line, and
    // appending `cd …` to it would run one garbage command instead. Losing the
    // half-line is the lesser cost. \r is what the Enter key actually sends.
    //
    // Then Ctrl-L, so the pane the human is handed doesn't open with the `cd`
    // we typed for them. It is the keystroke a person would press, which is
    // what makes it the right one here: the shell's own clear-screen widget
    // wipes the viewport and redraws the prompt, leaving the scrollback intact
    // and leaving nothing in the history — where `clear` would add a command
    // line AND, on macOS, drop the scrollback (/usr/bin/clear emits ESC[3J).
    // Keeping the history is the whole reason this walks the shell over instead
    // of restarting it. A shell with no line editor at all would take the ^L as
    // input; it stays buffered until the prompt reads it, so the worst case is
    // one stray character, never a command.
    let line = format!("\x15cd {}\r\x0c", sh_quote(dir));
    s.writer
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn term_try_cd(state: State<'_, Terminals>, id: u32, dir: String) -> Result<bool, String> {
    let mut sessions = state.0.lock().unwrap();
    let s = sessions.get_mut(&id).ok_or("no such terminal")?;
    try_cd(s, &dir)
}

/// Type one line at the program holding the pane — the human's correction,
/// handed to the agent the way they would type it themselves. The guard is the
/// MIRROR of `try_cd`'s, not a copy of it: a correction is addressed to a
/// program, so it is written only when a program is there to read it, and
/// refused when the pane sits at a bare shell prompt — there the human's prose
/// would be handed to zsh, which would try to run it as a command.
///
/// "Is a program there at all" is the safety property, and it is the only one
/// the foreground process group can carry. Idle-versus-working it cannot tell
/// apart, and does not need to: keystrokes sent to Claude Code mid-turn land in
/// its input box and Enter queues the message — rude at worst, never
/// destructive. Whether the agent looks idle is decided in the frontend, from
/// the OSC title Claude Code publishes; do not try to infer it here.
///
/// One line only. Splitting the text on newlines would submit several
/// messages, so text carrying `\r`/`\n` is refused outright rather than
/// silently truncated — the caller composes a single line or nothing.
fn try_prompt(s: &mut Session, text: &str) -> Result<bool, String> {
    if text.contains('\r') || text.contains('\n') {
        return Err("prompt text must be a single line".into());
    }
    if !program_running(s) {
        return Ok(false);
    }
    // Ctrl-U for the same reason as try_cd: the input line may already hold
    // something half-typed, and appending to it would submit one garbled line.
    // This assumes the target program reads ^U as kill-line — true of the tty
    // line discipline and of readline, NOT verified for Claude Code's own TUI.
    // Best-effort hygiene, not a guarantee; the guard above is the load-bearing
    // part of this function.
    let line = format!("\x15{text}\r");
    s.writer
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Type into the pane with this id. `Ok(None)` = no such pane (the human closed
/// it), `Ok(Some(false))` = the pane sits at a bare shell prompt, so there is no
/// agent there to talk to, `Ok(Some(true))` = sent.
pub fn prompt_pane(state: &Terminals, id: u32, text: &str) -> Result<Option<bool>, String> {
    let mut sessions = state.0.lock().unwrap();
    match sessions.get_mut(&id) {
        Some(s) => try_prompt(s, text).map(Some),
        None => Ok(None),
    }
}

#[tauri::command]
pub fn term_try_prompt(
    state: State<'_, Terminals>,
    id: u32,
    text: String,
) -> Result<bool, String> {
    prompt_pane(&state, id, &text)?.ok_or_else(|| "no such terminal".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_a_shell_would_otherwise_split() {
        assert_eq!(sh_quote("/Users/a/My Project"), "'/Users/a/My Project'");
        assert_eq!(sh_quote("/tmp/$HOME (1)"), "'/tmp/$HOME (1)'");
        assert_eq!(sh_quote("/tmp/it's"), r"'/tmp/it'\''s'");
    }

    /// A real shell on a real PTY, its output collected — the only way to check
    /// what these keystrokes actually do to the user's own `$SHELL`.
    #[cfg(unix)]
    struct Shell {
        session: Session,
        out: std::sync::Arc<std::sync::Mutex<String>>,
    }

    #[cfg(unix)]
    impl Shell {
        fn spawn(cwd: Option<String>) -> Self {
            let pair = native_pty_system().openpty(size(80, 24)).unwrap();
            let child = pair.slave.spawn_command(shell_command(cwd, 1)).unwrap();
            drop(pair.slave);
            let mut reader = pair.master.try_clone_reader().unwrap();
            let out = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let sink = out.clone();
            // Drained on a thread so a full PTY buffer can never be what stalls
            // a test, and so assertions can read what the shell printed.
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    sink.lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            });
            Shell {
                session: Session {
                    writer: pair.master.take_writer().unwrap(),
                    master: pair.master,
                    child,
                },
                out,
            }
        }

        fn type_in(&mut self, keys: &str) {
            self.session.writer.write_all(keys.as_bytes()).unwrap();
        }

        /// Poll rather than sleep a guessed interval: the shell claims the
        /// terminal, runs, and prints on its own schedule.
        fn wait_until(&self, what: impl Fn(&Shell) -> bool) -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                if what(self) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        }

        fn printed(&self, s: &str) -> bool {
            self.out.lock().unwrap().contains(s)
        }

        /// How many times it appeared — the tty echoes what we type, so only a
        /// second occurrence proves a program read the line and wrote it back.
        fn count(&self, s: &str) -> usize {
            self.out.lock().unwrap().matches(s).count()
        }
    }

    #[cfg(unix)]
    impl Drop for Shell {
        fn drop(&mut self) {
            let _ = self.session.child.kill();
        }
    }

    /// The property the whole feature rests on, in both directions: a shell
    /// waiting at its prompt reads as idle and as holding no program (that is
    /// what keeps `cd` out of a running agent's input, and prose out of zsh),
    /// and a shell with a program in it reads as the exact opposite.
    #[cfg(unix)]
    #[test]
    fn at_prompt_tells_a_waiting_prompt_from_a_running_program() {
        let mut sh = Shell::spawn(None);
        assert!(
            sh.wait_until(|sh| at_prompt(&sh.session)),
            "a fresh shell should read as at its prompt"
        );
        assert!(
            !program_running(&sh.session),
            "nothing is running in a fresh shell"
        );
        sh.type_in("sleep 30\r");
        assert!(
            sh.wait_until(|sh| !at_prompt(&sh.session)),
            "a shell running `sleep` should not read as at its prompt"
        );
        assert!(
            program_running(&sh.session),
            "`sleep` should read as a program holding the pane"
        );
        sh.type_in("\x03"); // ^C: back to the prompt
        assert!(
            sh.wait_until(|sh| at_prompt(&sh.session)),
            "the prompt should read as idle again"
        );
        assert!(!program_running(&sh.session));
    }

    /// End to end through the user's own shell: the pane lands in the target
    /// directory, a name full of shell metacharacters and all, and the line the
    /// human had half-typed does not get run as part of the `cd`.
    #[cfg(unix)]
    #[test]
    fn moves_an_idle_shell_into_the_target_directory() {
        let home = std::env::temp_dir().join("witnos-cd-test");
        let target = home.join("it's a project (1)");
        std::fs::create_dir_all(&target).unwrap();
        let target = std::fs::canonicalize(&target).unwrap();
        let shown = target.to_string_lossy().to_string();

        let mut sh = Shell::spawn(Some(std::env::temp_dir().to_string_lossy().into()));
        assert!(sh.wait_until(|sh| at_prompt(&sh.session)));
        // A half-typed line, never submitted — exactly what a prompt is likely
        // to be holding when the human clicks a folder.
        sh.type_in("echo MARKER");
        assert!(try_cd(&mut sh.session, &shown).unwrap());

        // The pane is handed over with a clean screen — the human should not
        // arrive to the `cd` we typed for them. The shell's own clear-screen
        // widget does it, so the scrollback survives: ESC[3J, which
        // /usr/bin/clear emits on macOS, must not appear.
        assert!(
            sh.wait_until(|sh| sh.printed("\x1b[2J")),
            "expected the viewport to be cleared, got:\n{}",
            sh.out.lock().unwrap()
        );
        assert!(
            !sh.printed("\x1b[3J"),
            "the scrollback must survive the clear:\n{}",
            sh.out.lock().unwrap()
        );

        sh.type_in("pwd\r");
        assert!(
            sh.wait_until(|sh| sh.printed(&format!("{shown}\r\n"))),
            "expected pwd to print {shown}, got:\n{}",
            sh.out.lock().unwrap()
        );
        // Without the kill-line, the shell would have run `echo MARKERcd '…'`.
        assert!(
            !sh.printed("MARKERcd"),
            "the half-typed line was concatenated into the cd:\n{}",
            sh.out.lock().unwrap()
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The correction path, end to end, in the case it exists for: a program
    /// (here `cat`, standing in for the agent) owns the pane, and the line
    /// reaches *it* — with whatever was half-typed killed off first rather than
    /// riding along.
    #[cfg(unix)]
    #[test]
    fn types_a_correction_into_the_program_holding_the_pane() {
        let mut sh = Shell::spawn(Some(std::env::temp_dir().to_string_lossy().into()));
        assert!(sh.wait_until(|sh| at_prompt(&sh.session)));
        sh.type_in("cat\r");
        assert!(sh.wait_until(|sh| program_running(&sh.session)));
        // A half-typed message, never submitted — what an agent's input box is
        // likely to be holding when the human clicks send.
        sh.type_in("HALF");

        assert!(try_prompt(&mut sh.session, "the-contract-moved").unwrap());
        // Twice on screen is the proof of delivery: once as the tty's echo of
        // the keystrokes, once written back out by the program that read them.
        assert!(
            sh.wait_until(|sh| sh.count("the-contract-moved") >= 2),
            "the line should have reached the program itself, got:\n{}",
            sh.out.lock().unwrap()
        );
        // `cat` writes back exactly what it was given: without the kill-line it
        // would have received (and echoed) one concatenated line.
        assert!(
            !sh.printed("HALFthe-contract-moved"),
            "the half-typed line was concatenated into the correction:\n{}",
            sh.out.lock().unwrap()
        );
    }

    /// The refusal that Fix 1 is about: an empty pane has no agent in it, and
    /// the human's prose is not a shell command. Nothing is typed at all.
    #[cfg(unix)]
    #[test]
    fn refuses_to_prompt_a_pane_sitting_at_a_bare_shell() {
        let mut sh = Shell::spawn(Some(std::env::temp_dir().to_string_lossy().into()));
        assert!(sh.wait_until(|sh| at_prompt(&sh.session)));

        assert!(!try_prompt(&mut sh.session, "the palette is still NOT-A-COMMAND").unwrap());
        // The sentinel proves the pane was live and echoing all along, so the
        // absence below is a refusal and not a dead terminal.
        sh.type_in("echo SENTINEL\r");
        assert!(sh.wait_until(|sh| sh.printed("SENTINEL\r\n")));
        assert!(
            !sh.printed("NOT-A-COMMAND"),
            "prose was handed to the shell, which would try to run it:\n{}",
            sh.out.lock().unwrap()
        );
    }

    /// Multi-line text would submit several messages, so it is refused before
    /// anything is written — the composer's job is to produce one line. Checked
    /// with a program in the pane, so it is the newline check refusing and not
    /// the guard.
    #[cfg(unix)]
    #[test]
    fn refuses_multi_line_prompt_text_without_writing() {
        let mut sh = Shell::spawn(Some(std::env::temp_dir().to_string_lossy().into()));
        assert!(sh.wait_until(|sh| at_prompt(&sh.session)));
        sh.type_in("cat\r");
        assert!(sh.wait_until(|sh| program_running(&sh.session)));

        assert!(try_prompt(&mut sh.session, "REFUSED\nline TWO").is_err());
        assert!(try_prompt(&mut sh.session, "REFUSED\rline TWO").is_err());
        // Nothing was written, so the program never saw either line. The
        // sentinel is what proves the pane was live and echoing all along.
        sh.type_in("SENTINEL\r");
        assert!(sh.wait_until(|sh| sh.printed("SENTINEL\r\n")));
        assert!(
            !sh.printed("REFUSED"),
            "refused text must not reach the pane:\n{}",
            sh.out.lock().unwrap()
        );
    }

    /// The `cd` refusal, end to end: while a program owns the terminal, nothing
    /// is typed into it — not the `cd`, not the kill-line — and the shell is
    /// still where it was afterwards. (The correction path is the mirror image
    /// and refuses in the opposite case — see the two tests above.)
    #[cfg(unix)]
    #[test]
    fn refuses_to_type_into_a_pane_that_is_busy() {
        let start = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let shown = start.to_string_lossy().to_string();
        let mut sh = Shell::spawn(Some(shown.clone()));
        assert!(sh.wait_until(|sh| at_prompt(&sh.session)));

        // `cat` stands in for the agent: it owns the terminal and would take
        // any keystrokes as its own input.
        sh.type_in("cat\r");
        assert!(sh.wait_until(|sh| !at_prompt(&sh.session)));
        assert!(!try_cd(&mut sh.session, "/usr").unwrap());
        assert!(
            !sh.printed("cd '/usr'"),
            "keystrokes reached the running program:\n{}",
            sh.out.lock().unwrap()
        );

        sh.type_in("\x04"); // EOF: cat exits, shell is back
        assert!(sh.wait_until(|sh| at_prompt(&sh.session)));
        sh.type_in("pwd\r");
        assert!(
            sh.wait_until(|sh| sh.printed(&format!("{shown}\r\n"))),
            "the shell should not have moved, got:\n{}",
            sh.out.lock().unwrap()
        );
        assert!(!sh.printed("/usr\r\n"), "the shell moved anyway");
    }
}
