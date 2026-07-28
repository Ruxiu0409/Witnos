import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "./api";
import "./App.css";

const STATUS_LABEL: Record<string, string> = {
  open: "open",
  laid: "awaiting your ruling",
  passed: "passed (oracle)",
  approved: "approved by you",
  rejected: "rejected by you",
};

function short(id: string): string {
  return id.slice(0, 8);
}

export default function App() {
  const [goals, setGoals] = useState<api.GoalSummary[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const [goal, setGoal] = useState<api.Goal | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem("witnos.sidebar_collapsed") === "1",
  );

  // Which evidence the user is looking at right now — the honest source of
  // the origin instrumentation (the strong-bet (b) signal).
  const [viewing, setViewing] = useState<string | null>(null);
  // Items whose evidence originals were opened this session (per-ruling flag).
  const drilled = useRef<Set<string>>(new Set());

  const [newTitle, setNewTitle] = useState("");
  const [newClaim, setNewClaim] = useState("");
  const [newCheck, setNewCheck] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [editClaim, setEditClaim] = useState("");
  const [editCheck, setEditCheck] = useState("");

  const refresh = useCallback(async () => {
    try {
      setGoals(await api.listGoals());
      if (sel) setGoal(await api.getGoal(sel));
      setErr(null);
    } catch (e) {
      setErr(String(e));
    }
  }, [sel]);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 1500);
    return () => clearInterval(t);
  }, [refresh]);

  const selectGoal = (id: string) => {
    setSel(id);
    setGoal(null);
    setViewing(null);
    setEditing(null);
  };

  const createGoal = async () => {
    if (!newTitle.trim()) return;
    const g = await api.createGoal(newTitle.trim());
    setNewTitle("");
    selectGoal(g.id);
    refresh();
  };

  const addItem = async () => {
    if (!goal || !newClaim.trim() || !newCheck.trim()) return;
    await api.addItem(goal.id, newClaim.trim(), newCheck.trim(), viewing);
    setNewClaim("");
    setNewCheck("");
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

  const toggleSidebar = () =>
    setCollapsed((c) => {
      localStorage.setItem("witnos.sidebar_collapsed", c ? "0" : "1");
      return !c;
    });

  const watchingCount = goals.filter((g) => g.watching).length;
  const needsYou = goal
    ? goal.items.filter(
        (i) => i.class.kind === "subjective" && i.status === "laid",
      )
    : [];

  const originNote = viewing
    ? `while viewing evidence ${short(viewing)} — records the strong-bet (b) signal`
    : goal && goal.sessions.length > 0
      ? "mid-run, unprompted"
      : "pre-run";

  return (
    <div className="app">
      <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
        <header>
          <div className="sidebar-title">
            <h1>witnos</h1>
            <div className="watching">
              {watchingCount > 0
                ? `watching ${watchingCount} goal${watchingCount > 1 ? "s" : ""}`
                : "watching nothing"}
            </div>
          </div>
          <button
            className="sidebar-toggle"
            onClick={toggleSidebar}
            title={collapsed ? "expand sidebar" : "collapse sidebar"}
            aria-label={collapsed ? "expand sidebar" : "collapse sidebar"}
            aria-expanded={!collapsed}
          >
            {collapsed ? "»" : "«"}
          </button>
        </header>
        {collapsed && watchingCount > 0 && (
          <div
            className="rail-watching"
            title={`watching ${watchingCount} goal${watchingCount > 1 ? "s" : ""}`}
          >
            👁 {watchingCount}
          </div>
        )}
        <div className="goal-list">
          {goals.map((g) => (
            <button
              key={g.id}
              className={`goal-row ${sel === g.id ? "sel" : ""}`}
              onClick={() => selectGoal(g.id)}
            >
              <span className="goal-title">{g.title}</span>
              <span className="goal-meta">
                v{g.contract_version} · {g.status.replaceAll("_", " ")}
                {g.watching ? " · 👁" : ""}
                {g.strong_bet_count > 0 ? ` · (b)×${g.strong_bet_count}` : ""}
              </span>
            </button>
          ))}
        </div>
        <div className="new-goal">
          <input
            placeholder="new goal title…"
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && createGoal()}
          />
          <button onClick={createGoal}>create</button>
        </div>
        {err && <div className="err">{err}</div>}
      </aside>

      <main className="detail">
        {!goal ? (
          <div className="empty">select a goal</div>
        ) : (
          <>
            <header className="goal-head">
              <h2>{goal.title}</h2>
              <div className="goal-sub">
                contract v{goal.contract_version} · agent synced v
                {goal.agent_synced_version} · {goal.status.replaceAll("_", " ")}
                {goal.project_dir ? ` · ${goal.project_dir}` : ""}
              </div>
              <div className="goal-actions">
                {goal.watching && (
                  <button onClick={() => api.unwatchGoal(goal.id).then(refresh)}>
                    stop watching
                  </button>
                )}
                {goal.status !== "closed" && (
                  <button
                    className="danger"
                    onClick={() =>
                      confirm("Close this goal? No agent will read it anymore.") &&
                      api.closeGoal(goal.id).then(refresh)
                    }
                  >
                    close goal
                  </button>
                )}
              </div>
            </header>

            {goal.status === "closed" && (
              <div className="banner closed">
                This goal is closed — no agent reads this contract anymore. To
                change the outcome, re-issue the goal.
              </div>
            )}
            {needsYou.length > 0 && (
              <div className="banner needs">
                {needsYou.length} subjective item{needsYou.length > 1 ? "s" : ""}{" "}
                await your ruling — the agent lays evidence and moves on; it is not
                waiting for you.
              </div>
            )}

            <section className="items">
              {goal.items.map((item) => {
                const evs = goal.evidence.filter((e) => e.item_id === item.id);
                const reinterpreted = item.interpretation_history.length > 1;
                return (
                  <article
                    key={item.id}
                    className={`item ${
                      item.status === "laid" && item.class.kind === "subjective"
                        ? "attention"
                        : ""
                    }`}
                  >
                    <div className="item-head">
                      <span className={`chip ${item.class.kind}`}>
                        {item.class.kind}
                      </span>
                      <span className={`chip status-${item.status}`}>
                        {STATUS_LABEL[item.status] ?? item.status}
                      </span>
                      {reinterpreted && (
                        <span
                          className="chip reinterpreted"
                          title="The agent re-read this criterion — check its current interpretation."
                        >
                          reinterpreted ×{item.interpretation_history.length - 1}
                        </span>
                      )}
                      <span className="chip origin" title={`origin: ${item.origin.kind}`}>
                        {item.origin.kind.replaceAll("_", " ")}
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
                          edit
                        </button>
                      )}
                    </div>

                    {editing === item.id ? (
                      <div className="edit-form">
                        <input
                          value={editClaim}
                          onChange={(e) => setEditClaim(e.target.value)}
                        />
                        <input
                          value={editCheck}
                          onChange={(e) => setEditCheck(e.target.value)}
                        />
                        <div>
                          <button onClick={() => saveEdit(item.id)}>
                            save (reopens item)
                          </button>
                          <button className="ghost" onClick={() => setEditing(null)}>
                            cancel
                          </button>
                        </div>
                      </div>
                    ) : (
                      <>
                        <div className="claim">{item.claim}</div>
                        <div className="check">check: {item.check}</div>
                      </>
                    )}

                    {item.interpretation && (
                      <div className="interp">
                        <span className="interp-label">agent reads this as:</span>{" "}
                        {item.interpretation}
                      </div>
                    )}

                    {evs.map((ev) => {
                      const stale = ev.against_version < item.last_edited_version;
                      return (
                        <div
                          key={ev.id}
                          className={`evidence ${viewing === ev.id ? "viewing" : ""}`}
                          onClick={() => setViewing(ev.id)}
                        >
                          <div className="ev-head">
                            <span className="ev-conclusion">{ev.conclusion}</span>
                            <span className="chip">against v{ev.against_version}</span>
                            {stale && (
                              <span
                                className="chip stale"
                                title="The criterion was edited after this evidence was captured."
                              >
                                stale
                              </span>
                            )}
                          </div>
                          <div className="ev-basis">{ev.basis}</div>
                          <div className="ev-prov">
                            {ev.provenance.map((p, idx) => (
                              <button
                                key={idx}
                                className="prov"
                                title="open the original (recorded as a drill-down)"
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
                      ["laid", "approved", "rejected"].includes(item.status) &&
                      goal.status !== "closed" && (
                        <div className="ruling">
                          <button
                            className={`approve ${item.status === "approved" ? "active" : ""}`}
                            onClick={() => rule(item, true)}
                          >
                            ✓ approve
                          </button>
                          <button
                            className={`reject ${item.status === "rejected" ? "active" : ""}`}
                            onClick={() => rule(item, false)}
                          >
                            ✗ reject
                          </button>
                        </div>
                      )}
                  </article>
                );
              })}
            </section>

            {goal.status !== "closed" && (
              <section className="add-item">
                <h3>add a verification item</h3>
                <input
                  placeholder="claim — what must hold"
                  value={newClaim}
                  onChange={(e) => setNewClaim(e.target.value)}
                />
                <input
                  placeholder="check — how to verify it"
                  value={newCheck}
                  onChange={(e) => setNewCheck(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && addItem()}
                />
                <div className="add-row">
                  <button onClick={addItem}>add (subjective by default)</button>
                  <span className="origin-note">
                    will be recorded as: {originNote}
                    {viewing && (
                      <button className="ghost" onClick={() => setViewing(null)}>
                        clear
                      </button>
                    )}
                  </span>
                </div>
              </section>
            )}
          </>
        )}
      </main>
    </div>
  );
}
