//! Embedded terminal: the command surface the webview drives, plus the two
//! backends that can own a pane's shell. This is pure UX shell — the store, the
//! gate, and the contract never touch it. The point is that opening Witnos is
//! enough to drive your agent; no external terminal needed.
//!
//! On unix the shells do NOT live here: they live in the `witnos pty-serve`
//! daemon, reached over `$WITNOS_HOME/pty.sock`, which is what lets an agent
//! session survive quitting the app — the goal bound to that session is not
//! orphaned by closing a window (see `daemon`). On Windows there is no daemon
//! (ConPTY offers neither a readable foreground process group nor a filesystem
//! socket), so the app keeps owning its PTYs in-process there and the honest
//! cost is that they still die with it (see `local`).
//!
//! What the two backends share lives here, because it is the part that must not
//! drift: the scope stamps every pane's shell carries, and the two keystroke
//! compositions whose guards point in OPPOSITE directions — a `cd` needs a shell
//! sitting at its prompt, a correction needs a program there to read it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

#[cfg(unix)]
mod daemon;
#[cfg(unix)]
pub use daemon::Terminals;

// The in-process backend is Windows's, and it is compiled into the unix TEST
// build as well — nowhere else. Nobody develops this on Windows, so
// type-checking it under `cargo test` / `cargo clippy --all-targets` is the only
// thing standing between a change to the command surface below and a platform
// that silently stops compiling. It is unreachable on unix, hence the
// dead-code allowance inside the module.
#[cfg(any(not(unix), test))]
mod local;
#[cfg(not(unix))]
pub use local::Terminals;

/// The `WITNOS_PANE` value handed to a backend: only whoever allocates the
/// session id can fill it in, and an environment is fixed at spawn — there is no
/// "set it afterwards". The daemon's protocol defines the placeholder; the
/// in-process backend substitutes it the same way.
const PANE_ID: &str = "{id}";

/// One pane's shell, as the webview needs to know it. Deserialized straight from
/// the daemon's `list` (which carries more fields; the extra ones are ignored).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: u32,
    pub cwd: String,
    pub alive: bool,
}

#[derive(Clone, Serialize)]
struct Output {
    id: u32,
    data: Vec<u8>,
}

/// What an attached pane reports, in the order it happens: output, then at most
/// one `Ended` — and `Ended` only when the SESSION finished, never when the app
/// merely let go of the pane. Confusing those two is how a live agent gets
/// painted as a dead one.
pub enum PaneEvent {
    Output(Vec<u8>),
    Ended,
}

/// Where an attached pane's traffic goes. A closure rather than an `AppHandle`
/// so the backends can be exercised without a Tauri app around them.
pub type Sink = Box<dyn Fn(u32, PaneEvent) + Send + Sync + 'static>;

/// Hand a pane's traffic to the webview, at the event names and shapes xterm.js
/// is already listening for.
fn webview_sink(app: AppHandle) -> Sink {
    Box::new(move |id, event| match event {
        // Raw bytes, forwarded as-is: xterm.js does its own UTF-8 decoding, so a
        // multibyte character split across two reads still renders correctly.
        // Anything lossy here (a `String::from_utf8_lossy`, say) would corrupt
        // every non-ASCII character that happens to straddle a read boundary.
        PaneEvent::Output(data) => {
            let _ = app.emit("term:output", Output { id, data });
        }
        PaneEvent::Ended => {
            let _ = app.emit("term:exit", id);
        }
    })
}

/// Everything a pane's shell is told about Witnos. The backend adds `TERM` and
/// nothing else, deliberately: these are the half that knows what Witnos IS, and
/// they have to survive the move off the in-process PTYs unchanged.
fn pane_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    // The scope stamp: agents started in this shell inherit it, and so do the
    // hook processes they run, which is how the hooks tell "launched from
    // Witnos" (gets a goal, gets gated) from any other terminal (left alone).
    env.insert("WITNOS_TERMINAL".to_string(), "1".to_string());
    // Which pane this is, inherited the same way. The binding hook forwards it
    // to the core, so a goal knows the terminal its agent session lives in —
    // that is the address the human's correction gets typed back to.
    env.insert("WITNOS_PANE".to_string(), PANE_ID.to_string());
    // Make the bundled `witnos` CLI reachable by name for the human and for
    // agents launched from this shell (agent-facing instructions carry the
    // absolute path anyway — this is convenience, not a load-bearing link).
    if let Some(bin_dir) = crate::bundled_cli().as_deref().and_then(Path::parent) {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let inherited = std::env::var("PATH").unwrap_or_default();
        env.insert("PATH".to_string(), format!("{}{sep}{inherited}", bin_dir.display()));
    }
    env
}

