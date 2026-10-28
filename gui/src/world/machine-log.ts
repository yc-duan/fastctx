// The hub's machine-layer history (`fastctx_types::machine_log`).
import type { AccountName, MemberKind, PlanLabel, Relay, TokenWay } from "./accounts";
import type { Address, MachineName, Timestamp } from "./wire";

export interface MachineLogActor {
  account: AccountName | null;
  authenticated: boolean;
  member: MachineName | null;
  peer: string | null;
}

export type MachineLogAction =
  | { change: "collab"; version: number }
  | { change: "share"; machine: MachineName; to: AccountName; shared: boolean; jobs?: number }
  // Assistance, under the names the hub first wrote it under, which hubs
  // and nodes of every version read.
  | { change: "lend"; machine: MachineName; to: AccountName; agent: string; home: string | null; lent: boolean; until?: Timestamp; weekly?: number; daily?: number; five_hour?: number; turns?: number }
  | { change: "lend_ended"; machine: MachineName; to: AccountName; agent: string; home: string | null }
  | { change: "plan"; machine: MachineName; agent: string; home: string | null; plan: PlanLabel | null }
  | { change: "stop_acting"; machine: MachineName }
  | { change: "relays"; machine: MachineName; relays: Relay[] }
  | { change: "relay_at"; machine: MachineName; addresses: string[] }
  | { change: "direct"; machine: MachineName; at: string[] }
  /** Taken off; `then`, told to uninstall Fastctx as it heard so. */
  | { change: "remove"; name: MachineName; reason: string; then?: "uninstall" | "everything" }
  /** Taken off to uninstall Fastctx, it could not do all of it. */
  | { change: "uninstall_left"; name: MachineName; left: string; by?: AccountName; home?: AccountName }
  /** An update kept for a machine away, or, without a version, taken back. */
  | { change: "update_due"; machine: MachineName; version?: string }
  /** A machine updated as someone asked from elsewhere. */
  | { change: "updated"; machine: MachineName; from: string; to: string; by: AccountName }
  | { change: "rename"; machine: MachineName; to: MachineName }
  | { change: "group"; machine: MachineName; group?: string }
  | { change: "rename_group"; owner: AccountName; from: string; to?: string }
  | { change: "logout" }
  | { change: "token"; register: boolean; name: MachineName | null; kind: MemberKind; expires: Timestamp; managed?: boolean; label?: string; uses?: number; way?: TokenWay }
  | { change: "revoke_token"; label?: string; way?: TokenWay; managed?: boolean }
  | { change: "password_start" | "password_finish"; target: AccountName }
  | { change: "open_account"; name: AccountName; visitor?: boolean }
  | { change: "delete_account"; name: AccountName }
  | { change: "set_role"; name: AccountName; visitor: boolean }
  | { change: "accept_role"; accept: boolean }
  | { change: "set_account"; name: AccountName; locked: boolean }
  | { change: "transfer"; to: AccountName }
  | { change: "settings"; self_registration: boolean }
  | { change: "updates"; channel: "stable" | "beta"; minimum?: string; managed_auto?: boolean; intake?: number }
  | { change: "dissolve"; name: string }
  | { change: "open_project"; town: string; name: string }
  | { change: "share_project"; town: string; name: string | null; with: AccountName; shared: boolean }
  | { change: "move_project"; town: string; name: string | null; to: AccountName }
  | { change: "delete_project"; town: string; name: string | null }
  | { change: "joined"; name: MachineName; kind: MemberKind; registered: boolean; via: "password" | "token"; managed?: boolean }
  | { change: "allow_acting"; machine: MachineName; agent: string; home: string | null }
  | { change: "allow_sentinel_home"; machine: MachineName }
  | { change: "acting_stopped"; machine: MachineName; account: AccountName }
  | { change: "left"; name: MachineName; account: AccountName; kind: MemberKind; unregistered: boolean; reason: string }
  /** A conversation the user deleted everywhere; then each member's part
   *  done, or refused for now with why. Read by its account alone. */
  | { change: "forget"; conversation: Address }
  | { change: "forgotten"; conversation: Address; on: MachineName };

export interface MachineLogRecord {
  id: number;
  at: Timestamp;
  actor: MachineLogActor;
  action: MachineLogAction;
  outcome: { kind: "done" } | { kind: "refused"; reason: string };
}

export interface MachineLogPage {
  records: MachineLogRecord[];
  next_before: number | null;
}
