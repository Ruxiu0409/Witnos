// Typed wrappers over Tauri IPC. The UI talks to the in-process store
// through these commands (human side); the agent talks HTTP through the
// witnos CLI — that split is the human/agent trust boundary.

import { invoke } from "@tauri-apps/api/core";

export interface GoalSummary {
  id: string;
  title: string;
  status: string;
  contract_version: number;
  watching: boolean;
  strong_bet_count: number;
  project_dir?: string | null;
}

export interface ProjectSummary {
  dir: string;
  goal_count: number;
  watching_count: number;
}

export interface Pointer {
  kind: "file" | "command" | "url";
  path?: string;
  lines?: string;
  cmd?: string;
  url?: string;
}

export interface Evidence {
  id: string;
  item_id: string;
  conclusion: string;
  basis: string;
  provenance: Pointer[];
  against_version: number;
  captured_at: number;
  workspace: { commit?: string | null; dirty_hash?: string | null };
}

export interface Interpretation {
  text: string;
  against_version: number;
  at: number;
}

export interface Item {
  id: string;
  claim: string;
  check: string;
  class: { kind: string };
  interpretation: string | null;
  interpretation_history: Interpretation[];
  status: string;
  evidence_ids: string[];
  origin: { kind: string; evidence_id?: string };
  added_in_version: number;
  last_edited_version: number;
}

export interface Goal {
  id: string;
  title: string;
  status: string;
  contract_version: number;
  agent_synced_version: number;
  // The last version a HUMAN moved (add/edit/send-back/waive). The agent bumps
  // `contract_version` itself as it lays items, so that one says nothing about
  // whether there is something of yours the agent hasn't seen — this one does,
  // and it is what the "reach it now" offer is allowed to key on.
  last_human_edit_version: number;
  // `pane` is the terminal the session was started in, when Witnos spawned it —
  // what send_to_agent types into, and absent for a session it didn't spawn.
  sessions: {
    session_id: string;
    agent: string;
    bound_at: number;
    pane?: number | null;
  }[];
  items: Item[];
  evidence: Evidence[];
  events: unknown[];
  project_dir?: string | null;
  watching: boolean;
}

export const listGoals = () => invoke<GoalSummary[]>("list_goals");
export const getGoal = (id: string) => invoke<Goal>("get_goal", { id });

export const addItem = (
  goalId: string,
  claim: string,
  check: string,
  viewingEvidence: string | null,
) => invoke("add_item", { goalId, claim, check, viewingEvidence });

export const editItem = (
  goalId: string,
  itemId: string,
  claim: string,
  check: string,
) => invoke("edit_item", { goalId, itemId, claim, check });

// "Your evidence doesn't pass" — the criterion stays. There is no approve
// counterpart: the agent's work is presumed correct, so the human only ever
// has to act when they disagree. This bumps the contract version, so it
// reaches a running agent through the delivery channel too.
export const rejectItem = (
  goalId: string,
  itemId: string,
  afterDrillDown: boolean,
) => invoke("reject_item", { goalId, itemId, afterDrillDown });

// "Don't check this at all" — the gate ignores a waived item. Toggling it
// back returns the item to `open`.
export const waiveItem = (goalId: string, itemId: string, waived: boolean) =>
  invoke("waive_item", { goalId, itemId, waived });

// Type the change into the pane the agent runs in, so it lands now instead of
// at the agent's next stop. "sent" = a program owns that pane and the line went
// into it; "no_agent" = the pane is sitting at a bare shell prompt, so nothing
// was typed (prose at a shell would run as a command); "unbound" = no session,
// no pane recorded, or that pane is gone. `note` is the human's own words only
// — the core composes the version line and the commands — so it is optional.
export type SendOutcome = "sent" | "no_agent" | "unbound";

export const sendToAgent = (goalId: string, note = "") =>
  invoke<SendOutcome>("send_to_agent", { goalId, note });

export const drillDown = (
  goalId: string,
  evidenceId: string,
  pointer: Pointer,
  editor: string,
) => invoke("drill_down", { goalId, evidenceId, pointer, editor });

export const closeGoal = (goalId: string) => invoke("close_goal", { goalId });
export const deleteGoal = (goalId: string) => invoke("delete_goal", { goalId });

// Auto-watched projects (human-only surface — IPC, never HTTP).
export const pickProjectDir = () => invoke<string | null>("pick_project_dir");
export const addAutoProject = (dir: string) => invoke("add_auto_project", { dir });
export const removeAutoProject = (dir: string) =>
  invoke("remove_auto_project", { dir });
export const listAutoProjects = () =>
  invoke<ProjectSummary[]>("list_auto_projects");
