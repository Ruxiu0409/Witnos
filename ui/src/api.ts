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
  sessions: { session_id: string; agent: string; bound_at: number }[];
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

export const ruleItem = (
  goalId: string,
  itemId: string,
  approve: boolean,
  afterDrillDown: boolean,
) => invoke("rule_item", { goalId, itemId, approve, afterDrillDown });

export const drillDown = (goalId: string, evidenceId: string, pointer: Pointer) =>
  invoke("drill_down", { goalId, evidenceId, pointer });

export const closeGoal = (goalId: string) => invoke("close_goal", { goalId });
export const deleteGoal = (goalId: string) => invoke("delete_goal", { goalId });
export const unwatchGoal = (goalId: string) => invoke("unwatch_goal", { goalId });

// Auto-watched projects (human-only surface — IPC, never HTTP).
export const pickProjectDir = () => invoke<string | null>("pick_project_dir");
export const addAutoProject = (dir: string) => invoke("add_auto_project", { dir });
export const removeAutoProject = (dir: string) =>
  invoke("remove_auto_project", { dir });
export const listAutoProjects = () =>
  invoke<ProjectSummary[]>("list_auto_projects");
