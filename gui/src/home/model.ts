import type { Address, MachineName, Timestamp, TownName } from "../world/wire";
import type { Machine } from "../app/reach";
import type { JobOwner, MachineWho, ResidentNow, TownSummary, WhoFor, Writer } from "../world/world";

/** One line of what happened while the reader was away, read from what
 *  the World answered (`home/since.ts`). */
export type Happening =
  | { kind: "file_action"; at: Timestamp; who: Address; town: TownName | null; hand: string; paths: string; result: string }
  | { kind: "turn_ended"; at: Timestamp; who: Address; town: TownName | null; how: string; reason: string | null; said: string | null }
  | { kind: "job_ended"; at: Timestamp; job: string; machine: MachineName; owner: JobOwner; town: TownName | null; how: string }
  | { kind: "asked"; at: Timestamp; who: Address; colleague: Address; town: TownName | null; label: string }
  | { kind: "arrived"; at: Timestamp; who: Address; town: TownName | null; agent: string }
  | { kind: "moved"; at: Timestamp; who: Address; town: TownName | null; from: TownName | null; to: TownName | null }
  | { kind: "project"; at: Timestamp; town: TownName; version: number; name: string; by: Writer }
  | { kind: "uncertain"; at: Timestamp; who: Address; town: TownName | null; why: string }
  | { kind: "undelivered"; at: Timestamp; who: Address; to: Address; town: TownName | null; why: string }
  | { kind: "machine"; at: Timestamp; machine: MachineName; online: boolean };

/** A machine whose part of the answer is missing or partial, and why. */
export interface Unasked {
  machine: MachineName;
  offline: boolean;
  why: string | null;
}

/** What happened while the reader was away, as far as the World said. */
export interface Away {
  happenings: Happening[];
  unasked: Unasked[];
  /** The World could not be asked at all. */
  trouble: string | null;
}

/** The homepage's assembled view, not a serialized WorldWho or NodesReply. */
export interface WorldState {
  here: MachineName;
  machines: MachineWho[];
  /** Every conversation known, archived ones too, and on a machine away as
   *  it last answered: one told of is named by what it is called. */
  known: ResidentNow[];
  /** What runs there for other accounts than the reader's, and for whom. */
  others?: WhoFor[];
  /** The machines this computer or device reaches, itself first. */
  nodes: Machine[];
  /** Machines of others whose agents assist the user, by whose they are:
   *  never shown as machines (`world-network.md`, 共享机器与智能体协助). */
  assisting: Map<string, string>;
  towns: TownSummary[];
  /** What happened while the reader was away; `null` before the World
   *  has been asked, or where there is no earlier visit to ask from. */
  away: Away | null;
}
