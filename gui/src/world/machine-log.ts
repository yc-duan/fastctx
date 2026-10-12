// The hub's machine-layer history (`fastctx_types::machine_log`).
import type { AccountName, MemberKind, PlanLabel, Relay } from "./accounts";
import type { MachineName, Timestamp } from "./wire";

export interface MachineLogActor {
  account: AccountName | null;
  authenticated: boolean;
  member: MachineName | null;
  peer: string | null;
}

export type MachineLogAction =
  | { change: "collab"; version: number }
  | { change: "share"; machine: MachineName; to: AccountName; shared: boolean }
  | { change: "lend"; machine: MachineName; to: AccountName; agent: string; home: string | null; lent: boolean; until?: Timestamp }
  | { change: "lend_ended"; machine: MachineName; to: AccountName; agent: string; home: string | null }
  | { change: "plan"; machine: MachineName; agent: string; home: string | null; plan: PlanLabel | null }
  | { change: "stop_acting"; machine: MachineName }
  | { change: "relays"; machine: MachineName; relays: Relay[] }
  | { change: "relay_at"; machine: MachineName; addresses: string[] }
  | { change: "remove"; name: MachineName; reason: string }
  | { change: "logout" }
  | { change: "token"; register: boolean; name: MachineName | null; kind: MemberKind; expires: Timestamp }
  | { change: "password_start" | "password_finish"; target: AccountName }
  | { change: "open_account" | "delete_account"; name: AccountName }
  | { change: "set_account"; name: AccountName; locked: boolean }
  | { change: "transfer"; to: AccountName }
  | { change: "settings"; self_registration: boolean }
  | { change: "open_project"; town: string; name: string }
  | { change: "share_project"; town: string; name: string | null; with: AccountName; shared: boolean }
  | { change: "move_project"; town: string; name: string | null; to: AccountName }
  | { change: "joined"; name: MachineName; kind: MemberKind; registered: boolean; via: "password" | "token" }
  | { change: "allow_acting"; machine: MachineName; agent: string; home: string | null }
  | { change: "acting_stopped"; machine: MachineName; account: AccountName }
  | { change: "left"; name: MachineName; account: AccountName; kind: MemberKind; unregistered: boolean; reason: string };

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
