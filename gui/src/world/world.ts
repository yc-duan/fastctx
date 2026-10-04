// The World as the front page reads it: the JSON forms of what `fastctx w`
// and `nodes` answer (`fastctx_types::town`, `::residents`, `::project`,
// `::world`), and of what happened while the reader was away.

import type { Address, MachineName, ResidentId } from "./events";

export interface Host {
  agent: string;
  client: string;
  version: string;
}

export type Origin = { kind: "user" } | { kind: "colleague"; asked_by: Address; label: string };

export interface About {
  id: ResidentId;
  host: Host;
  machine: MachineName;
  born_at: string;
  origin: Origin;
  session: string | null;
  world_started: boolean;
  model: string | null;
}

export interface Now {
  activity: "busy" | "idle" | "asleep";
  since: string;
  turn: string | null;
  trouble: { at: string; text: string } | null;
  desk: boolean;
  waiting_for_user: boolean;
}

export interface Action {
  at: string;
  machine: MachineName;
  hand: string;
  what: string;
  result: string;
}

export interface ResidentNow {
  about: About;
  town: string | null;
  here: boolean;
  now: Now | null;
  last_action: Action | null;
}

export interface RunningJob {
  job: string;
  machine: MachineName;
  owner: Address;
  started_at: string;
  pid: number | null;
  command: string;
  output: string;
}

export type MachineWho =
  | { kind: "answered"; reply: { machine: MachineName; residents: ResidentNow[]; jobs: RunningJob[] } }
  | { kind: "offline"; machine: MachineName; since: string | null }
  | { kind: "no_answer"; machine: MachineName; reason: string };

export interface MachineFacts {
  os: string;
  arch: string;
  home: string;
}

export interface Node {
  name: MachineName;
  online: boolean | null;
  since: string | null;
  facts: { Ok: MachineFacts } | { Err: string };
}

export interface TownSummary {
  town: string;
  record: { name: string; what: string; machines: MachineName[] };
  journals: number;
}

/** Something that happened, as the World can say it from its records. */
export type Happening =
  | { kind: "turn_ended"; at: string; who: Address; town: string | null; how: string; reason: string | null; said: string | null }
  | { kind: "job_ended"; at: string; job: string; machine: MachineName; owner: Address; town: string | null; how: string }
  | { kind: "journal"; at: string; town: string; journal: string; by: Address | null; first: string }
  | { kind: "asked"; at: string; who: Address; colleague: Address; town: string | null; label: string }
  | { kind: "machine"; at: string; machine: MachineName; online: boolean };

export interface WorldState {
  /** The machine this interface runs on. */
  here: MachineName;
  machines: MachineWho[];
  nodes: Node[];
  towns: TownSummary[];
  /** What happened lately, where the World can say it; `null` where it
   *  cannot yet. */
  happenings: Happening[] | null;
}
