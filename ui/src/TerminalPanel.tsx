import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { Terminal } from "@xterm/xterm";
import type { ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Messages } from "./i18n";
import type { Appearance } from "./theme";
import Icon from "./Icon";
import "@xterm/xterm/css/xterm.css";

// Pane keys are this component's bookkeeping and nothing else: they name a row
// in the stack, never a shell. Shell ids come from the backend, which is where
// they have to come from — the shells outlive the app, and `WITNOS_PANE` is the
// durable address a goal's session binding points at.
let nextKey = 1;

/** One pane's shell, minted once and remembered OUTSIDE React.
 *
 *  StrictMode mounts every view twice, and a second `term_spawn` would open a
 *  second shell nobody is watching — which would then sit in the daemon for
 *  ever, since letting go of a pane no longer ends it. Keyed by pane key, so one
 *  pane is one session for as long as the pane lives. This is memory of an id,
 *  never a generator of one. */
const shells = new Map<number, Promise<number>>();

function shellFor(
  key: number,
  restore: number | null,
  cols: number,
  rows: number,
  cwd: string | null,
): Promise<number> {
  let pending = shells.get(key);
  if (!pending) {
    pending = invoke<number>("term_spawn", { id: restore, cols, rows, cwd });
    // A failed open must stay retryable (the header offers a restart), so it is
    // not remembered as this pane's answer.
    pending.catch(() => shells.delete(key));
    shells.set(key, pending);
  }
  return pending;
}

/** End a pane's shell and forget it, so nothing reattaches to a session that is
 *  gone and the next mount opens a fresh one. The only callers are the human's
 *  two deliberate gestures — ✕ and restart; everything else detaches. */
function killShell(pane: Pane) {
  if (pane.shellId !== null) {
    invoke("term_kill", { id: pane.shellId }).catch(() => {});
  }
  shells.delete(pane.key);
}

// xterm's stock ANSI set assumes a dark background: on a light one its yellow
// and its white land near-invisible, and half of what an agent prints is
// coloured. So light mode brings its own darkened set; dark mode keeps the
// defaults, which were tuned for exactly that.
const LIGHT_ANSI: ITheme = {
  black: "#23252c",
  red: "#c0392f",
  green: "#2c7a39",
  yellow: "#8a6300",
  blue: "#2455c4",
  magenta: "#8e3ba8",
  cyan: "#0f7285",
  white: "#5b6070",
  brightBlack: "#666c7a",
  brightRed: "#d84a3f",
  brightGreen: "#37934a",
  brightYellow: "#a3760a",
  brightBlue: "#3a6fd8",
  brightMagenta: "#a44ec0",
  brightCyan: "#158aa0",
  brightWhite: "#23252c",
};

/** Read off the same token block as the rest of the UI (index.css) rather than
 *  restating the palette here — the terminal is most of the window, and two
 *  copies of a colour is how they drift apart. */
function terminalTheme(appearance: Appearance): ITheme {
  const tokens = getComputedStyle(document.documentElement);
  const v = (name: string) => tokens.getPropertyValue(name).trim();
  return {
    background: v("--bg"),
    foreground: v("--text"),
    cursor: v("--accent"),
    selectionBackground: v("--term-sel"),
    ...(appearance === "light" ? LIGHT_ANSI : {}),
  };
}

// Under this a pane shows too few rows to work in; the splitter and the split
// action both refuse to go below it rather than produce unusable slivers.
const MIN_PANE_PX = 84;

/** What the program in a pane is doing, as far as its title lets us tell.
 *  `unknown` is a first-class answer, not a failure: whoever asks must treat it
 *  as "go ahead", never as a reason to withhold something from the agent.
 *
 *  `gone` is the one answer that is NOT about state: there is no such pane in
 *  this app any more, or its shell is dead. It used to be folded into `unknown`,
 *  which made "the title is unfamiliar" and "the agent this goal is bound to no
 *  longer exists" indistinguishable — and the second one is permanent, since a
 *  Claude Code session id never comes back. Only `gone` means unreachable. */
