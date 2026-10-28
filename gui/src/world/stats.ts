// How a machine says it is doing, for its details page
// (`fastctx_types::stats`): read when asked, nothing sampled meanwhile.

import type { HubName } from "./accounts";
import type { MachineName, Timestamp } from "./wire";

export interface MachineStats {
  /** Which run of its node told it; an older node says nothing. */
  run?: string;
  machine: MachineName;
  /** When this was read. */
  at: Timestamp;
  /** When the machine started, as its clock has it. */
  boot?: Timestamp;
  /** The system as people call it, such as `Ubuntu 24.04.1 LTS`. */
  system: string;
  /** The kernel's own version, where it is not in `system`. */
  kernel?: string;
  /** The machine's own host name. */
  host: string;
  arch: string;
  cpu: Cpu;
  memory: Memory;
  disks: Disk[];
  cards: Card[];
  gpus?: Gpu[];
  /** Its lines to the hubs it is on. */
  lines: Line[];
  node: NodeSeen;
  /** The jobs running on it now, by the account each works for; its own
   *  under none. */
  jobs?: AccountJobs[];
}

/** How many jobs one account runs on a machine now, and the most its owner
 *  lets it run at once. */
export interface AccountJobs {
  account?: { hub: string; account: string };
  running: number;
  most?: number;
}

export interface Cpu {
  /** Empty when the system does not say. */
  model: string;
  /** Logical processors. */
  threads: number;
  /** How busy all of them were lately, 0 to 1. */
  busy?: number;
  /** Load averages over 1, 5 and 15 minutes, where the system keeps them. */
  load?: [number, number, number];
}

/** Bytes. */
export interface Memory {
  total: number;
  used: number;
  /** The page file or swap. */
  swap_total: number;
  swap_used: number;
}

export interface Disk {
  /** `C:/`, or a mount point such as `/`. */
  at: string;
  label?: string;
  fs?: string;
  total: number;
  free: number;
}

/** A network card. */
export interface Card {
  name: string;
  up: boolean;
  /** A card of the machine's own, not a virtual switch or a tunnel a
   *  program made: what crosses a tunnel crosses a card too. */
  hardware?: boolean;
  /** Link speed in bits a second, where the system knows it. */
  speed?: number;
  /** Bytes since it came up. */
  received: number;
  sent: number;
  /** Bytes a second, lately. */
  receiving?: number;
  sending?: number;
  addresses?: string[];
}

export interface Gpu {
  name: string;
  /** Bytes. */
  memory_total: number;
  memory_used: number;
  /** 0 to 1. */
  busy?: number;
}

/** The machine's line to one hub. */
export interface Line {
  hub: HubName;
  /** How many round trips and changes its node kept in this run. */
  kept?: LineKept;
  /** Since when it is up, or, while it is down, since when it tries. */
  since: Timestamp;
  up: boolean;
  /** The machine it goes through, when its own line is down. */
  through?: string;
  /** Round trips over the last hour, one each heartbeat; only those after
   *  the moment asked for, when one was. */
  trips: Trip[];
  /** When it came up and went down over the last 30 days, oldest first;
   *  only those after the moment asked for, when one was. */
  changes: LineChange[];
}

export interface Trip {
  at: Timestamp;
  ms: number;
}

export type LineChange =
  /** The node started: before it, the machine or the node was not running. */
  | { kind: "started"; at: Timestamp }
  | { kind: "up"; at: Timestamp }
  | { kind: "down"; at: Timestamp; why?: string };

export interface NodeSeen {
  version: string;
  started: Timestamp;
  /** Whether a system service keeps it running. */
  service: boolean;
  /** What of its work has gone on too long, as it last looked at itself. */
  stuck?: StuckWork[];
  /** What of its records needs attention, as its own words say it: a
   *  usage ledger with rows it set aside, or one it cannot read now. */
  health?: { kind: string; text: string }[];
  /** The last place macOS kept private from it while it runs, and when. */
  refused?: { path: string; at: Timestamp };
}

/** One piece of a node's work that has gone on too long. */
export interface StuckWork {
  doing: string;
  what: string;
  seconds: number;
}

export interface LineKept {
  trips: number;
  changes: number;
}

/** What the last reading told of a machine's lines, so the next brings only
 *  what came after it, whatever the machine's clock did meanwhile. */
export interface StatsSeen {
  run: string;
  lines: [HubName, LineKept][];
}
