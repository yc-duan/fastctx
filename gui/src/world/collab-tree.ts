// A conversation's collaboration as a tree (`world-gui.md`): the colleagues
// it asked, each with its goal and the host's agents it sent, and the
// conversation's own agents. Reached through the host, which gathers what the
// colleagues' machines report; parts it could not reach are named, not faked.

import { invoke } from "@tauri-apps/api/core";
import type { Address, ResidentId, Timestamp } from "./wire";

export type GoalStateWire = "active" | "complete" | "blocked" | "paused";

export interface TreeGoal {
  state: GoalStateWire;
  started_at: Timestamp;
  elapsed_ms: number;
  id?: string | null;
  /** The asker's goal it branched from. */
  parent?: string | null;
  objective?: string | null;
}

/** The host's agent, and the agents it sent under it, as far as they are known. */
export interface NativeAgent {
  id: string;
  name?: string | null;
  task?: string | null;
  model?: string | null;
  state?: string | null;
  started_at?: Timestamp | null;
  ended_at?: Timestamp | null;
  duration_ms?: number | null;
  children: NativeAgent[];
  unavailable?: string | null;
}

export interface Branch {
  address: Address;
  label?: string | null;
  born_at?: Timestamp | null;
  now?: { activity?: string; trouble?: unknown; title?: string | null; since?: Timestamp } | null;
  /** The colleague's goal, or the reason it could not be read (Rust Result). */
  goal: { Ok: TreeGoal | null } | { Err: string };
  agents: NativeAgent[];
  unavailable: string[];
}

export interface Tree {
  conversation: Branch;
  colleagues: Branch[];
}

export function collaborationTree(resident: ResidentId, machine?: string): Promise<Tree> {
  return invoke<Tree>("collaboration_tree", { resident, machine });
}
