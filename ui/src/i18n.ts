// Hand-rolled UI strings: two locales, one typed table, zero dependencies.
// `en` defines the key set; the type checker forces zh-Hant to cover it
// exactly. Enum values coming from the store (goal status, item status,
// origin kind) go through lookup functions that fall back to the raw value,
// so an unknown value from a newer core shows untranslated instead of empty.

export type Lang = "en" | "zh-Hant";

// Native name plus what each UI language calls it — the picker renders
// "native • name-in-current-UI-language" (skipping the second half when
// identical), like macOS language menus.
export const LANGS: {
  value: Lang;
  native: string;
  names: Record<Lang, string>;
}[] = [
  {
    value: "en",
    native: "English",
    names: { en: "English", "zh-Hant": "英文" },
  },
  {
    value: "zh-Hant",
    native: "繁體中文",
    names: { en: "Traditional Chinese", "zh-Hant": "繁體中文" },
  },
];

const LS_KEY = "witnos.lang";

export function detectLang(): Lang {
  const saved = localStorage.getItem(LS_KEY);
  if (saved === "en" || saved === "zh-Hant") return saved;
  return navigator.language.toLowerCase().startsWith("zh") ? "zh-Hant" : "en";
}

export function saveLang(lang: Lang) {
  localStorage.setItem(LS_KEY, lang);
}

// No `ruled`: it left the domain with approval (a delivered goal now simply
// stays `awaiting_rulings`), and the core aliases the stored value onto that as
// it reads an older goal, so it never reaches the UI. `awaiting_rulings` is
// labelled for what actually happened — the agent laid its evidence — because
// nothing is in fact waiting on a verdict.
const GOAL_STATUS_EN: Record<string, string> = {
  running: "running",
  awaiting_rulings: "evidence laid",
  turn_ended_unmet: "turn ended unmet",
  closed: "closed",
};

const GOAL_STATUS_ZH: Record<string, string> = {
  running: "執行中",
  awaiting_rulings: "已擺出證據",
  turn_ended_unmet: "回合結束（條件未達）",
  closed: "已關閉",
};

// No `approved` either, for the same reason; App.tsx folds it too, since the
// status string drives filters and CSS classes and not just this label.
const ITEM_STATUS_EN: Record<string, string> = {
  open: "open",
  laid: "evidence laid",
  passed: "passed (oracle)",
  rejected: "sent back",
  waived: "waived",
};

const ITEM_STATUS_ZH: Record<string, string> = {
  open: "待驗證",
  laid: "已擺出證據",
  passed: "已通過（oracle）",
  rejected: "已退回",
  waived: "已豁免",
};

const ITEM_CLASS_EN: Record<string, string> = {
  subjective: "subjective",
  objective: "objective",
};

const ITEM_CLASS_ZH: Record<string, string> = {
  subjective: "主觀",
  objective: "客觀",
};

const ORIGIN_ZH: Record<string, string> = {
  user_pre_run: "使用者・執行前",
  user_viewing_evidence: "使用者・查看證據時",
  user_mid_run: "使用者・執行中",
  agent_initial: "代理・初始契約",
  agent_blindspot: "代理・盲點補充",
};

