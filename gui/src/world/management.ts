// What the user does to an agent (`fastctx_types::management`): remove it
// from the World, let it sleep, wake it, move it to another project, point
// it at machines and folders. The
// World answers with each stage as it went, so what was done and what was
// not stays in view when only part of it could be.

import { invoke } from "@tauri-apps/api/core";
import type { Landed } from "./bridge";
import type { Address, Timestamp, TownName } from "./wire";
import type { Activity } from "./world";
import type { Focus } from "./focus";

export type Management =
  | { do: "remove" }
  | { do: "sleep" }
  | { do: "wake" }
  | { do: "move"; town: TownName | null }
  | { do: "focus"; focus: Focus };

export type ManagementOutcome = "done" | "pending" | "unsupported" | "failed";

export interface ManagementStep {
  /** `host`, `records`, `tools` or `notification`. */
  stage: string;
  outcome: ManagementOutcome;
  /** The World's own account of it. */
  detail: string;
}

export interface Managed {
  /** When it left the World, once it has. */
  removed: Timestamp | null;
  town: TownName | null;
  activity: Activity | null;
  steps: ManagementStep[];
  /** How the agent was told, when it was moved. */
  notification: Landed | null;
}

export function manage(who: Address, action: Management): Promise<Managed> {
  return invoke<Managed>("manage", { residentId: who.resident, machineName: who.machine, action });
}
