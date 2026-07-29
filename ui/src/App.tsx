import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as api from "./api";
import TerminalPanel, { type ActivityProbe } from "./TerminalPanel";
import ResizeHandle, { type WidthSpec } from "./ResizeHandle";
import Picker from "./Picker";
import Icon from "./Icon";
import { LANGS, detectLang, messages, saveLang, type Lang } from "./i18n";
import {
  EDITOR_NAMES,
  detectEditor,
  saveEditor,
  type Editor,
} from "./editors";
import {
  applyAppearance,
  detectTheme,
  resolveAppearance,
  saveTheme,
  syncWindowTheme,
  watchSystemAppearance,
  type Appearance,
  type Theme,
} from "./theme";
import "./App.css";

function short(id: string): string {
  return id.slice(0, 8);
}

// Approval left the domain (it was mechanically inert — the gate treated
// "laid" and "approved" identically). The core already aliases the stored
// `approved` onto `laid` as it reads a goal, so this is the belt to that
// braces: the status string also becomes a CSS class and a filter key here, and
// an unknown one must not fan out into three silently wrong renderings.
function itemStatus(status: string): string {
  return status === "approved" ? "laid" : status;
}

// Only macOS gets the overlaid title bar (tauri's titleBarStyle is macOS-only),
// so only there must the strip leave room for the traffic lights.
const IS_MAC = /Mac/i.test(navigator.userAgent);

const SIDEBAR_W: WidthSpec = { def: 280, min: 180, max: 480 };
const DETAIL_W: WidthSpec = { def: 440, min: 300, max: 720 };

// Panel width persisted to localStorage; always clamped to the spec.
function usePanelWidth(key: string, spec: WidthSpec) {
  const clamp = (v: number) =>
    Math.min(spec.max, Math.max(spec.min, Math.round(v)));
  const [w, setW] = useState(() => {
    const saved = Number(localStorage.getItem(key));
    return Number.isFinite(saved) && saved > 0 ? clamp(saved) : spec.def;
  });
  const set = (next: number) => {
    const v = clamp(next);
    setW(v);
    localStorage.setItem(key, String(v));
  };
  return [w, set] as const;
}

function basename(dir: string): string {
  return (
    dir
      .replace(/[\\/]+$/, "")
      .split(/[\\/]/)
      .pop() || dir
  );
}

// The terminal a correction would go to: the newest session that recorded one
// (after a resume a goal can carry several, and the live one is the last we
// heard from). Deliberately the same pick send_to_agent makes on the Rust side —
// asking about one pane and typing into another would be worse than not asking.
function livePane(goal: api.Goal): number | null {
  return goal.sessions.reduce<number | null>(
    (found, s) => s.pane ?? found,
    null,
  );
}

// Is this goal's agent gone for good? Liveness is computed, never stored: a goal
// is reachable exactly while one of the panes it recorded is still a pane the
// terminal panel has, with a live shell. Once that pane is really gone so is the
// session — and a Claude Code session id never comes back (/clear and resume both
// mint a new one, which by design gets its own goal).
//
// Quitting the app is no longer one of the ways a pane goes away: the shells live
// in a daemon that outlives it, and the panel rebuilds a pane per surviving shell
// on startup, so a restored pane reads as alive here. That is the same premise
// the core's startup sweep now works from (Store::account_ended_panes takes the
// surviving pane ids) — the two must agree, or the banner and the goal's status
// would tell the human opposite stories about the same run.
//
// Three deliberate asymmetries, all erring toward not declaring a live run dead.
// A goal with NO pane recorded answers false: that session came from a shell
// Witnos never spawned and may be alive right now. Every pane must be gone, not
// just the newest one livePane picks for sending. And a probe that is null —
// there is no panel, or it has not yet asked what survived — answers false too:
// not knowing is not the same as knowing it ended.
function sessionGone(goal: api.Goal, probe: ActivityProbe | null): boolean {
  const panes = goal.sessions
    .map((s) => s.pane)
    .filter((p): p is number => p !== null && p !== undefined);
  if (!probe || panes.length === 0) return false;
  return panes.every((p) => probe(p) === "gone");
}