const en = {
  // sidebar
  watchingNone: "watching nothing",
  watchingCount: (n: number) => `watching ${n} goal${n > 1 ? "s" : ""}`,
  // Read aloud for the eye on a goal row: the only icon in the UI whose
  // meaning no text beside it repeats.
  watchingMark: "watching",
  expandSidebar: "expand sidebar",
  collapseSidebar: "collapse sidebar",
  expandDetail: "expand detail pane",
  collapseDetail: "collapse detail pane",
  resizeSidebar: "resize sidebar (double-click to reset)",
  resizeDetail: "resize detail pane (double-click to reset)",
  terminal: "terminal",
  splitBelow: "open another terminal below (⌘D)",
  closePane: "close this terminal",
  resizePanes: "drag to resize (double-click to even out)",
  restartShell: "restart shell",
  shellExited: "[shell exited]",
  shellStartFailed: (e: string) => `failed to start shell: ${e}`,
  settings: "settings",
  closeSettings: "close settings",
  language: "Language",
  searchLanguage: "search languages…",
  noMatches: "no matches",
  appearance: "Appearance",
  appearanceSystem: "system default",
  appearanceLight: "light",
  appearanceDark: "dark",
  openFilesWith: "Open files with",
  openFilesWithHint:
    "Evidence file pointers open here (with the line when known); links still open in the browser.",
  editorSystem: "system default",
  archive: "archive",
  showArchive: "show archived goals",
  hideArchive: "back to active goals",
  archivedHeading: (n: number) => `archived goals (${n})`,
  deleteGoalMenu: "delete goal…",
  confirmDeleteGoal: (title: string) =>
    `Delete goal "${title}"? This permanently removes its contract, evidence, and events.`,
  confirm: "confirm",
  delete: "delete",

  // projects (sidebar grouping)
  projectsHeading: "projects",
  watchProjectAuto: "watch a project (auto)",
  removeProject: "stop watching",
  confirmRemoveProject: (dir: string) =>
    `Stop auto-watching "${dir}"? Its session goals stop gating; the goals and their evidence stay.`,
  projectAddedNotice:
    "Hooks installed. Make sure Claude Code trusts this folder (new hooks may need /hooks approval). Every new agent session here gets its own goal from its first prompt.",
  projectNotWatched: "not auto-watched",
  noProjectHeading: "no project",
  needsRulingDot: "new evidence for you to look at",
  restartHere: "restart shell here",

  // goal detail
  goalStatus: (s: string) => GOAL_STATUS_EN[s] ?? s.replaceAll("_", " "),
  closeGoal: "close goal",
  confirmCloseGoal: (title: string) =>
    `Close goal "${title}"? No agent will read it anymore.`,
  closedBanner:
    "This goal is closed — no agent reads this contract anymore. To change the outcome, re-issue the goal.",
  // Same boundary as a closed goal, reached without anyone choosing it: the
  // terminal this goal's agent ran in is gone, so nothing reads the contract
  // anymore. Say so where the human is looking at it, or every lever on this
  // pane silently aims at an agent that no longer exists.
  endedBanner:
    "This goal's agent session has ended — closing a terminal, /clear, or quitting Witnos all start a new session, and a session never comes back. No agent reads this contract anymore: you can still edit and waive items, but to change the outcome, re-issue the goal.",
  needsBanner: (n: number) =>
    `${n} subjective item${n > 1 ? "s" : ""} laid evidence for you to look at. The agent moved on and is not waiting — if you disagree, edit the item or send it back.`,
  needsBannerEnded: (n: number) =>
    `${n} subjective item${n > 1 ? "s" : ""} laid evidence for you to look at. Its session has ended, so nothing goes back to the agent — read it, and re-issue the goal if you disagree.`,

  // sending the change into the agent's own shell (Witnos owns that terminal,
  // so a correction can run now instead of waiting for the agent's next stop).
  // Only YOUR edits raise this — the agent bumps the version itself all run
  // long, and a banner that fired on its own bookkeeping would be pure noise.
  // No version numbers and no "reconcile": both are the agent's vocabulary, and
  // nothing else in this window ever shows a v-number to anchor them to, so they
  // read as a machine talking about itself. What the human can act on is that
  // the edit hasn't landed yet, and the button that lands it.
  unsyncedBanner:
    "Your change hasn't reached the agent yet. Its next stop is blocked until it reads the change and answers it — or reach it now:",
  sendToAgent: "send it to the agent now",
  sentToAgent: "Sent — typed into the agent's terminal, so it has your change.",
  agentWorking:
    "The agent is working, so it was left alone — and it needs no typing: your change is injected into its own conversation after its next tool call.",
  agentNotRunning:
    "That terminal is sitting at a shell prompt — no agent to type into, and prose typed at a shell would just run as a command. Nothing was sent; your change stays in this contract.",
  agentUnbound:
    "No live terminal for this goal's session — a /clear or a closed pane starts a new session, and a new session gets its own goal (that is the design). Your change stays in this contract.",

  // items
  itemClass: (k: string) => ITEM_CLASS_EN[k] ?? k,
  itemStatus: (s: string) => ITEM_STATUS_EN[s] ?? s,
  reinterpreted: (n: number) => `reinterpreted ×${n}`,
  reinterpretedTitle:
    "The agent re-read this criterion — check its current interpretation.",
  originKind: (k: string) => k.replaceAll("_", " "),
  originTitle: (kindLabel: string) => `origin: ${kindLabel}`,
  edit: "edit",
  saveReopens: "save (reopens item)",
  cancel: "cancel",
  checkLine: (c: string) => `check: ${c}`,
  agentReadsThisAs: "agent reads this as:",
  stale: "stale",
  staleTitle: "The criterion was edited after this evidence was captured.",
  provTitle: "open the original (recorded as a drill-down)",
  sendItBack: "send it back",
  sendItBackTitle:
    "Keeps the criterion, tells the agent its evidence doesn't pass. An agent still running hears it too, without being interrupted.",
  waive: "don't check this",
  waiveTitle:
    "Waive this item: nobody checks it. The agent is no longer asked for evidence, and the gate ignores it.",
  unwaive: "check this again",
  waivedNote: "waived — nobody checks this one.",

  // add item
  addItemHeading: "add a verification item",
  claimPlaceholder: "what must be true when this is done",
  checkPlaceholder: "how to verify (optional — the agent proposes one)",
  addSubjective: "add (subjective by default)",
  recordedAs: (note: string) => `will be recorded as: ${note}`,
  originViewing: (id: string) =>
    `while viewing evidence ${id} — records the strong-bet (b) signal`,
  originMidRun: "mid-run, unprompted",
  originPreRun: "pre-run",
  clear: "clear",
};

