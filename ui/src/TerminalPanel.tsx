import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { Terminal } from "@xterm/xterm";
import type { ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Messages } from "./i18n";
import type { Appearance } from "./theme";
import "@xterm/xterm/css/xterm.css";

// Fresh ids per mount: StrictMode's mount→unmount→mount must not let two
// shells share one id (spawn/kill invokes may resolve out of order). Pane
// keys come from the same counter, so a new pane can never inherit a dead
// pane's identity either.
let nextId = Math.floor(Math.random() * 0x7fffffff);

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
  /** The id the shell was spawned under — this pane's identity to the core.
   *  It is minted per mount inside the view, so it travels up from there
   *  instead of being hoisted: a restart must get a fresh one. */
  shellId: number;
  focus: () => void;
  /** Move this shell to `dir`; resolves false if it was busy (nothing sent).
   *  On success the pane clears its own viewport once the cd has landed, so
   *  the human doesn't arrive to the `cd` we typed for them. */
  tryCd: (dir: string) => Promise<boolean>;
};

type Pane = {
  key: number;
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

function basename(dir: string): string {
  return (
    dir
      .replace(/[\\/]+$/, "")
      .split(/[\\/]/)
      .pop() || dir
  );
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
    key: nextId++,
    cwd,
    liveCwd: null,
    title: null,
    gen: 0,
    exited: false,
    weight,
  };
}

