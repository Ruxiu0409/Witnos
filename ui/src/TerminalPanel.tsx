import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Messages } from "./i18n";
import "@xterm/xterm/css/xterm.css";

// Fresh id per mount: StrictMode's mount→unmount→mount must not let two
// shells share one id (spawn/kill invokes may resolve out of order).
let nextId = Math.floor(Math.random() * 0x7fffffff);

const THEME = {
  background: "#14151a", // --bg
  foreground: "#c9ccd6", // --text
  cursor: "#7aa2f7", // --accent
  selectionBackground: "#33364280",
};

function TerminalView({
  cwd,
  onExit,
  t,
}: {
  cwd: string | null;
  onExit: () => void;
  t: Messages;
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  // Messages written into the terminal happen inside the mount-once effect;
  // read through a ref so they use the language current at write time.
  const tRef = useRef(t);
  tRef.current = t;

  useEffect(() => {
    const id = nextId++;
    const term = new Terminal({
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 13,
      theme: THEME,
      cursorBlink: true,
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(boxRef.current!);
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;

    let alive = true;
    invoke("term_spawn", { id, cols: term.cols, rows: term.rows, cwd }).catch(
      (e) =>
        term.write(`\r\n\x1b[31m${tRef.current.shellStartFailed(String(e))}\x1b[0m\r\n`),
    );

    term.onData((data) => {
      invoke("term_write", { id, data }).catch(() => {});
    });

    const unOut = listen<{ id: number; data: number[] }>(
      "term:output",
      (e) => {
        if (e.payload.id === id) term.write(new Uint8Array(e.payload.data));
      },
    );
    const unExit = listen<number>("term:exit", (e) => {
      if (e.payload !== id || !alive) return;
      term.write(`\r\n\x1b[2m${tRef.current.shellExited}\x1b[0m\r\n`);
      onExit();
    });

    const ro = new ResizeObserver(() => {
      if (!boxRef.current || boxRef.current.clientHeight === 0) return;
      fit.fit();
      invoke("term_resize", { id, cols: term.cols, rows: term.rows }).catch(
        () => {},
      );
    });
    ro.observe(boxRef.current!);
    term.focus();

    return () => {
      alive = false;
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

  // Refit + refocus when the panel becomes visible again.
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const obs = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        requestAnimationFrame(() => {
          fitRef.current?.fit();
          termRef.current?.focus();
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
  hidden = false,
}: {
  cwd: string | null;
  t: Messages;
  // Hide instead of unmount: the shell must survive workspace view switches.
  hidden?: boolean;
}) {
  const [gen, setGen] = useState(0);
  const [exited, setExited] = useState(false);
  // The cwd prop tracks the selected goal; the shell only reads it at spawn
  // time (mount / restart), so the header shows the spawn-time value, never a
  // directory the running shell isn't actually in.
  const [spawnCwd, setSpawnCwd] = useState(cwd);

  const restart = useCallback(() => {
    setSpawnCwd(cwd);
    setExited(false);
    setGen((g) => g + 1);
  }, [cwd]);

  return (
    <div className={`term-panel ${hidden ? "hidden" : ""}`}>
      <header className="term-head">
        <span className="term-title">{t.terminal}</span>
        {spawnCwd && <span className="term-cwd">{spawnCwd}</span>}
        <span className="spacer" />
        {!exited && cwd !== null && cwd !== spawnCwd && (
          <button className="ghost" onClick={restart} title={cwd}>
            ↻ {t.restartHere}
          </button>
        )}
        {exited && (
          <button className="ghost" onClick={restart}>
            ↻ {t.restartShell}
          </button>
        )}
      </header>
      <TerminalView
        key={gen}
        cwd={spawnCwd}
        onExit={() => setExited(true)}
        t={t}
      />
    </div>
  );
}