export type PaneActivity = "working" | "idle" | "unknown" | "gone";

/** Ask by shell id — the id the core knows a pane by (WITNOS_PANE, and the
 *  `pane` recorded on a goal's session), not the React-side pane key. */
export type ActivityProbe = (shellId: number) => PaneActivity;

/** "Give me a shell in this directory, focused." Handed up to the app the same
 *  way the activity probe is, for the gestures outside this panel that end a
 *  session and so need a new one (closing a goal). False if the stack had no
 *  room for another pane — the one case where the caller got no shell. */
export type ShellOpener = (dir: string | null) => boolean;

// Claude Code publishes its own state in the title it sets: "✳ …" while it is
// working, "· Claude Code" when it is waiting on you. Read it as a hint only —
// another agent, a shell with its own title, or a future Claude Code that
// renames these all land on `unknown`, which must degrade to "go ahead" rather
// than silently swallow what the human wanted to send.
const WORKING_MARK = "✳";
const IDLE_MARK = "·";

function activityFromTitle(title: string | null): PaneActivity {
  if (!title) return "unknown";
  if (title.startsWith(WORKING_MARK)) return "working";
  if (title.startsWith(IDLE_MARK)) return "idle";
  return "unknown";
}

type PaneApi = {
  focus: () => void;
  /** Move this shell to `dir`; resolves false if it was busy (nothing sent).
   *  On success the pane clears its own viewport once the cd has landed, so
   *  the human doesn't arrive to the `cd` we typed for them. */
  tryCd: (dir: string) => Promise<boolean>;
};

/** A shell the backend already has, as `term_list` reports it. */
type PaneInfo = {
  id: number;
  cwd: string;
  alive: boolean;
};

type Pane = {
  key: number;
  /** The shell this pane is attached to, as the backend named it — null until it
   *  answers, and null again after a restart. This is the id the core knows the
   *  pane by (WITNOS_PANE, and the `pane` recorded on a goal's session), which is
   *  why a restored pane carries the one it already had. */
  shellId: number | null;
  /** Where the shell was started — the only directory it is known to be in. */
  cwd: string | null;
  /** Where it says it is now, when the shell reports it (OSC 7). */
  liveCwd: string | null;
  /** What the program inside calls itself (OSC 0/2), e.g. "· Claude Code". */
  title: string | null;
  /** Bumped to restart this pane's shell (remounts the view). */
  gen: number;
  exited: boolean;
  /** Share of the stack's height, relative to its siblings' weights. */
  weight: number;
};

function trimDir(dir: string): string {
  return dir.replace(/[\\/]+$/, "");
}

function basename(dir: string): string {
  return trimDir(dir).split(/[\\/]/).pop() || dir;
}

/** The same directory, a trailing separator aside. */
function sameDir(dir: string | null, other: string): boolean {
  return dir !== null && trimDir(dir) === trimDir(other);
}

/** `dir` is somewhere inside `root`. A shell the human walked down into a
 *  subdirectory is still that project's terminal, so it counts as one when we
 *  ask whether the project already has a shell. */
function insideDir(dir: string | null, root: string): boolean {
  if (dir === null) return false;
  const d = trimDir(dir);
  const r = trimDir(root);
  return d.length > r.length && d.startsWith(r) && /[\\/]/.test(d[r.length]);
}

/** OSC 7 carries the cwd as a file URL: file://host/path/to/dir. */
function cwdFromOsc7(data: string): string | null {
  const m = /^file:\/\/[^/]*(\/.*)$/.exec(data);
  if (!m) return null;
  try {
    return decodeURIComponent(m[1]);
  } catch {
    return m[1]; // a malformed escape is still better than dropping the path
  }
}

function newPane(cwd: string | null, weight: number): Pane {
  return {
    key: nextKey++,
    shellId: null,
    cwd,
    liveCwd: null,
    title: null,
    gen: 0,
    exited: false,
    weight,
  };
}