/// Quote a path for a POSIX shell: single quotes, with any embedded single
/// quote closed-escaped-reopened. Spaces, `$`, parens and the rest survive.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The keystrokes that walk a shell to `dir`.
///
/// Ctrl-U first: an idle prompt may still hold a half-typed line, and appending
/// `cd …` to it would run one garbage command instead. Losing the half-line is
/// the lesser cost. `\r` is what the Enter key actually sends.
fn cd_keys(dir: &str) -> String {
    format!("\x15cd {}\r", sh_quote(dir))
}

/// The keystrokes that hand `text` to the program in a pane.
///
/// One line only. Splitting the text on newlines would submit several messages,
/// so text carrying `\r`/`\n` is refused outright rather than silently
/// truncated — the caller composes a single line or nothing. Checked before any
/// guard, so nothing is ever written on the way to this refusal.
///
/// Ctrl-U for the same reason as `cd_keys`: the input line may already hold
/// something half-typed, and appending to it would submit one garbled line. This
/// assumes the target program reads ^U as kill-line — true of the tty line
/// discipline and of readline, NOT verified for Claude Code's own TUI.
/// Best-effort hygiene, not a guarantee; the guard is the load-bearing part.
fn prompt_keys(text: &str) -> Result<String, String> {
    if text.contains('\r') || text.contains('\n') {
        return Err("prompt text must be a single line".into());
    }
    Ok(format!("\x15{text}\r"))
}

