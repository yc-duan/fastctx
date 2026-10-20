// What the user does to a conversation (`fastctx_types::management`):
// archive it or bring it back, let it sleep, wake it, move it to another
// project, point it at machines and folders, and delete it everywhere. The
// World answers with each stage as it went, so what was done and what was
// not stays in view when only part of it could be.

import { invoke } from "@tauri-apps/api/core";
import type { Landed } from "./bridge";
import type { Address, Timestamp, TownName } from "./wire";
import type { Activity } from "./world";
import type { Focus } from "./focus";

export type Management =
  /** Archives it: written as removing was, which nodes of every version read. */
  | { do: "remove" }
  | { do: "restore" }
  | { do: "sleep" }
  | { do: "wake" }
  | { do: "move"; town: TownName | null }
  | { do: "focus"; focus: Focus }
  /** The account's MCP servers it goes without (`world-user-config.md`). */
  | { do: "mcp"; off: string[] };

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

/** How deleting a conversation everywhere went, as far as this device can
 *  say: deleted here, in no network; taken by its hub, which has each
 *  machine and device delete what it keeps; or kept to ask the hub once
 *  this computer reaches it. */
export type Forgetting = "deleted" | "asked" | "queued";

export function forgetConversation(who: Address): Promise<Forgetting> {
  return invoke<Forgetting>("forget_conversation", { residentId: who.resident, machineName: who.machine });
}