/** A pane rebuilt from a shell that is already running: the same id it had
 *  before, the directory it was started in, and its scrollback on the way as the
 *  replay. */
function restoredPane(info: PaneInfo, weight: number): Pane {
  return {
    key: nextKey++,
    shellId: info.id,
    cwd: info.cwd,
    liveCwd: null,
    title: null,
    gen: 0,
    exited: !info.alive,
    weight,
  };
}

function TerminalView({
  paneKey,
  shellId,
  cwd,
  onExit,
  onShell,
  onTitle,
  onCwd,
  bindApi,
  t,
  appearance,
}: {
  paneKey: number;
  /** A shell to reattach to (a pane restored on startup), or null to open one. */
  shellId: number | null;
  cwd: string | null;
  onExit: () => void;
  onShell: (id: number) => void;
  onTitle: (title: string) => void;
  onCwd: (dir: string) => void;
  bindApi: (key: number, api: PaneApi | null) => void;
  t: Messages;
  appearance: Appearance;
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  // Everything the mount-once effect calls later goes through this ref, so a
  // re-render (new language, new pane list) can't leave it on stale closures.
  const cb = useRef({ onExit, onShell, onTitle, onCwd, t });
  cb.current = { onExit, onShell, onTitle, onCwd, t };

  useEffect(() => {
    // The shell this view talks to, once the backend has answered — everything
    // below reads it through this one binding. Before it lands there is no shell
    // to talk to, and nothing pretends otherwise.
    let id: number | null = null;
    const term = new Terminal({
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 13,
      theme: terminalTheme(appearance),
      cursorBlink: true,
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(boxRef.current!);
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;
    // Handing the pane over after a `cd` means not showing the human the `cd`
    // itself. The emulator clears its own viewport for that — no keystroke to
    // the shell, so it costs nothing from whatever shell the pane happens to
    // run, and nothing lands in that shell's command history.
    //
    // Keep the scrollback: `term.clear()` drops the buffer, and the history is
    // what this whole path preserves by walking the shell over instead of
    // restarting it. So push the prompt row to the top of the viewport instead
    // — newlines to scroll it up there, then the cursor back onto it, at the
    // column the shell believes it is in. What was above stays scrollable.
    const clearViewport = () => {
      const n = term.rows - 1;
      if (n > 0) term.write("\n".repeat(n) + `\x1b[${n}A`);
    };
    // "After the cd lands" is a thing only the output stream knows. A
    // successful cd arms this; the FIRST chunk back starts the clock and every
    // later one pushes it out, so the clear happens once the shell has finished
    // echoing the command and drawing the new prompt. Two ways not to fire,
    // both deliberate: output still coming after the deadline disarms it (a
    // chpwd hook that prints, say — clearing the top off something the human
    // may want to read is the worse mistake), and a shell that says nothing at
    // all never starts the clock, leaving the pane exactly as it was.
    let deadline: number | null = null;
    let quiet: ReturnType<typeof setTimeout> | null = null;
    const disarm = () => {
      deadline = null;
      if (quiet) clearTimeout(quiet);
      quiet = null;
    };
    const onOutput = () => {
      if (deadline === null) return;
      if (Date.now() > deadline) return disarm();
      if (quiet) clearTimeout(quiet);
      quiet = setTimeout(() => {
        quiet = null;
        disarm();
        clearViewport();
      }, 120);
    };
    bindApi(paneKey, {
      focus: () => term.focus(),
      tryCd: async (dir) => {
        if (id === null) return false;
        const moved = await invoke<boolean>("term_try_cd", { id, dir });
        if (moved) deadline = Date.now() + 1500;
        return moved;
      },
    });

    let alive = true;
    // The token naming OUR attachment, so letting go cannot cut off whichever
    // view took the pane over after us (StrictMode's second mount is exactly
    // that, and so is any remount).
    let token: number | null = null;
    const opening = shellFor(paneKey, shellId, term.cols, term.rows, cwd);
    opening
      .then(async (shell) => {
        // The pane stopped claiming this shell while the open was in flight —
        // it was closed or restarted. Nobody will ever attach to it now, and a
        // shell nobody attaches to would sit in the daemon for ever, so this is
        // the one place other than the human's own gestures that ends a session.
        if (shells.get(paneKey) !== opening) {
          invoke("term_kill", { id: shell }).catch(() => {});
          return;
        }
        // Already unmounted, but the pane still claims this shell: whoever
        // mounted next attaches to it, and this view must not touch it.
        if (!alive) return;
        id = shell;
        cb.current.onShell(shell);
        // Attaching is per mount, not per session: the shell was already running
        // (a restored pane) or has just been opened, and this is what starts its
        // scrollback replay and its live output flowing into this view.
        token = await invoke<number>("term_attach", { id: shell });
        // Unmounted while that was in flight: let go again, or the pane would
        // keep streaming into a terminal that has been disposed.
        if (!alive) invoke("term_detach", { id: shell, token }).catch(() => {});
      })
      .catch((e) => {
        if (!alive) return;
        term.write(
          `\r\n\x1b[31m${cb.current.t.shellStartFailed(String(e))}\x1b[0m\r\n`,
        );
        // Say it in the header too, or a pane with no shell offers no way back.
        cb.current.onExit();
      });

    term.onData((data) => {
      if (id === null) return; // nothing has opened yet; there is nowhere to type
      invoke("term_write", { id, data }).catch(() => {});
    });

    // The pane header mirrors what the program inside reports: Claude Code
    // publishes its own state there ("· Claude Code" idle, "✳ …" working),
    // which is what makes a split stack readable at a glance.
    term.onTitleChange((title) => cb.current.onTitle(title.trim()));
    // OSC 7 keeps the header honest after a `cd`. Shells that never emit it
    // (macOS's stock zsh only does for Apple_Terminal) just leave the spawn
    // directory showing.
    term.parser.registerOscHandler(7, (data) => {
      const dir = cwdFromOsc7(data);
      if (dir) cb.current.onCwd(dir);
      return true;
    });

    const unOut = listen<{ id: number; data: number[] }>("term:output", (e) => {
      if (e.payload.id !== id) return;
      term.write(new Uint8Array(e.payload.data));
      onOutput();
    });
    const unExit = listen<number>("term:exit", (e) => {
      if (e.payload !== id || !alive) return;
      term.write(`\r\n\x1b[2m${cb.current.t.shellExited}\x1b[0m\r\n`);
      cb.current.onExit();
    });

    // Also the SIGWINCH path for layout changes: splitting, closing, and
    // dragging the divider all resize this box, so a full-screen TUI redraws
    // at the width it actually has.
    const ro = new ResizeObserver(() => {
      if (!boxRef.current || boxRef.current.clientHeight === 0) return;
      fit.fit();
      if (id === null) return; // the size it opens with is the size we just fitted
      invoke("term_resize", { id, cols: term.cols, rows: term.rows }).catch(
        () => {},
      );
    });
    ro.observe(boxRef.current!);

    return () => {
      alive = false;
      if (quiet) clearTimeout(quiet);
      bindApi(paneKey, null);
      ro.disconnect();
      unOut.then((f) => f());
      unExit.then((f) => f());
      // Detach, never kill. This runs on every unmount — StrictMode's second
      // mount, a workspace view switch, the window closing — and the shell has
      // to survive all three: that is the whole feature. Ending a session is a
      // human gesture, and it happens in the panel (see killShell).
      if (id !== null && token !== null) {
        invoke("term_detach", { id, token }).catch(() => {});
      }
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []); // one shell per mount; restart = remount via key

  // Repaint a live shell when the appearance changes — scrollback and all,
  // which is why the theme is swapped on the running terminal instead of
  // remounting it (a remount would kill the shell).
  useEffect(() => {
    const term = termRef.current;
    if (term) term.options.theme = terminalTheme(appearance);
  }, [appearance]);

  // Refit + refocus when the panel becomes visible again.
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const obs = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        requestAnimationFrame(() => {
          fitRef.current?.fit();
        });
      }
    });
    obs.observe(box);
    return () => obs.disconnect();
  }, []);

  return <div className="term-box" ref={boxRef} />;
}