/// Make sure this pane has a shell, and answer with the id it lives under.
///
/// `id` names a session the backend is expected to still have — a pane rebuilt
/// on startup, which is how a restored terminal keeps the identity a goal's
/// session binding points at. It is verified, never assumed: a shell that exited
/// meanwhile must surface as an error rather than be silently replaced by a
/// different one under the same id.
///
/// Without an id a new session is allocated and its id returned. Ids come from
/// whoever owns the shells and from nowhere else — the webview must not mint
/// them, because `WITNOS_PANE` is a durable address.
#[tauri::command]
pub async fn term_spawn(
    state: State<'_, Terminals>,
    id: Option<u32>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<u32, String> {
    // Async, and blocking inside: the first call of a run may have to start the
    // daemon, and that is seconds the window must not spend frozen.
    state.ensure(id, cols, rows, cwd.as_deref(), &pane_env())
}

/// Stream a pane here: its scrollback replays first, then live output continues.
/// Attaching again replaces the earlier attachment, which is what a remount is.
/// The token that comes back names this attachment, and `term_detach` needs it.
#[tauri::command]
pub fn term_attach(app: AppHandle, state: State<'_, Terminals>, id: u32) -> Result<u64, String> {
    state.attach(id, webview_sink(app))
}

/// Stop streaming a pane and leave its shell running. This is the ordinary way
/// a view goes away — unmounting, switching the workspace view, quitting the app
/// — and it is the whole feature: the agent in there does not notice.
///
/// `token` is the one `term_attach` answered with, so a view that was unmounted
/// while its own attach was still in flight cannot cut off the view that has
/// since taken the pane over.
#[tauri::command]
pub fn term_detach(state: State<'_, Terminals>, id: u32, token: u64) {
    state.detach(id, Some(token));
}

/// The panes that exist right now, so a fresh window can be rebuilt from the
/// shells that are already running instead of starting new ones.
#[tauri::command]
pub async fn term_list(state: State<'_, Terminals>) -> Result<Vec<PaneInfo>, String> {
    state.list()
}

#[tauri::command]
pub fn term_write(state: State<'_, Terminals>, id: u32, data: String) -> Result<(), String> {
    state.write(id, data.as_bytes())
}

#[tauri::command]
pub fn term_resize(state: State<'_, Terminals>, id: u32, cols: u16, rows: u16) -> Result<(), String> {
    state.resize(id, cols, rows)
}

/// End a pane for good. The only path that signals a shell: closing a pane with
/// ✕ and restarting one are deliberate human acts, and everything else detaches.
#[tauri::command]
pub fn term_kill(state: State<'_, Terminals>, id: u32) {
    state.kill(id);
}

/// Walk an idle shell over to `dir` the way the human would — the pane keeps its
/// scrollback and its process. Returns false when the pane is busy, having sent
/// nothing at all: that refusal is the whole safety property, so there is no
/// path here that writes anyway.
///
/// The guard is `at_prompt`, and it is the PTY's foreground process group that
/// answers it: the shell owns that group exactly when nothing else is running in
/// it. An agent, a build, or an editor owns its own group, and keystrokes sent
/// then would land in *that* program's input — for Claude Code they would become
/// a prompt.
///
/// A pane that answers NEITHER way (see `Terminals::foreground`) refuses too:
/// not knowing must cost a refusal, never a guess.
fn try_cd(state: &Terminals, id: u32, dir: &str) -> Result<bool, String> {
    let Some((at_prompt, _)) = state.foreground(id)? else {
        return Err("no such terminal".into());
    };
    if !at_prompt {
        return Ok(false);
    }
    state.write(id, cd_keys(dir).as_bytes())?;
    Ok(true)
}

#[tauri::command]
pub fn term_try_cd(state: State<'_, Terminals>, id: u32, dir: String) -> Result<bool, String> {
    try_cd(&state, id, &dir)
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
/// `Ok(None)` = no such pane (the human closed it, or the shell exited).
/// `Ok(Some(false))` = the pane is there but sitting at a bare shell prompt, so
/// there is no agent in it to talk to. `Ok(Some(true))` = sent.
pub fn prompt_pane(state: &Terminals, id: u32, text: &str) -> Result<Option<bool>, String> {
    let keys = prompt_keys(text)?;
    let Some((_, program_running)) = state.foreground(id)? else {
        return Ok(None);
    };
    if !program_running {
        return Ok(Some(false));
    }
    state.write(id, keys.as_bytes())?;
    Ok(Some(true))
}

#[tauri::command]
pub fn term_try_prompt(state: State<'_, Terminals>, id: u32, text: String) -> Result<bool, String> {
    prompt_pane(&state, id, &text)?.ok_or_else(|| "no such terminal".to_string())
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

    /// Both compositions start with the kill-line, because both are typed into
    /// something that may already hold a half-finished line, and both end with
    /// the carriage return the Enter key actually sends.
    #[test]
    fn both_keystroke_lines_kill_whatever_was_half_typed_first() {
        assert_eq!(cd_keys("/tmp/it's"), "\x15cd '/tmp/it'\\''s'\r");
        assert_eq!(prompt_keys("the palette moved").unwrap(), "\x15the palette moved\r");
    }

    /// Multi-line text would submit several messages, so it is refused before
    /// anything can be written — the refusal is the composition's job, ahead of
    /// any pane guard.
    #[test]
    fn multi_line_prompt_text_is_refused_rather_than_split() {
        assert!(prompt_keys("REFUSED\nline TWO").is_err());
        assert!(prompt_keys("REFUSED\rline TWO").is_err());
    }

    /// The stamps are what make a session Witnos's business at all: without
    /// `WITNOS_TERMINAL` the hooks leave the agent alone, and without a pane
    /// address a correction has nowhere to go. The pane value stays the
    /// placeholder here — only the backend that allocates the id can fill it in.
    #[test]
    fn every_pane_carries_the_scope_stamps() {
        let env = pane_env();
        assert_eq!(env.get("WITNOS_TERMINAL").map(String::as_str), Some("1"));
        assert_eq!(env.get("WITNOS_PANE").map(String::as_str), Some("{id}"));
    }
}