function TerminalView({
  paneKey,
  cwd,
  onExit,
  onTitle,
  onCwd,
  bindApi,
  t,
  appearance,
}: {
  paneKey: number;
  cwd: string | null;
  onExit: () => void;
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
  const cb = useRef({ onExit, onTitle, onCwd, t });
  cb.current = { onExit, onTitle, onCwd, t };

  useEffect(() => {
    const id = nextId++;
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
    // "After the cd lands" is known only to the output stream: the echo, the
    // cd, and the new prompt arrive as a couple of chunks, so wait out a short
    // silence after the last of them. A shell that keeps printing (a chpwd
    // hook, say) runs past the deadline and is left alone — clearing the top
    // off output the human may want to read is the worse mistake.
    let deadline: number | null = null;
    let quiet: ReturnType<typeof setTimeout> | null = null;
    const settle = () => {
      if (deadline === null) return;
      if (quiet) clearTimeout(quiet);
      quiet = setTimeout(() => {
        quiet = null;
        if (deadline === null) return;
        deadline = null;
        clearViewport();
      }, 120);
    };
    bindApi(paneKey, {
      shellId: id,
      focus: () => term.focus(),
      tryCd: async (dir) => {
        const moved = await invoke<boolean>("term_try_cd", { id, dir });
        if (moved) {
          deadline = Date.now() + 1500;
          settle(); // a shell that prints nothing at all still gets cleared
        }
        return moved;
      },
    });

    let alive = true;
    invoke("term_spawn", { id, cols: term.cols, rows: term.rows, cwd }).catch(
      (e) =>
        term.write(
          `\r\n\x1b[31m${cb.current.t.shellStartFailed(String(e))}\x1b[0m\r\n`,
        ),
    );

    term.onData((data) => {
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
      if (deadline !== null && Date.now() > deadline) {
        deadline = null; // still talking: not our screen to clear
        if (quiet) clearTimeout(quiet);
        quiet = null;
      } else {
        settle();
      }
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
      invoke("term_kill", { id }).catch(() => {});
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
  // Hide instead of unmount: the shells must survive workspace view switches.
  hidden?: boolean;
}) {
  // A vertical stack of shells, top to bottom. One at first; ⌘D adds another.
  const [panes, setPanes] = useState<Pane[]>(() => [newPane(cwd, 1)]);
  const [focused, setFocused] = useState(panes[0].key);
  const stackRef = useRef<HTMLDivElement>(null);
  const apis = useRef(new Map<number, PaneApi>());
  // Mirrors of the state for the keyboard/pointer handlers, which must read
  // the current values without re-subscribing on every pane change.
  const panesRef = useRef(panes);
  panesRef.current = panes;
  const focusedRef = useRef(focused);
  focusedRef.current = focused;

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

  // Answered from the title this panel already tracks, at the moment someone
  // asks (see App's send-to-agent), off the same refs the keyboard handlers
  // read — so the answer is current without this being a dependency of
  // anything. A pane this app never had, or one whose shell died, is `gone`
  // rather than a state: its title is frozen at the moment of death, and after
  // a restart the ids on a goal name panes from a previous run. Nothing can be
  // typed at either, so whoever asks may say so plainly.
  const activity = useCallback<ActivityProbe>((shellId) => {
    const pane = panesRef.current.find(
      (p) => apis.current.get(p.key)?.shellId === shellId,
    );
    if (!pane || pane.exited) return "gone";
    return activityFromTitle(pane.title);
  }, []);

  useEffect(() => {
    bindActivity(activity);
    return () => bindActivity(null);
  }, [bindActivity, activity]);

  // A new pane opens in the selected project's directory — the same rule the
  // first pane and "restart here" already follow — falling back to where the
  // pane it split from is.
  const split = useCallback(
    (fromKey?: number) => {
      const list = panesRef.current;
      const i = list.findIndex(
        (p) => p.key === (fromKey ?? focusedRef.current),
      );
      const src = list[i < 0 ? 0 : i];
      if (!src) return;
      const stack = stackRef.current;
      if (stack && stack.clientHeight < MIN_PANE_PX * (list.length + 1)) return;
      const pane = newPane(cwd ?? src.liveCwd ?? src.cwd, src.weight / 2);
      setFocused(pane.key); // bindApi focuses it the moment it mounts
      setPanes((prev) => {
        const j = prev.findIndex((p) => p.key === src.key);
        if (j < 0) return prev;
        const out = prev.slice();
        out[j] = { ...prev[j], weight: prev[j].weight / 2 };
        out.splice(j + 1, 0, pane);
        return out;
      });
    },
    [cwd],
  );

  const close = useCallback((key: number) => {
    setPanes((prev) => {
      // The workspace always keeps one shell — closing the last one would
      // leave nothing to type into.
      if (prev.length < 2) return prev;
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
    (key: number) => {
      setPanes((prev) =>
        prev.map((p) =>
          p.key === key
            ? {
                ...p,
                cwd: cwd ?? p.liveCwd ?? p.cwd,
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

  // Picking a project in the sidebar walks the shell you're typing in over to
  // it — the same `cd` you'd type, so the pane keeps its scrollback and its
  // process. Only the focused pane moves: panes are separate workspaces, and
  // a stack deliberately spread across directories shouldn't collapse onto one
  // click. A pane that's running something is left strictly alone (term_try_cd
  // refuses rather than typing into an agent); the header's "restart here"
  // button stays as the deliberate way in.
  const targeted = useRef(cwd);
  useEffect(() => {
    if (cwd === null || cwd === targeted.current) return;
    targeted.current = cwd;
    const key = focusedRef.current;
    const pane = panesRef.current.find((p) => p.key === key);
    if (!pane || (pane.liveCwd ?? pane.cwd) === cwd) return;
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
  }, [cwd, restart, patch]);

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
                  <span aria-hidden="true">📂</span> {label}
                </span>
                {dir && <span className="term-cwd">{dir}</span>}
                <span className="spacer" />
                {!p.exited && cwd !== null && cwd !== p.cwd && (
                  <button
                    className="ghost"
                    onClick={() => restart(p.key)}
                    title={cwd}
                  >
                    ↻ {t.restartHere}
                  </button>
                )}
                {p.exited && (
                  <button className="ghost" onClick={() => restart(p.key)}>
                    ↻ {t.restartShell}
                  </button>
                )}
                <button
                  className="ghost icon"
                  onClick={() => split(p.key)}
                  title={t.splitBelow}
                  aria-label={t.splitBelow}
                  aria-keyshortcuts="Meta+D"
                >
                  ＋
                </button>
                {multi && (
                  <button
                    className="ghost icon"
                    onClick={() => close(p.key)}
                    title={t.closePane}
                    aria-label={t.closePane}
                  >
                    ✕
                  </button>
                )}
              </header>
              <TerminalView
                key={p.gen}
                paneKey={p.key}
                cwd={p.cwd}
                bindApi={bindApi}
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