export type Messages = typeof en;

const zhHant: Messages = {
  // sidebar
  watchingNone: "未監看任何目標",
  watchingCount: (n) => `正在監看 ${n} 個目標`,
  watchingMark: "監看中",
  expandSidebar: "展開側欄",
  collapseSidebar: "收合側欄",
  expandDetail: "展開詳情欄",
  collapseDetail: "收合詳情欄",
  resizeSidebar: "調整側欄寬度（雙擊還原預設）",
  resizeDetail: "調整詳情欄寬度（雙擊還原預設）",
  terminal: "終端機",
  splitBelow: "在下方開另一個終端機（⌘D）",
  closePane: "關閉這個終端機",
  resizePanes: "拖曳調整高度（雙擊平均分配）",
  restartShell: "重啟 shell",
  shellExited: "[shell 已結束]",
  shellStartFailed: (e) => `shell 啟動失敗：${e}`,
  settings: "設定",
  closeSettings: "關閉設定",
  language: "語言",
  searchLanguage: "搜尋語言…",
  noMatches: "沒有符合的語言",
  appearance: "外觀",
  appearanceSystem: "系統預設",
  appearanceLight: "淺色",
  appearanceDark: "深色",
  openFilesWith: "檔案開啟方式",
  openFilesWithHint:
    "證據裡的檔案來源會用它開啟（有行號就跳到該行）；連結仍用瀏覽器開啟。",
  editorSystem: "系統預設",
  archive: "封存",
  showArchive: "顯示已封存的目標",
  hideArchive: "返回進行中的目標",
  archivedHeading: (n) => `已封存的目標（${n}）`,
  deleteGoalMenu: "刪除目標…",
  confirmDeleteGoal: (title) =>
    `確定刪除目標「${title}」？這會永久移除它的契約、證據與事件紀錄。`,

  // projects (sidebar grouping)
  projectsHeading: "專案",
  watchProjectAuto: "監看專案（自動）",
  removeProject: "停止監看",
  confirmRemoveProject: (dir) =>
    `停止自動監看「${dir}」？其 session 目標不再攔停；目標與證據會保留。`,
  projectAddedNotice:
    "Hooks 已裝好。請確認 Claude Code 信任這個資料夾（新 hooks 可能需要在 /hooks 核准）。之後在這裡的每個新 agent session，第一個 prompt 會自動建立各自的目標。",
  projectNotWatched: "未自動監看",
  noProjectHeading: "未綁定專案",
  needsRulingDot: "有新證據等你看",
  restartHere: "在此重啟 shell",
  confirm: "確認",
  delete: "刪除",

  // goal detail
  goalStatus: (s) => GOAL_STATUS_ZH[s] ?? s.replaceAll("_", " "),
  closeGoal: "關閉目標",
  confirmCloseGoal: (title) => `確定關閉目標「${title}」？之後不會再有代理讀取它。`,
  closedBanner:
    "此目標已關閉——不會再有代理讀取這份契約。要改變結果，請重新發布這個目標。",
  endedBanner:
    "這個目標的 agent session 已經結束——關掉終端機、/clear、或結束 Witnos，都會開始新的 session，而舊的 session 不會回來。不會再有代理讀取這份契約：你仍然可以編輯或豁免項目，但要改變結果，請重新發布這個目標。",
  needsBanner: (n) =>
    `${n} 個主觀項目擺好了證據等你看。代理擺完就繼續前進，不會等你——你不同意的話，就編輯這個項目或把它退回。`,
  needsBannerEnded: (n) =>
    `${n} 個主觀項目擺好了證據等你看。它的 session 已經結束，任何東西都送不回代理了——看完之後若你不同意，請重新發布這個目標。`,

  // sending the change into the agent's own shell (Witnos owns that terminal,
  // so a correction can run now instead of waiting for the agent's next stop).
  // Only YOUR edits raise this — the agent bumps the version itself all run
  // long, and a banner that fired on its own bookkeeping would be pure noise.
  unsyncedBanner:
    "你的修改還沒傳到代理手上。它下次想停下時會被攔住，直到它讀完你的修改並回應——或者現在就送過去：",
  sendToAgent: "立刻送到代理",
  sentToAgent: "已送出——已打進代理的終端機，它拿到你的修改了。",
  agentWorking:
    "代理正在工作，所以沒有打斷它——而且根本不用打字：你的修改會在它下一次呼叫工具之後，直接注入它的對話裡。",
  agentNotRunning:
    "那個終端機停在 shell 提示符——沒有代理可以打字進去，而在 shell 打一段話只會被當成指令執行。所以什麼都沒送出；你的修改留在這份契約裡。",
  agentUnbound:
    "這個目標的 session 沒有還活著的終端機——/clear 或關掉終端機都會開始新的 session，而新的 session 會有自己的目標（這是設計如此）。你的修改會留在這份契約裡。",

  // items
  itemClass: (k) => ITEM_CLASS_ZH[k] ?? k,
  itemStatus: (s) => ITEM_STATUS_ZH[s] ?? s,
  reinterpreted: (n) => `重新詮釋 ×${n}`,
  reinterpretedTitle: "代理重新解讀了這項驗收條件——檢查它目前的詮釋。",
  originKind: (k) => ORIGIN_ZH[k] ?? k.replaceAll("_", " "),
  originTitle: (kindLabel) => `來源：${kindLabel}`,
  edit: "編輯",
  saveReopens: "儲存（項目重新開啟）",
  cancel: "取消",
  checkLine: (c) => `檢核方式：${c}`,
  agentReadsThisAs: "代理對此的解讀：",
  stale: "過時",
  staleTitle: "這項驗收條件在證據擷取之後被修改過。",
  provTitle: "開啟原始出處（會記錄為一次下鑽）",
  sendItBack: "退回給代理",
  sendItBackTitle:
    "保留這項條件，但告訴代理它的證據不算通過。還在跑的代理也收得到，而且不會被打斷。",
  waive: "不檢查這項",
  waiveTitle:
    "豁免這一項：沒有人會檢查它。不再要求代理提供證據，gate 也會忽略它。",
  unwaive: "恢復檢查",
  waivedNote: "已豁免——沒有人會檢查這一項。",

  // add item
  addItemHeading: "新增驗證項目",
  claimPlaceholder: "完成時必須成立的事",
  checkPlaceholder: "如何驗證（選填——留空由代理提出）",
  addSubjective: "新增（預設為主觀）",
  recordedAs: (note) => `將記錄為：${note}`,
  originViewing: (id) => `查看證據 ${id} 時——記錄為 strong-bet (b) 訊號`,
  originMidRun: "執行中・自發",
  originPreRun: "執行前",
  clear: "清除",
};

export const messages: Record<Lang, Messages> = { en, "zh-Hant": zhHant };