export default function TerminalPanel({
  cwd,
  t,
  appearance,
  bindActivity,
  bindOpen,
  hidden = false,
}: {
  cwd: string | null;
  t: Messages;
  appearance: Appearance;
  // Hands the panel's "what is that pane doing" lookup to the owner, the way
  // TerminalView hands up its PaneApi. A probe rather than state on purpose:
  // an agent retitles its pane every few seconds while it works, and none of
  // that should re-render anything outside this panel.
  bindActivity: (probe: ActivityProbe | null) => void;
  // Same shape, other direction: lets the owner ask for a shell somewhere
  // without it having to know anything about panes.
  bindOpen: (open: ShellOpener | null) => void;
  // Hide instead of unmount: the shells must survive workspace view switches.
  hidden?: boolean;
}) {
  // A vertical stack of shells, top to bottom. Empty until the backend has said
  // which ones already exist (see the restore effect); ⌘D adds another.
  const [panes, setPanes] = useState<Pane[]>([]);
  // Has that question been asked and answered? Until it has, this panel knows
  // nothing about any pane and must not answer questions about them — "not asked
  // yet" is not "gone", and the difference decides whether a goal's agent is
  // reported as unreachable.
  const [restored, setRestored] = useState(false);
  const [focused, setFocused] = useState(0);
  const stackRef = useRef<HTMLDivElement>(null);
  const apis = useRef(new Map<number, PaneApi>());
  // Mirrors of the state for the keyboard/pointer handlers, which must read
  // the current values without re-subscribing on every pane change.
  const panesRef = useRef(panes);
  panesRef.current = panes;
  const focusedRef = useRef(focused);
  focusedRef.current = focused;
  const cwdRef = useRef(cwd);
  cwdRef.current = cwd;

  const bindApi = useCallback((key: number, api: PaneApi | null) => {
    if (!api) {
      apis.current.delete(key);
      return;
    }
    apis.current.set(key, api);
    // A pane that just appeared takes the keyboard, so ⌘D then typing lands
    // in the new shell instead of the old one.
    if (key === focusedRef.current) api.focus();
  }, []);

  const patch = useCallback((key: number, part: Partial<Pane>) => {
    setPanes((prev) =>
      prev.map((p) => (p.key === key ? { ...p, ...part } : p)),
    );
  }, []);

  // Sessions are restored, not respawned: the shells outlive the app, so
  // reopening the window rebuilds one pane per surviving shell and attaches to
  // each — the replay is what makes a terminal look as it was left, agent and
  // all. A fresh shell opens only when nothing survived. Asked once per panel:
  // which shells exist is a property of the machine, not of a render.
  //
  // Read through the ref, not the closure: the census is in flight while the
  // human is free to click a project, and the first pane should open where they
  // are now, not where the panel mounted.
  useEffect(() => {
    let alive = true;
    invoke<PaneInfo[]>("term_list")
      .then((existing) => {
        if (!alive) return;
        setPanes(
          existing.length > 0
            ? existing.map((info) => restoredPane(info, 1))
            : [newPane(cwdRef.current, 1)],
        );
        setRestored(true);
      })
      .catch(() => {
        // Nothing could be listed, so nothing can be restored; the pane that
        // opens here will report its own failure if the backend is really gone.
        if (!alive) return;
        setPanes([newPane(cwdRef.current, 1)]);
        setRestored(true);
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Answered from the title this panel already tracks, at the moment someone
  // asks (see App's send-to-agent), off the same refs the keyboard handlers
  // read — so the answer is current without this being a dependency of
  // anything. A pane this app has no row for, or one whose shell died, is `gone`
  // rather than a state: nothing can be typed at either, so whoever asks may say
  // so plainly. Shell ids are durable and never reused, so a recorded pane this
  // panel cannot find really is gone — but only once the restore has run, which
  // is why the probe is not published before then.
  const activity = useCallback<ActivityProbe>((shellId) => {
    const pane = panesRef.current.find((p) => p.shellId === shellId);
    if (!pane || pane.exited) return "gone";
    return activityFromTitle(pane.title);
  }, []);

  useEffect(() => {
    bindActivity(restored ? activity : null);
    return () => bindActivity(null);
  }, [bindActivity, activity, restored]);

  // A new pane opens in the selected project's directory — the same rule the
  // first pane and "restart here" already follow — falling back to where the
  // pane it split from is. `dir` overrides that for a caller who names the
  // directory outright (see openIn), which is not the same as the selection.
  const split = useCallback(
    (fromKey?: number, dir?: string | null) => {
      const list = panesRef.current;
      const i = list.findIndex(
        (p) => p.key === (fromKey ?? focusedRef.current),
      );
      const src = list[i < 0 ? 0 : i];
      if (!src) return false;
      const stack = stackRef.current;
      if (stack && stack.clientHeight < MIN_PANE_PX * (list.length + 1))
        return false;
      const pane = newPane(dir ?? cwd ?? src.liveCwd ?? src.cwd, src.weight / 2);
      setFocused(pane.key); // bindApi focuses it the moment it mounts
      setPanes((prev) => {
        const j = prev.findIndex((p) => p.key === src.key);
        if (j < 0) return prev;
        const out = prev.slice();
        out[j] = { ...prev[j], weight: prev[j].weight / 2 };
        out.splice(j + 1, 0, pane);
        return out;
      });
      return true;
    },
    [cwd],
  );

  const close = useCallback((key: number) => {
    const list = panesRef.current;
    // The workspace always keeps one shell — closing the last one would leave
    // nothing to type into.
    if (list.length < 2) return;
    const closing = list.find((p) => p.key === key);
    if (!closing) return;
    // Closing a pane is the one gesture that means "end this shell", so it is
    // the one place that kills. Everything else — unmounting, hiding,
    // quitting — detaches.
    killShell(closing);
    setPanes((prev) => {
      const i = prev.findIndex((p) => p.key === key);
      if (i < 0) return prev;
      const out = prev.filter((p) => p.key !== key);
      const heir = Math.min(i, out.length - 1);
      out[heir] = { ...out[heir], weight: out[heir].weight + prev[i].weight };
      return out;
    });
  }, []);

  // Whoever inherits the space also inherits the keyboard.
  useEffect(() => {
    if (panes.some((p) => p.key === focused)) return;
    const heir = panes[0];
    if (!heir) return;
    setFocused(heir.key);
    apis.current.get(heir.key)?.focus();
  }, [panes, focused]);

  const focusStep = useCallback((d: number) => {
    const list = panesRef.current;
    const i = list.findIndex((p) => p.key === focusedRef.current);
    const target =
      list[Math.min(list.length - 1, Math.max(0, (i < 0 ? 0 : i) + d))];
    if (!target) return;
    setFocused(target.key);
    apis.current.get(target.key)?.focus();
  }, []);

  const restart = useCallback(
    (key: number, dir?: string | null) => {
      // The human asked for a new shell here, so the old one ends — otherwise it
      // would keep running in the daemon with nothing attached to it. Clearing
      // shellId is what makes the remount open a fresh session instead of
      // reattaching to the one just killed.
      const previous = panesRef.current.find((p) => p.key === key);
      if (previous) killShell(previous);
      setPanes((prev) =>
        prev.map((p) =>
          p.key === key
            ? {
                ...p,
                shellId: null,
                cwd: dir ?? cwd ?? p.liveCwd ?? p.cwd,
                liveCwd: null,
                title: null,
                exited: false,
                gen: p.gen + 1,
              }
            : p,
        ),
      );
      setFocused(key);
    },
    [cwd],
  );

  // "Give me a shell in this directory" from outside the panel. A goal is bound
  // to one agent session and a session id never comes back, so closing a goal
  // means the next one in that project needs a shell of its own — and it must be
  // a *new* shell, not a cd: /clear or a fresh session is exactly what starts
  // the next goal.
  //
  // A pane whose shell already exited is reused, because there is nothing there
  // to preserve; anything alive is left alone and this splits instead. In
  // particular the closed goal's own pane keeps running with its transcript
  // intact — that is the record of the work just finished, and ✕ or restart stay
  // the only things that end a shell. A stack with no room for another pane
  // declines, the same way ⌘D does.
  const openIn = useCallback<ShellOpener>(
    (dir) => {
      const dead = panesRef.current.find((p) => p.exited);
      if (dead) {
        restart(dead.key, dir);
        return true;
      }
      return split(undefined, dir);
    },
    [restart, split],
  );

  useEffect(() => {
    bindOpen(openIn);
    return () => bindOpen(null);
  }, [bindOpen, openIn]);

  // Picking a project in the sidebar means "put me in this project": its own
  // shell, not the one you were typing in walked over. A pane is a workspace —
  // it holds an agent, a transcript, a place in a directory — so switching
  // projects opens a pane in the new one and leaves the old one standing, the
  // way clicking a project in an editor does. Clicking back finds that pane
  // again rather than a second one: a shell already sitting in the project
  // (or somewhere under it, if you cd'd into a crate) IS that project's
  // terminal, so it is focused instead.
  //
  // The `cd` walk survives as the last resort for a stack with no room for
  // another pane — there the choice is between moving a shell and the click
  // meaning nothing, and it still refuses to type at a pane running something.
  const targeted = useRef(cwd);
  useEffect(() => {
    // Before the census lands this panel knows of no panes, so it cannot tell
    // "no shell is in that project" from "no shell is known yet" — and would
    // open a duplicate of one it is about to restore. The selection waits here
    // and is handled when `restored` flips.
    if (!restored || cwd === null || cwd === targeted.current) return;
    targeted.current = cwd;
    const live = panesRef.current.filter((p) => !p.exited);
    const here =
      live.find((p) => sameDir(p.liveCwd ?? p.cwd, cwd)) ??
      live.find((p) => insideDir(p.liveCwd ?? p.cwd, cwd));
    if (here) {
      setFocused(here.key);
      apis.current.get(here.key)?.focus();
      return;
    }
    if (openIn(cwd)) return;
    const key = focusedRef.current;
    const pane = panesRef.current.find((p) => p.key === key);
    if (!pane) return;
    if (pane.exited) {
      restart(key); // dead shell: nothing to preserve, reopen it in the new dir
      return;
    }
    apis.current
      .get(key)
      ?.tryCd(cwd)
      // The shell is where we just sent it; shells that report cwd (OSC 7)
      // will confirm it a moment later, the rest would otherwise keep showing
      // the directory they were spawned in.
      .then((moved) => moved && patch(key, { cwd, liveCwd: null }))
      .catch(() => {});
  }, [cwd, restored, openIn, restart, patch]);

  useEffect(() => {
    if (hidden) return;
    const h = (e: KeyboardEvent) => {
      if (!e.metaKey || e.ctrlKey) return;
      if (!e.altKey && !e.shiftKey && e.key.toLowerCase() === "d") {
        e.preventDefault();
        split();
      } else if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
        e.preventDefault();
        focusStep(e.key === "ArrowDown" ? 1 : -1);
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [hidden, split, focusStep]);

  // Drag the divider between pane i and i+1: the pair trades weight, the rest
  // of the stack keeps its share. Each pane keeps MIN_PANE_PX.
  const dragFrom = (i: number) => (e: ReactPointerEvent) => {
    const stack = stackRef.current;
    if (!stack) return;
    e.preventDefault();
    const startY = e.clientY;
    const h = Math.max(stack.clientHeight, 1);
    const snapshot = panesRef.current.map((p) => p.weight);
    const total = snapshot.reduce((a, b) => a + b, 0);
    const min = (total * MIN_PANE_PX) / h;
    const [a, b] = [snapshot[i], snapshot[i + 1]];
    const [lo, hi] = [min - a, b - min];
    if (hi < lo) return; // no room to trade; a drag here would only jitter
    const move = (ev: PointerEvent) => {
      const d = Math.max(lo, Math.min(hi, ((ev.clientY - startY) / h) * total));
      setPanes((prev) =>
        prev.map((p, idx) =>
          idx === i
            ? { ...p, weight: a + d }
            : idx === i + 1
              ? { ...p, weight: b - d }
              : p,
        ),
      );
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.classList.remove("row-resizing");
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    document.body.classList.add("row-resizing");
  };

  const equalize = () =>
    setPanes((prev) => prev.map((p) => ({ ...p, weight: 1 })));

  const multi = panes.length > 1;

  return (
    <div className={`term-panel ${hidden ? "hidden" : ""}`} ref={stackRef}>
      {panes.map((p, i) => {
        const dir = p.liveCwd ?? p.cwd;
        // What the program says it is, else the folder it sits in.
        const label = p.title || (dir ? basename(dir) : "") || t.terminal;
        return (
          <Fragment key={p.key}>
            {i > 0 && (
              <div
                className="term-splitter"
                role="separator"
                aria-orientation="horizontal"
                aria-label={t.resizePanes}
                onPointerDown={dragFrom(i - 1)}
                onDoubleClick={equalize}
              />
            )}
            <section
              className={`term-pane ${multi && p.key === focused ? "focused" : ""}`}
              style={{ flexGrow: p.weight }}
              aria-label={label}
              onFocus={() => setFocused(p.key)}
              onMouseDown={() => setFocused(p.key)}
            >
              <header className="term-head">
                <span className="term-title" title={dir ?? undefined}>
                  <Icon name="folder-open" size={12} /> {label}
                </span>
                {dir && <span className="term-cwd">{dir}</span>}
                <span className="spacer" />
                {!p.exited && cwd !== null && cwd !== p.cwd && (
                  <button
                    className="ghost"
                    onClick={() => restart(p.key)}
                    title={cwd}
                  >
                    <Icon name="rotate-cw" size={12} /> {t.restartHere}
                  </button>
                )}
                {p.exited && (
                  <button className="ghost" onClick={() => restart(p.key)}>
                    <Icon name="rotate-cw" size={12} /> {t.restartShell}
                  </button>
                )}
                <button
                  className="ghost icon"
                  onClick={() => split(p.key)}
                  title={t.splitBelow}
                  aria-label={t.splitBelow}
                  aria-keyshortcuts="Meta+D"
                >
                  <Icon name="plus" size={12} />
                </button>
                {multi && (
                  <button
                    className="ghost icon"
                    onClick={() => close(p.key)}
                    title={t.closePane}
                    aria-label={t.closePane}
                  >
                    <Icon name="x" size={12} />
                  </button>
                )}
              </header>
              <TerminalView
                key={p.gen}
                paneKey={p.key}
                shellId={p.shellId}
                cwd={p.cwd}
                bindApi={bindApi}
                onShell={(shell) => patch(p.key, { shellId: shell })}
                onExit={() => patch(p.key, { exited: true })}
                onTitle={(title) => patch(p.key, { title: title || null })}
                onCwd={(d) => patch(p.key, { liveCwd: d })}
                t={t}
                appearance={appearance}
              />
            </section>
          </Fragment>
        );
      })}
    </div>
  );
}
