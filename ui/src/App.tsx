import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as api from "./api";
import TerminalPanel from "./TerminalPanel";
import ResizeHandle, { type WidthSpec } from "./ResizeHandle";
import Picker from "./Picker";
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

export default function App() {
  const [goals, setGoals] = useState<api.GoalSummary[]>([]);
  const [projects, setProjects] = useState<api.ProjectSummary[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const [selProject, setSelProject] = useState<string | null>(null);
  const [goal, setGoal] = useState<api.Goal | null>(null);
  const [err, setErr] = useState<string | null>(null);
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

  // Which evidence the user is looking at right now — the honest source of
  // the origin instrumentation (the strong-bet (b) signal).
  const [viewing, setViewing] = useState<string | null>(null);
  // Items whose evidence originals were opened this session (per-ruling flag).
  const drilled = useRef<Set<string>>(new Set());

  const [newClaim, setNewClaim] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [editClaim, setEditClaim] = useState("");
  const [editCheck, setEditCheck] = useState("");

  const refresh = useCallback(async () => {
    try {
      setGoals(await api.listGoals());
      setProjects(await api.listAutoProjects());
      if (sel) setGoal(await api.getGoal(sel));
      setErr(null);
    } catch (e) {
      setErr(String(e));
    }
  }, [sel]);

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

  const selectGoal = (id: string) => {
    setSel(id);
    setSelProject(null);
    setGoal(null);
    setViewing(null);
    setEditing(null);
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
      setErr(String(e));
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
        {g.watching ? " · 👁" : ""}
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
    // A folded group must not swallow the "awaiting your ruling" signal
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
            <span aria-hidden="true">{folded ? "📁" : "📂"}</span>
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

  const rule = async (item: api.Item, approve: boolean) => {
    if (!goal) return;
    await api.ruleItem(goal.id, item.id, approve, drilled.current.has(item.id));
    refresh();
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
      setErr(String(e));
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
      } else if (k === "i") {
        // ⌘I, not ⌘D: the terminal owns ⌘D (split a shell downwards), the
        // one shortcut a terminal user reaches for without thinking.
        e.preventDefault();
        toggleDetail();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [toggleSidebar, toggleDetail]);

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
        (i) => i.class.kind === "subjective" && i.status === "laid",
      )
    : [];

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
            👁 {watchingCount}
          </span>
        )}
        <div className="tb-right" data-tauri-drag-region>
          {workspaceView !== "settings" &&
            detailCollapsed &&
            needsYou.length > 0 && (
              <span
                className="tb-count needs"
                title={t.needsBanner(needsYou.length)}
                data-tauri-drag-region
              >
                ⚖ {needsYou.length}
              </span>
            )}
          {workspaceView !== "settings" && (
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
            <div className="watching">
              {watchingCount > 0
                ? t.watchingCount(watchingCount)
                : t.watchingNone}
            </div>
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
        {err && <div className="err">{err}</div>}
        {notice && (
          <button
            className="notice"
            onClick={() => setNotice(null)}
            title={t.clear}
          >
            {notice}
          </button>
        )}
        <div className="sidebar-footer">
          <button
            className={`settings-btn ${showArchive ? "active" : ""}`}
            onClick={() => setShowArchive((a) => !a)}
            title={showArchive ? t.hideArchive : t.showArchive}
            aria-label={showArchive ? t.hideArchive : t.showArchive}
            aria-pressed={showArchive}
          >
            <span className="settings-icon">🗃</span>
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
            <span className="settings-icon">⚙</span>
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
          hidden={workspaceView !== "terminal"}
        />
        {workspaceView === "settings" && (
          <section className="settings-pane" aria-label={t.settings}>
            <header className="settings-head">
              <span className="settings-title">{t.settings}</span>
              <span className="spacer" />
              <button
                className="ghost"
                onClick={() => setWorkspaceView("terminal")}
                aria-label={t.closeSettings}
              >
                ✕
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

      {workspaceView !== "settings" && (
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
              {!goal ? (
                <div className="empty">
                  {selProject ? t.projectHint : t.selectAGoal}
                </div>
              ) : (
                <>
                  <header className="goal-head">
                    <h2>{goal.title}</h2>
                    <div className="goal-sub">
                      {t.goalStatus(goal.status)}
                      {goal.project_dir ? ` · ${goal.project_dir}` : ""}
                    </div>
                    <div className="goal-actions">
                      {goal.watching && (
                        <button
                          onClick={() => api.unwatchGoal(goal.id).then(refresh)}
                        >
                          {t.stopWatching}
                        </button>
                      )}
                      {goal.status !== "closed" && (
                        <button
                          className="danger"
                          onClick={() =>
                            setConfirmBox({
                              message: t.confirmCloseGoal,
                              label: t.closeGoal,
                              action: () =>
                                api.closeGoal(goal.id).then(refresh),
                            })
                          }
                        >
                          {t.closeGoal}
                        </button>
                      )}
                    </div>
                  </header>

                  {goal.status === "closed" && (
                    <div className="banner closed">{t.closedBanner}</div>
                  )}
                  {needsYou.length > 0 && (
                    <div className="banner needs">
                      {t.needsBanner(needsYou.length)}
                    </div>
                  )}

                  <section className="items">
                    {goal.items.map((item) => {
                      const evs = goal.evidence.filter(
                        (e) => e.item_id === item.id,
                      );
                      const reinterpreted =
                        item.interpretation_history.length > 1;
                      return (
                        <article
                          key={item.id}
                          className={`item ${
                            item.status === "laid" &&
                            item.class.kind === "subjective"
                              ? "attention"
                              : ""
                          }`}
                        >
                          <div className="item-head">
                            <span className={`chip ${item.class.kind}`}>
                              {t.itemClass(item.class.kind)}
                            </span>
                            <span className={`chip status-${item.status}`}>
                              {t.itemStatus(item.status)}
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
                            {goal.status !== "closed" &&
                              editing !== item.id && (
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
                              )}
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
                                      {p.kind === "file"
                                        ? `📄 ${p.path}${p.lines ? `:${p.lines}` : ""}`
                                        : p.kind === "url"
                                          ? `🔗 ${p.url}`
                                          : `$ ${p.cmd}`}
                                    </button>
                                  ))}
                                </div>
                              </div>
                            );
                          })}

                          {item.class.kind === "subjective" &&
                            ["laid", "approved", "rejected"].includes(
                              item.status,
                            ) &&
                            goal.status !== "closed" && (
                              <div className="ruling">
                                <button
                                  className={`approve ${item.status === "approved" ? "active" : ""}`}
                                  onClick={() => rule(item, true)}
                                >
                                  ✓ {t.approve}
                                </button>
                                <button
                                  className={`reject ${item.status === "rejected" ? "active" : ""}`}
                                  onClick={() => rule(item, false)}
                                >
                                  ✗ {t.reject}
                                </button>
                              </div>
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
