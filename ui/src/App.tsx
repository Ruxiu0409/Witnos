import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import * as api from "./api";
import TerminalPanel from "./TerminalPanel";
import ResizeHandle, { type WidthSpec } from "./ResizeHandle";
import LangPicker from "./LangPicker";
import { detectLang, messages, saveLang, type Lang } from "./i18n";
import "./App.css";

function short(id: string): string {
  return id.slice(0, 8);
}

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
  return dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || dir;
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
  const [sidebarW, setSidebarW] = usePanelWidth(
    "witnos.sidebar_width",
    SIDEBAR_W,
  );
  const [detailW, setDetailW] = usePanelWidth("witnos.detail_width", DETAIL_W);
  const [resizing, setResizing] = useState(false);
  // What the workspace (the middle pane) is showing: the terminal, or the
  // settings view. The terminal stays mounted underneath so its shell
  // session survives the switch.
  const [workspaceView, setWorkspaceView] = useState<"terminal" | "settings">(
    "terminal",
  );
  const [showArchive, setShowArchive] = useState(false);
  const [lang, setLang] = useState<Lang>(detectLang);
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

  const [newTitle, setNewTitle] = useState("");
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

  const changeLang = (l: Lang) => {
    saveLang(l);
    setLang(l);
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

  const createGoal = async () => {
    if (!newTitle.trim()) return;
    const g = await api.createGoal(newTitle.trim());
    setNewTitle("");
    selectGoal(g.id);
    refresh();
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
    await api.drillDown(goal.id, ev.id, ptr);
  };

  const toggleSidebar = useCallback(() => {
    setCollapsed((c) => {
      localStorage.setItem("witnos.sidebar_collapsed", c ? "0" : "1");
      return !c;
    });
  }, []);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (
        e.metaKey &&
        !e.ctrlKey &&
        !e.altKey &&
        !e.shiftKey &&
        e.key.toLowerCase() === "s"
      ) {
        e.preventDefault();
        toggleSidebar();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [toggleSidebar]);

  const watchingCount = goals.filter((g) => g.watching).length;
  const archivedGoals = goals.filter((g) => g.status === "closed");
  const listedGoals = showArchive
    ? archivedGoals
    : goals.filter((g) => g.status !== "closed");
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
      className={`app ${resizing ? "resizing" : ""}`}
      style={
        {
          "--sidebar-w": `${sidebarW}px`,
          "--detail-w": `${detailW}px`,
        } as CSSProperties
      }
    >
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
        </header>
        {collapsed && watchingCount > 0 && (
          <div className="rail-watching" title={t.watchingCount(watchingCount)}>
            👁 {watchingCount}
          </div>
        )}
        <div className="goal-list">
          {!showArchive && (
            <div className="proj-section">
              {projects.length > 0 && (
                <div className="archive-head">{t.projectsHeading}</div>
              )}
              {projects.map((p) => (
                <button
                  key={p.dir}
                  className={`goal-row ${selProject === p.dir ? "sel" : ""}`}
                  title={p.dir}
                  onClick={() => selectProject(p.dir)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    setMenu({ x: e.clientX, y: e.clientY, project: p });
                  }}
                >
                  <span className="goal-title">📁 {basename(p.dir)}</span>
                  <span className="goal-meta">
                    {t.projectGoals(p.goal_count)}
                    {p.watching_count > 0 ? " · 👁" : ""}
                  </span>
                </button>
              ))}
              <button className="ghost watch-project" onClick={watchProject}>
                {t.watchProjectAuto}
              </button>
            </div>
          )}
          {showArchive && (
            <div className="archive-head">
              {t.archivedHeading(archivedGoals.length)}
            </div>
          )}
          {showArchive && archivedGoals.length === 0 && (
            <div className="list-empty">{t.archivedNone}</div>
          )}
          {listedGoals.map((g) => (
            <button
              key={g.id}
              className={`goal-row ${sel === g.id ? "sel" : ""}`}
              onClick={() => selectGoal(g.id)}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setMenu({ x: e.clientX, y: e.clientY, goal: g });
              }}
            >
              <span className="goal-title">{g.title}</span>
              <span className="goal-meta">
                v{g.contract_version} · {t.goalStatus(g.status)}
                {g.watching ? " · 👁" : ""}
                {g.strong_bet_count > 0 ? ` · (b)×${g.strong_bet_count}` : ""}
              </span>
            </button>
          ))}
        </div>
        {!showArchive && (
          <div className="new-goal">
            <input
              placeholder={t.newGoalPlaceholder}
              value={newTitle}
              onChange={(e) => setNewTitle(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && createGoal()}
            />
            <button onClick={createGoal}>{t.create}</button>
          </div>
        )}
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
                <LangPicker lang={lang} onChange={changeLang} t={t} />
              </div>
            </div>
          </section>
        )}
      </main>

      {workspaceView !== "settings" && (
        <>
          <ResizeHandle
            className="for-detail"
            label={t.resizeDetail}
            width={detailW}
            spec={DETAIL_W}
            dir={-1}
            onWidth={setDetailW}
            onResizing={setResizing}
          />
          <aside className="detail">
            {!goal ? (
              <div className="empty">
                {selProject ? t.projectHint : t.selectAGoal}
              </div>
            ) : (
              <>
                <header className="goal-head">
                  <h2>{goal.title}</h2>
                  <div className="goal-sub">
                    {t.contractV(goal.contract_version)} ·{" "}
                    {t.agentSyncedV(goal.agent_synced_version)} ·{" "}
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
                            action: () => api.closeGoal(goal.id).then(refresh),
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
                          {goal.status !== "closed" && editing !== item.id && (
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
                                <span className="chip">
                                  {t.againstV(ev.against_version)}
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
          </aside>
        </>
      )}
    </div>
  );
}
