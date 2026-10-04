import type { Address, MachineName, Timestamp, TownName } from "../world/wire";
import type { MachineWho, Node, TownSummary } from "../world/world";

/** Local presentation data. No Rust reply currently supplies this chronicle. */
export type Happening =
  | { kind: "turn_ended"; at: Timestamp; who: Address; town: TownName | null; how: string; reason: string | null; said: string | null }
  | { kind: "job_ended"; at: Timestamp; job: string; machine: MachineName; owner: Address; town: TownName | null; how: string }
  | { kind: "journal"; at: Timestamp; town: TownName; journal: string; by: Address | null; first: string }
  | { kind: "asked"; at: Timestamp; who: Address; colleague: Address; town: TownName | null; label: string }
  | { kind: "machine"; at: Timestamp; machine: MachineName; online: boolean };

/** The homepage's assembled view, not a serialized WorldWho or NodesReply. */
export interface WorldState {
  here: MachineName;
  machines: MachineWho[];
  nodes: Node[];
  towns: TownSummary[];
  /** What happened lately, where the World can say it; `null` where it
   *  cannot yet. */
  happenings: Happening[] | null;
}
