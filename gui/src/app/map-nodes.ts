// What the drawing of the machines shows (`map-of.ts` reads it from the
// World): each hub, each machine with what is said of it, and the direct
// links between machines.

export interface MapHub {
  id: string;
  name: string;
  online: boolean;
}

export interface MapNode {
  key: string;
  name: string;
  /** Under the name: its system, or whose it is. */
  sub: string;
  hubs: string[];
  here: boolean;
  online: boolean | null;
  hold: "mine" | "others" | "lent";
  /** Its agents may reach the account's machines. */
  reaching: boolean;
  /** The agents on it, by what they are doing. */
  marks: ("busy" | "waiting" | "idle")[];
  /** What it is, said whole, for a pointer resting on it. */
  title: string;
  /** Where other machines connect to it directly, when the network's
   *  administrator allowed that; others reach the hub through it when their
   *  own line is down. */
  directAt?: string[];
  /** The machine it reaches the hub through now, when its own line is
   *  down. */
  through?: string | null;
  /** The round trip of its line to the hub, in milliseconds, as last
   *  measured. */
  hubMs?: number | undefined;
  /** Whose it is, as the drawing groups machines round their hub: the
   *  account's own, one other's, or lent by one other. */
  group?: { key: string; label: string } | undefined;
}

/** A direct link between two machines on the drawing, by their keys. */
export interface MapLink {
  a: string;
  b: string;
  up: boolean;
  /** Its round trip in milliseconds while up. */
  ms?: number | undefined;
  /** Why it is down. */
  why?: string | undefined;
}