export default function App() {
  const [goals, setGoals] = useState<api.GoalSummary[]>([]);
  const [projects, setProjects] = useState<api.ProjectSummary[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const [selProject, setSelProject] = useState<string | null>(null);
  const [goal, setGoal] = useState<api.Goal | null>(null);
  // Tagged by who raised it: the 1.5s poll must be able to clear its own
  // "core unreachable" complaint the moment the core answers again, without
  // wiping the error your last click produced — which you would otherwise
  // never get to read.
  const [err, setErr] = useState<{ text: string; poll: boolean } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem("witnos.sidebar_collapsed") === "1",
  );
  const [detailCollapsed, setDetailCollapsed] = useState(
    () => localStorage.getItem("witnos.detail_collapsed") === "1",
  );
  const [sidebarW, setSidebarW] = usePanelWidth(
    "witnos.sidebar_width",
    SIDEBAR_W,
  );
  const [detailW, setDetailW] = usePanelWidth("witnos.detail_width", DETAIL_W);
  const [resizing, setResizing] = useState(false);
  // Fullscreen hides the traffic lights, and the strip must stop reserving
  // room for them (see --tb-lead).
  const [fullscreen, setFullscreen] = useState(false);
  // What the workspace (the middle pane) is showing: the terminal, or the
  // settings view. The terminal stays mounted underneath so its shell
  // session survives the switch.
  const [workspaceView, setWorkspaceView] = useState<"terminal" | "settings">(
    "terminal",
  );
  const [showArchive, setShowArchive] = useState(false);
  const [lang, setLang] = useState<Lang>(detectLang);
  const [editor, setEditor] = useState<Editor>(detectEditor);
  const [theme, setTheme] = useState<Theme>(detectTheme);
  // What is painting right now. main.tsx already applied it once, before the
  // first frame; this state is what keeps it in step afterwards.
  const [appearance, setAppearance] = useState<Appearance>(() =>
    resolveAppearance(theme),
  );
  const t = messages[lang];
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    goal?: api.GoalSummary;
    project?: api.ProjectSummary;
  } | null>(null);
  // In-app confirm: window.confirm() is a silent no-op in wry's WKWebView
  // (no WKUIDelegate confirm panel), so never use it here.
  const [confirmBox, setConfirmBox] = useState<{
    message: string;
    label: string;
    action: () => void;
  } | null>(null);

  // Asked once, at click time (see pushToAgent), never during render: a working
  // agent retitles its pane constantly, and none of that belongs in this tree's
  // render path. Null whenever the terminal panel is unmounted.
  const paneActivity = useRef<ActivityProbe | null>(null);
  const bindActivity = useCallback((probe: ActivityProbe | null) => {
    paneActivity.current = probe;
  }, []);
  // The goal we last sampled as having no live pane left (see sessionGone). Its
  // agent is unreachable for good, which is what the detail pane has to say and
  // what makes the levers that only work on a future stop dishonest there.
  const [goneFor, setGoneFor] = useState<string | null>(null);

  // Every goal id this pane has already listed. A goal appears in exactly one
  // way — a human issued it (in auto mode, the first prompt of a session in
  // Witnos's own terminal creates one) — so an id that was not here a moment ago
  // is the thing they just did, and selecting it is what they would click next.
  // Null until the first poll answers: at launch every goal is new to this ref,
  // and none of them is new to the human.
  const seen = useRef<Set<string> | null>(null);

  // Which evidence the user is looking at right now — the honest source of
  // the origin instrumentation (the strong-bet (b) signal).
  const [viewing, setViewing] = useState<string | null>(null);
  // Items whose evidence originals were opened this session (per-ruling flag).
  const drilled = useRef<Set<string>>(new Set());

  const [newClaim, setNewClaim] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [editClaim, setEditClaim] = useState("");
  const [editCheck, setEditCheck] = useState("");

  const selectGoal = useCallback((id: string) => {
    setSel(id);
    setSelProject(null);
    setGoal(null);
    setViewing(null);
    setEditing(null);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const list = await api.listGoals();
      setGoals(list);
      setProjects(await api.listAutoProjects());
      // Newest first (the store sorts by creation), so the freshest issuance
      // wins if a poll ever catches two at once.
      const fresh = seen.current
        ? list.find((g) => !seen.current!.has(g.id))
        : undefined;
      seen.current = new Set(list.map((g) => g.id));
      if (fresh) {
        // Selecting a row the sidebar isn't showing would be a selection the
        // human can't see.
        setShowArchive(false);
        selectGoal(fresh.id);
      }
      // The one just selected, not the one selected when this poll started:
      // `sel` is a render old here, and loading the goal we are leaving would
      // paint the wrong contract until the next beat.
      const want = fresh?.id ?? sel;
      if (want) {
        const g = await api.getGoal(want);
        setGoal(g);
        // Sampled on this beat rather than during render: the probe reads live
        // refs that a working agent churns constantly (see paneActivity), and
        // panes only appear or die on a human action anyway. Keyed by goal id
        // because the sample lags a selection by up to one tick, and a stale
        // "its session ended" must never be painted onto another goal.
        setGoneFor(sessionGone(g, paneActivity.current) ? g.id : null);
      }
      setErr((prev) => (prev?.poll ? null : prev));
    } catch (e) {
      setErr({ text: String(e), poll: true });
    }
  }, [sel, selectGoal]);

  /** An error the human caused by clicking something: stays until dismissed. */
  const failed = (e: unknown) => setErr({ text: String(e), poll: false });

  useEffect(() => {
    refresh();
    const iv = setInterval(refresh, 1500);
    return () => clearInterval(iv);
  }, [refresh]);

  useEffect(() => {
    if (!menu && !confirmBox && workspaceView !== "settings") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setMenu(null);
        setConfirmBox(null);
        setWorkspaceView("terminal");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu, confirmBox, workspaceView]);

  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  // Entering or leaving fullscreen is a resize, so one listener covers both;
  // ask the window rather than infer it from dimensions, which can't tell
  // fullscreen from a window merely filling the screen.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let stale = false;
    try {
      const win = getCurrentWindow();
      const sync = () =>
        win
          .isFullscreen()
          .then(setFullscreen)
          .catch(() => {});
      sync();
      win
        .onResized(sync)
        .then((f) => {
          if (stale) f();
          else unlisten = f;
        })
        .catch(() => {});
    } catch {
      // Not running inside Tauri: the strip reserves nothing anyway (IS_MAC
      // gates --tb-lead), so there is nothing to keep in sync.
    }
    return () => {
      stale = true;
      unlisten?.();
    };
  }, []);

  const changeLang = (l: Lang) => {
    saveLang(l);
    setLang(l);
  };

  const changeEditor = (e: Editor) => {
    saveEditor(e);
    setEditor(e);
  };

  useEffect(() => {
    applyAppearance(appearance);
  }, [appearance]);

  // The frame follows the pinned theme, and while the preference is "system" the
  // OS stays in charge — flipping it mid-session repaints without a restart.
  useEffect(() => {
    syncWindowTheme(theme);
    if (theme !== "system") return;
    return watchSystemAppearance(setAppearance);
  }, [theme]);

  const changeTheme = (v: Theme) => {
    saveTheme(v);
    setTheme(v);
    setAppearance(resolveAppearance(v));
  };

  const selectProject = (dir: string) => {
    setSelProject(dir);
    setSel(null);
    setGoal(null);
    setViewing(null);
    setEditing(null);
  };

  const watchProject = async () => {
    try {
      const dir = await api.pickProjectDir();
      if (!dir) return;
      await api.addAutoProject(dir);
      setNotice(t.projectAddedNotice);
      selectProject(dir);
      refresh();
    } catch (e) {
      failed(e);
    }
  };

  const removeProject = (p: api.ProjectSummary) =>
    setConfirmBox({
      message: t.confirmRemoveProject(p.dir),
      label: t.removeProject,
      action: async () => {
        await api.removeAutoProject(p.dir);
        if (selProject === p.dir) setSelProject(null);
        refresh();
      },
    });

  const goalRow = (g: api.GoalSummary, nested: boolean) => (
    <button
      key={g.id}
      className={`goal-row ${nested ? "nested" : ""} ${sel === g.id ? "sel" : ""}`}
      onClick={() => selectGoal(g.id)}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        setMenu({ x: e.clientX, y: e.clientY, goal: g });
      }}
    >
      <span className="goal-title">
        {g.title}
        {g.status === "awaiting_rulings" && (
          <span className="needs-dot" title={t.needsRulingDot} />
        )}
      </span>
      <span className="goal-meta">
        {t.goalStatus(g.status)}
        {g.watching && (
          <>
            {" · "}
            <Icon name="eye" size={12} label={t.watchingMark} />
          </>
        )}
        {g.strong_bet_count > 0 ? ` · (b)×${g.strong_bet_count}` : ""}
      </span>
    </button>
  );

  // Collapsible project groups (Finder-like): clicking the project row
  // selects it (terminal cwd) and folds/unfolds its goals. Folded dirs
  // persist across launches.
  const [collapsedDirs, setCollapsedDirs] = useState<Set<string>>(() => {
    try {
      return new Set<string>(
        JSON.parse(localStorage.getItem("witnos.collapsed_projects") ?? "[]"),
      );
    } catch {
      return new Set<string>();
    }
  });
  const toggleDir = (dir: string) => {
    setCollapsedDirs((prev) => {
      const next = new Set(prev);
      if (!next.delete(dir)) next.add(dir);
      localStorage.setItem(
        "witnos.collapsed_projects",
        JSON.stringify([...next]),
      );
      return next;
    });
  };

  const projGroup = (
    dir: string,
    dirGoals: api.GoalSummary[],
    onCtxMenu?: (e: MouseEvent<HTMLButtonElement>) => void,
    meta?: string,
  ) => {
    const folded = collapsedDirs.has(dir);
    // A folded group must not swallow the "there is new evidence" signal
    // (principle 6): the dot climbs up to the project row.
    const needsDot =
      folded && dirGoals.some((g) => g.status === "awaiting_rulings");
    return (
      <div key={dir} className="proj-group">
        <button
          className={`goal-row ${selProject === dir ? "sel" : ""}`}
          title={dir}
          aria-expanded={!folded}
          onClick={() => {
            selectProject(dir);
            toggleDir(dir);
          }}
          onContextMenu={onCtxMenu}
        >
          <span className="goal-title proj-line">
            <Icon name={folded ? "folder" : "folder-open"} />
            <span className="proj-name">{basename(dir)}</span>
            {needsDot && (
              <span className="needs-dot right" title={t.needsRulingDot} />
            )}
          </span>
          {meta && <span className="goal-meta">{meta}</span>}
        </button>
        {!folded && dirGoals.map((g) => goalRow(g, true))}
      </div>
    );
  };

  // Both live in the sidebar's context menu, on the row itself: they act on a
  // goal, not on whatever the detail pane happens to be showing, so they name
  // the one they mean.
  const closeGoal = (g: api.GoalSummary) =>
    setConfirmBox({
      message: t.confirmCloseGoal(g.title),
      label: t.closeGoal,
      action: () => {
        api.closeGoal(g.id).then(refresh).catch(failed);
      },
    });

  const removeGoal = (g: api.GoalSummary) =>
    setConfirmBox({
      message: t.confirmDeleteGoal(g.title),
      label: t.delete,
      action: async () => {
        await api.deleteGoal(g.id);
        if (sel === g.id) {
          setSel(null);
          setGoal(null);
        }
        refresh();
      },
    });

  // Quick-add takes only the criterion text — no check field. How to verify
  // is the agent's to propose (interpretation + evidence, human rules on it),
  // so an empty check is a valid contract item, not missing data.
  const addItem = async () => {
    if (!goal || !newClaim.trim()) return;
    await api.addItem(goal.id, newClaim.trim(), "", viewing);
    setNewClaim("");
    refresh();
  };

  const saveEdit = async (itemId: string) => {
    if (!goal) return;
    await api.editItem(goal.id, itemId, editClaim, editCheck);
    setEditing(null);
    refresh();
  };

  // The two ways to disagree. Both bump the contract version, so both reach a
  // running agent through the delivery channel — and a failure has to say so,
  // otherwise the click just looks like it did nothing.
  const sendItemBack = async (item: api.Item) => {
    if (!goal) return;
    try {
      await api.rejectItem(goal.id, item.id, drilled.current.has(item.id));
      refresh();
    } catch (e) {
      failed(e);
    }
  };

  const setWaived = async (item: api.Item, waived: boolean) => {
    if (!goal) return;
    try {
      await api.waiveItem(goal.id, item.id, waived);
      refresh();
    } catch (e) {
      failed(e);
    }
  };

  // Witnos owns the terminal the agent runs in, so the change can be typed into
  // that pane and run now, instead of waiting for the gate to catch it at the
  // agent's next stop. No note: the core composes the version line and the
  // commands, and `note` is reserved for the human's own words — there is no
  // field for them yet, and inventing prose on their behalf would only push the
  // mechanics further down the line the agent reads. All three outcomes are
  // reported, because two of them mean nothing was typed and the human must not
  // read that as delivered.
  const pushToAgent = async () => {
    if (!goal) return;
    // The politeness the core can't do: it only sees whether *something* owns
    // the pane, not whether that something is mid-thought. Interrupting a
    // working agent buys nothing anyway — the delivery channel injects the
    // delta after its next tool call, with no typing at all. Idle or unreadable
    // (see PaneActivity) both fall through and send.
    const pane = livePane(goal);
    if (pane !== null && paneActivity.current?.(pane) === "working") {
      setNotice(t.agentWorking);
      return;
    }
    try {
      const outcome = await api.sendToAgent(goal.id);
      setNotice(
        outcome === "sent"
          ? t.sentToAgent
          : outcome === "no_agent"
            ? t.agentNotRunning
            : t.agentUnbound,
      );
    } catch (e) {
      failed(e);
    }
  };

  const drill = async (item: api.Item, ev: api.Evidence, ptr: api.Pointer) => {
    if (!goal) return;
    drilled.current.add(item.id);
    setViewing(ev.id);
    try {
      await api.drillDown(goal.id, ev.id, ptr, editor);
    } catch (e) {
      // An unresolvable pointer must say so — silently opening nothing is how
      // a human ends up ruling on evidence they never actually saw.
      failed(e);
    }
  };

  const toggleSidebar = useCallback(() => {
    setCollapsed((c) => {
      localStorage.setItem("witnos.sidebar_collapsed", c ? "0" : "1");
      return !c;
    });
  }, []);

  const toggleDetail = useCallback(() => {
    setDetailCollapsed((c) => {
      localStorage.setItem("witnos.detail_collapsed", c ? "0" : "1");
      return !c;
    });
  }, []);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (!e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
      const k = e.key.toLowerCase();
      if (k === "s") {
        e.preventDefault();
        toggleSidebar();
      } else if (k === "i" && sel) {
        // ⌘I, not ⌘D: the terminal owns ⌘D (split a shell downwards), the
        // one shortcut a terminal user reaches for without thinking. Dead
        // while no goal is selected: there is no pane to toggle, and flipping
        // the stored preference from here would only surprise you later.
        e.preventDefault();
        toggleDetail();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [toggleSidebar, toggleDetail, sel]);

  const watchingCount = goals.filter((g) => g.watching).length;
  const archivedGoals = goals.filter((g) => g.status === "closed");
  const activeGoals = goals.filter((g) => g.status !== "closed");

  // Sidebar grouping: every goal sits under its project, so belonging is
  // visible at a glance. Registered (auto-watched) projects come first,
  // then dirs that only exist on goals, then project-less goals.
  const goalsByDir = new Map<string, api.GoalSummary[]>();
  const noDirGoals: api.GoalSummary[] = [];
  for (const g of activeGoals) {
    if (g.project_dir) {
      const list = goalsByDir.get(g.project_dir) ?? [];
      list.push(g);
      goalsByDir.set(g.project_dir, list);
    } else {
      noDirGoals.push(g);
    }
  }
  const registered = new Set(projects.map((p) => p.dir));
  const extraDirs = [...goalsByDir.keys()]
    .filter((d) => !registered.has(d))
    .sort();
  const needsYou = goal
    ? goal.items.filter(
        (i) => i.class.kind === "subjective" && itemStatus(i.status) === "laid",
      )
    : [];
  // The selected goal's agent session has ended. A closed goal already says the
  // stronger version of this, so it keeps that banner and this one stays quiet.
  const ended = !!goal && goal.status !== "closed" && goneFor === goal.id;
  // Every lever that only bites on a future stop is dishonest here, so the copy
  // that offers one changes with it: there is no next stop to block.
  const needsNote = ended
    ? t.needsBannerEnded(needsYou.length)
    : t.needsBanner(needsYou.length);
  // An edit of the human's own that the agent hasn't read. Not `contract_version`:
  // the agent bumps that itself every time it lays items, before it reconciles,
  // so keying on it made the banner appear during ordinary mid-run work — noise
  // by principle 4, and noise that cries wolf about the one thing the banner
  // exists to say. The gate catches these at the agent's next stop anyway; this
  // is only what makes the "or reach it now" offer appear — which is also why a
  // goal whose session is gone shows nothing: both halves of what it promises
  // (the gate will block, or you can reach the agent now) would be untrue.
  const unsynced =
    !!goal &&
    goal.status !== "closed" &&
    !ended &&
    goal.last_human_edit_version > goal.agent_synced_version;

  const originNote = viewing
    ? t.originViewing(short(viewing))
    : goal && goal.sessions.length > 0
      ? t.originMidRun
      : t.originPreRun;

  return (
    <div
      className={`app ${IS_MAC ? "mac" : ""} ${fullscreen ? "fullscreen" : ""} ${resizing ? "resizing" : ""}`}
      style={
        {
          "--sidebar-w": `${sidebarW}px`,
          "--detail-w": `${detailW}px`,
        } as CSSProperties
      }
    >
      {/* The window's title bar is overlaid on the webview, so the pane
          toggles live up here beside the traffic lights. Bare
          data-tauri-drag-region: only clicks on the strip itself drag the
          window — the buttons inside keep their clicks. */}
      <div className="titlebar" data-tauri-drag-region>
        <button
          className="sidebar-toggle"
          onClick={toggleSidebar}
          aria-label={collapsed ? t.expandSidebar : t.collapseSidebar}
          aria-expanded={!collapsed}
          aria-keyshortcuts="Meta+S"
        >
          {/* sidebar.left in the SF Symbols style; pane filled = sidebar shown */}
          <svg
            width="16"
            height="16"
            viewBox="0 0 16 16"
            fill="none"
            aria-hidden="true"
          >
            <rect
              x="1.6"
              y="2.6"
              width="12.8"
              height="10.8"
              rx="2.2"
              stroke="currentColor"
              strokeWidth="1.2"
            />
            <path d="M6.2 2.6v10.8" stroke="currentColor" strokeWidth="1.2" />
            {!collapsed && (
              <rect
                x="2.9"
                y="3.9"
                width="2.2"
                height="8.2"
                rx="0.9"
                fill="currentColor"
                opacity="0.55"
              />
            )}
          </svg>
          <span className="toggle-tip" aria-hidden="true">
            {collapsed ? t.expandSidebar : t.collapseSidebar}
            <kbd>⌘S</kbd>
          </span>
        </button>
        {collapsed && watchingCount > 0 && (
          <span
            className="tb-count"
            title={t.watchingCount(watchingCount)}
            data-tauri-drag-region
          >
            <Icon name="eye" size={12} /> {watchingCount}
          </span>
        )}
        <div className="tb-right" data-tauri-drag-region>
          {workspaceView !== "settings" &&
            detailCollapsed &&
            needsYou.length > 0 && (
              <span
                className="tb-count needs"
                title={needsNote}
                data-tauri-drag-region
              >
                <Icon name="scale" size={12} /> {needsYou.length}
              </span>
            )}
          {workspaceView !== "settings" && sel && (
            <button
              className="detail-toggle"
              onClick={toggleDetail}
              aria-label={detailCollapsed ? t.expandDetail : t.collapseDetail}
              aria-expanded={!detailCollapsed}
              aria-keyshortcuts="Meta+I"
            >
              {/* sidebar.right in the SF Symbols style; pane filled = detail shown */}
              <svg
                width="16"
                height="16"
                viewBox="0 0 16 16"
                fill="none"
                aria-hidden="true"
              >
                <rect
                  x="1.6"
                  y="2.6"
                  width="12.8"
                  height="10.8"
                  rx="2.2"
                  stroke="currentColor"
                  strokeWidth="1.2"
                />
                <path
                  d="M9.8 2.6v10.8"
                  stroke="currentColor"
                  strokeWidth="1.2"
                />
                {!detailCollapsed && (
                  <rect
                    x="10.9"
                    y="3.9"
                    width="2.2"
                    height="8.2"
                    rx="0.9"
                    fill="currentColor"
                    opacity="0.55"
                  />
                )}
              </svg>
              <span className="toggle-tip" aria-hidden="true">
                {detailCollapsed ? t.expandDetail : t.collapseDetail}
                <kbd>⌘I</kbd>
              </span>
            </button>
          )}
        </div>
      </div>

      <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
        <header>
          <div className="sidebar-title">
            <h1>witnos</h1>
            {/* Only the armed state earns a line here: "watching N goals" is
                what explains a deliberate stall. Its absence says nothing is
                watched — spelling that out was chrome for the empty case. */}
            {watchingCount > 0 && (
              <div className="watching">{t.watchingCount(watchingCount)}</div>
            )}
          </div>
        </header>
        <div className="goal-list">
          {!showArchive && (
            <div className="proj-section">
              {/* The add affordance lives in the heading (shown on hover,
                  ChatGPT-style), so the heading renders even with zero
                  projects — otherwise the first one couldn't be added. */}
              <div className="archive-head proj-head">
                <span>{t.projectsHeading}</span>
                <button
                  className="proj-add"
                  onClick={watchProject}
                  title={t.watchProjectAuto}
                  aria-label={t.watchProjectAuto}
                >
                  <svg
                    width="14"
                    height="14"
                    viewBox="0 0 14 14"
                    fill="none"
                    aria-hidden="true"
                  >
                    <path
                      d="M7 2.5v9M2.5 7h9"
                      stroke="currentColor"
                      strokeWidth="1.4"
                      strokeLinecap="round"
                    />
                  </svg>
                </button>
              </div>
              {projects.map((p) =>
                projGroup(p.dir, goalsByDir.get(p.dir) ?? [], (e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  setMenu({ x: e.clientX, y: e.clientY, project: p });
                }),
              )}
              {extraDirs.map((dir) =>
                projGroup(
                  dir,
                  goalsByDir.get(dir)!,
                  undefined,
                  t.projectNotWatched,
                ),
              )}
            </div>
          )}
          {!showArchive && noDirGoals.length > 0 && (
            <>
              <div className="archive-head">{t.noProjectHeading}</div>
              {noDirGoals.map((g) => goalRow(g, false))}
            </>
          )}
          {showArchive && (
            <div className="archive-head">
              {t.archivedHeading(archivedGoals.length)}
            </div>
          )}
          {showArchive && archivedGoals.map((g) => goalRow(g, false))}
        </div>
        <div className="sidebar-footer">
          <button
            className={`settings-btn ${showArchive ? "active" : ""}`}
            onClick={() => setShowArchive((a) => !a)}
            title={showArchive ? t.hideArchive : t.showArchive}
            aria-label={showArchive ? t.hideArchive : t.showArchive}
            aria-pressed={showArchive}
          >
            <Icon name="archive" />
            <span className="settings-label">
              {t.archive}
              {archivedGoals.length > 0 ? ` (${archivedGoals.length})` : ""}
            </span>
          </button>
          <button
            className={`settings-btn ${workspaceView === "settings" ? "active" : ""}`}
            onClick={() =>
              setWorkspaceView((v) =>
                v === "settings" ? "terminal" : "settings",
              )
            }
            title={t.settings}
            aria-label={t.settings}
            aria-pressed={workspaceView === "settings"}
          >
            <Icon name="settings" />
            <span className="settings-label">{t.settings}</span>
          </button>
        </div>
      </aside>

      {!collapsed && (
        <ResizeHandle
          className="for-sidebar"
          label={t.resizeSidebar}
          width={sidebarW}
          spec={SIDEBAR_W}
          dir={1}
          onWidth={setSidebarW}
          onResizing={setResizing}
        />
      )}

      {/* Feedback belongs to the window, not to a pane. These used to sit in the
          sidebar, where ⌘S hid them: you could send a correction to the agent
          and be told nothing at all. Click either one to dismiss it. */}
      <div className="toasts" role="status" aria-live="polite">
        {err && (
          <button
            className="toast bad"
            onClick={() => setErr(null)}
            title={t.clear}
          >
            {err.text}
          </button>
        )}
        {notice && (
          <button
            className="toast"
            onClick={() => setNotice(null)}
            title={t.clear}
          >
            {notice}
          </button>
        )}
      </div>

      {menu && (
        <div
          className="ctx-backdrop"
          onClick={() => setMenu(null)}
          onContextMenu={(e) => {
            e.preventDefault();
            setMenu(null);
          }}
        >
          <div
            className="ctx-menu"
            role="menu"
            style={{
              left: Math.min(menu.x, window.innerWidth - 170),
              top: Math.min(menu.y, window.innerHeight - 48),
            }}
          >
            {menu.goal && menu.goal.status !== "closed" && (
              <button
                className="ctx-item"
                role="menuitem"
                onClick={() => {
                  const g = menu.goal!;
                  setMenu(null);
                  closeGoal(g);
                }}
              >
                {t.closeGoal}…
              </button>
            )}
            {menu.goal && (
              <button
                className="ctx-item danger-item"
                role="menuitem"
                onClick={() => {
                  const g = menu.goal!;
                  setMenu(null);
                  removeGoal(g);
                }}
              >
                {t.deleteGoalMenu}
              </button>
            )}
            {menu.project && (
              <button
                className="ctx-item danger-item"
                role="menuitem"
                onClick={() => {
                  const p = menu.project!;
                  setMenu(null);
                  removeProject(p);
                }}
              >
                {t.removeProject}…
              </button>
            )}
          </div>
        </div>
      )}

      {confirmBox && (
        <div className="modal-backdrop" onClick={() => setConfirmBox(null)}>
          <div
            className="modal confirm"
            role="alertdialog"
            aria-label={t.confirm}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="confirm-msg">{confirmBox.message}</div>
            <div className="confirm-actions">
              <button className="ghost" onClick={() => setConfirmBox(null)}>
                {t.cancel}
              </button>
              <button
                className="danger"
                onClick={() => {
                  const act = confirmBox.action;
                  setConfirmBox(null);
                  act();
                }}
              >
                {confirmBox.label}
              </button>
            </div>
          </div>
        </div>
      )}

      <main className="workspace">
        <TerminalPanel
          cwd={selProject ?? goal?.project_dir ?? null}
          t={t}
          appearance={appearance}
          bindActivity={bindActivity}
          hidden={workspaceView !== "terminal"}
        />
        {workspaceView === "settings" && (
          <section className="settings-pane" aria-label={t.settings}>
            <header className="settings-head">
              <span className="settings-title">{t.settings}</span>
              <span className="spacer" />
              <button
                className="ghost icon"
                onClick={() => setWorkspaceView("terminal")}
                aria-label={t.closeSettings}
              >
                <Icon name="x" />
              </button>
            </header>
            <div className="settings-body">
              <div className="setting-row">
                <span>{t.language}</span>
                <Picker
                  value={lang}
                  onChange={changeLang}
                  searchPlaceholder={t.searchLanguage}
                  noMatchesLabel={t.noMatches}
                  options={LANGS.map((l) => ({
                    value: l.value,
                    primary: l.native,
                    secondary:
                      l.names[lang] !== l.native ? l.names[lang] : undefined,
                    keywords: Object.values(l.names),
                  }))}
                />
              </div>
              <div className="setting-row">
                <span>{t.appearance}</span>
                <Picker
                  value={theme}
                  onChange={changeTheme}
                  options={[
                    { value: "system" as Theme, primary: t.appearanceSystem },
                    { value: "light" as Theme, primary: t.appearanceLight },
                    { value: "dark" as Theme, primary: t.appearanceDark },
                  ]}
                />
              </div>
              <div className="setting-row">
                <div className="setting-label">
                  <span>{t.openFilesWith}</span>
                  <span className="setting-hint">{t.openFilesWithHint}</span>
                </div>
                <Picker
                  value={editor}
                  onChange={changeEditor}
                  options={[
                    { value: "system" as Editor, primary: t.editorSystem },
                    ...EDITOR_NAMES.map(([value, primary]) => ({
                      value,
                      primary,
                    })),
                  ]}
                />
              </div>
            </div>
          </section>
        )}
      </main>

      {/* No goal, no pane. This pane is a contract or it is nothing: with
          nothing selected it used to stand there full-width explaining itself,
          which is a third of the window spent on a sentence you read once. The
          toggle goes with it — an empty pane is not worth a control, and ⌘I
          must not quietly flip a preference that only bites later, when a goal
          IS open. */}
      {workspaceView !== "settings" && sel && (
        <>
          {!detailCollapsed && (
            <ResizeHandle
              className="for-detail"
              label={t.resizeDetail}
              width={detailW}
              spec={DETAIL_W}
              dir={-1}
              onWidth={setDetailW}
              onResizing={setResizing}
            />
          )}
          <aside className={`detail ${detailCollapsed ? "collapsed" : ""}`}>
            <div className="detail-body">
              {/* Blank only for the moment between clicking a goal and the
                  store answering — there is no "nothing selected" state to
                  render here any more. */}
              {goal && (
                <>
                  {/* Title only. The status is on the row in the sidebar, and
                      the two states that actually change what this pane means —
                      closed, and the session gone — say so in a banner right
                      below. The project directory is the folder you picked to
                      get here; repeating the absolute path bought nothing but a
                      line of noise above the contract. */}
                  <header className="goal-head">
                    <h2>{goal.title}</h2>
                  </header>

                  {goal.status === "closed" && (
                    <div className="banner closed">{t.closedBanner}</div>
                  )}
                  {/* The session died with its pane and no agent will read this
                      contract again — same boundary as a closed goal (principle
                      5), reached without anyone deciding it, so it is said in the
                      same voice. The contract stays exactly as it is: editing and
                      waiving still work, and re-issuing the goal is the way out. */}
                  {ended && <div className="banner ended">{t.endedBanner}</div>}
                  {needsYou.length > 0 && (
                    <div className="banner needs">{needsNote}</div>
                  )}
                  {/* The versions the agent is behind by stay out of this line:
                      they name the gap precisely, but no other surface in the app
                      shows a v-number, so there is nothing for the reader to
                      anchor them to. The typed nudge still carries them — that
                      end of the wire is the agent's. */}
                  {unsynced && (
                    <div className="banner unsynced">
                      <span>{t.unsyncedBanner}</span>
                      <button onClick={pushToAgent}>{t.sendToAgent}</button>
                    </div>
                  )}

                  <section className="items">
                    {goal.items.map((item) => {
                      const evs = goal.evidence.filter(
                        (e) => e.item_id === item.id,
                      );
                      const reinterpreted =
                        item.interpretation_history.length > 1;
                      const st = itemStatus(item.status);
                      const waived = st === "waived";
                      return (
                        <article
                          key={item.id}
                          className={`item ${waived ? "waived" : ""} ${
                            st === "laid" && item.class.kind === "subjective"
                              ? "attention"
                              : ""
                          }`}
                        >
                          <div className="item-head">
                            <span className={`chip ${item.class.kind}`}>
                              {t.itemClass(item.class.kind)}
                            </span>
                            <span className={`chip status-${st}`}>
                              {t.itemStatus(st)}
                            </span>
                            {reinterpreted && (
                              <span
                                className="chip reinterpreted"
                                title={t.reinterpretedTitle}
                              >
                                {t.reinterpreted(
                                  item.interpretation_history.length - 1,
                                )}
                              </span>
                            )}
                            <span
                              className="chip origin"
                              title={t.originTitle(
                                t.originKind(item.origin.kind),
                              )}
                            >
                              {t.originKind(item.origin.kind)}
                            </span>
                            <span className="spacer" />
                            {/* A waived item offers only the way back: editing
                                a criterion nobody checks is busywork. */}
                            {goal.status !== "closed" &&
                              editing !== item.id &&
                              (waived ? (
                                <button
                                  className="ghost"
                                  onClick={() => setWaived(item, false)}
                                >
                                  {t.unwaive}
                                </button>
                              ) : (
                                <>
                                  <button
                                    className="ghost"
                                    onClick={() => {
                                      setEditing(item.id);
                                      setEditClaim(item.claim);
                                      setEditCheck(item.check);
                                    }}
                                  >
                                    {t.edit}
                                  </button>
                                  <button
                                    className="ghost icon"
                                    title={t.waiveTitle}
                                    aria-label={t.waive}
                                    onClick={() => setWaived(item, true)}
                                  >
                                    <Icon name="x" size={12} />
                                  </button>
                                </>
                              ))}
                          </div>

                          {editing === item.id ? (
                            <div className="edit-form">
                              <input
                                placeholder={t.claimPlaceholder}
                                value={editClaim}
                                onChange={(e) => setEditClaim(e.target.value)}
                              />
                              <input
                                placeholder={t.checkPlaceholder}
                                value={editCheck}
                                onChange={(e) => setEditCheck(e.target.value)}
                              />
                              <div>
                                <button onClick={() => saveEdit(item.id)}>
                                  {t.saveReopens}
                                </button>
                                <button
                                  className="ghost"
                                  onClick={() => setEditing(null)}
                                >
                                  {t.cancel}
                                </button>
                              </div>
                            </div>
                          ) : (
                            <>
                              <div className="claim">{item.claim}</div>
                              {item.check && (
                                <div className="check">
                                  {t.checkLine(item.check)}
                                </div>
                              )}
                            </>
                          )}

                          {item.interpretation && (
                            <div className="interp">
                              <span className="interp-label">
                                {t.agentReadsThisAs}
                              </span>{" "}
                              {item.interpretation}
                            </div>
                          )}

                          {evs.map((ev) => {
                            const stale =
                              ev.against_version < item.last_edited_version;
                            return (
                              <div
                                key={ev.id}
                                className={`evidence ${viewing === ev.id ? "viewing" : ""}`}
                                onClick={() => setViewing(ev.id)}
                              >
                                <div className="ev-head">
                                  <span className="ev-conclusion">
                                    {ev.conclusion}
                                  </span>
                                  {stale && (
                                    <span
                                      className="chip stale"
                                      title={t.staleTitle}
                                    >
                                      {t.stale}
                                    </span>
                                  )}
                                </div>
                                <div className="ev-basis">{ev.basis}</div>
                                <div className="ev-prov">
                                  {ev.provenance.map((p, idx) => (
                                    <button
                                      key={idx}
                                      className="prov"
                                      title={t.provTitle}
                                      onClick={(e) => {
                                        e.stopPropagation();
                                        drill(item, ev, p);
                                      }}
                                    >
                                      <Icon
                                        name={
                                          p.kind === "file"
                                            ? "file-text"
                                            : p.kind === "url"
                                              ? "external-link"
                                              : "square-terminal"
                                        }
                                        size={12}
                                      />{" "}
                                      {p.kind === "file"
                                        ? `${p.path}${p.lines ? `:${p.lines}` : ""}`
                                        : p.kind === "url"
                                          ? p.url
                                          : p.cmd}
                                    </button>
                                  ))}
                                </div>
                              </div>
                            );
                          })}

                          {/* No approve button: the agent's work is presumed
                              correct, so the human only acts on disagreement.
                              The line of prose that used to sit above this
                              button — "the agent isn't waiting for you" — is
                              gone: the goal's own banner says it once, and
                              repeating it under every subjective item is the
                              reading load principle 4 exists to cut.
                              Gone with the session, too: sending an item back
                              does its whole work at a stop that will never come,
                              so offering it would be theatre. Edit and waive
                              stay — the contract is still worth curating, not
                              least when re-issuing the goal is the next move. */}
                          {item.class.kind === "subjective" &&
                            ["laid", "rejected"].includes(st) &&
                            goal.status !== "closed" &&
                            !ended && (
                              <div className="ruling">
                                <button
                                  className={`reject ${st === "rejected" ? "active" : ""}`}
                                  title={t.sendItBackTitle}
                                  onClick={() => sendItemBack(item)}
                                >
                                  <Icon name="corner-up-left" size={12} />{" "}
                                  {t.sendItBack}
                                </button>
                              </div>
                            )}
                          {waived && (
                            <div className="waived-note">{t.waivedNote}</div>
                          )}
                        </article>
                      );
                    })}
                  </section>

                  {goal.status !== "closed" && (
                    <section className="add-item">
                      <h3>{t.addItemHeading}</h3>
                      <input
                        placeholder={t.claimPlaceholder}
                        value={newClaim}
                        onChange={(e) => setNewClaim(e.target.value)}
                        onKeyDown={(e) => e.key === "Enter" && addItem()}
                      />
                      <div className="add-row">
                        <button onClick={addItem}>{t.addSubjective}</button>
                        <span className="origin-note">
                          {t.recordedAs(originNote)}
                          {viewing && (
                            <button
                              className="ghost"
                              onClick={() => setViewing(null)}
                            >
                              {t.clear}
                            </button>
                          )}
                        </span>
                      </div>
                    </section>
                  )}
                </>
              )}
            </div>
          </aside>
        </>
      )}
    </div>
  );
}
