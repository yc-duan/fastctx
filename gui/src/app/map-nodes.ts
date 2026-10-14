// What the drawing of the machines shows (`map-of.ts` reads it from the
// World): each hub, each machine with what is said of it, the phones signed
// in to the account, the direct links between machines, and for the
// network's administrator every person.

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
  /** Whose it is to the account: its own, another's it works on, one
   *  whose agent assists it, or a visitor's it only sees. */
  hold: "mine" | "others" | "assist" | "visitor";
  /** Its agents may reach the account's machines. */
  reaching: boolean;
  /** How many agents are at work or waiting on it now, whosever they
   *  are: given to its owner alone, none for anyone else (`world-gui.md`,
   *  机器页). */
  busy: number;
  /** What it is, said whole, for a pointer resting on it. */
  title: string;
  /** The keys it had under the names it had: renamed, it keeps the place
   *  it stood in. */
  was?: string[];
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
   *  account's own, one other's, or one other's whose agent assists it; or,
   *  put in one of its owner's groups, that group, `within` the owner's box
   *  (`world-network.md`, 机器的分组). */
  group?: { key: string; label: string; within?: { key: string; label: string } | undefined } | undefined;
  /** A phone signed in to the account: a device that only logs in, where
   *  no agent runs; drawn with a phone in its ring, nothing done to it but
   *  taking it out of the account. */
  device?: boolean;
  /** For a device away, since when. */
  since?: string | null;
}

/** The key of a phone signed in to the account, on `hub`. */
export const deviceKey = (hub: string, name: string) => `device:${hub}/${name}`;

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

/** A person of a network, as its administrator sees them on the drawing:
 *  a box of their own, with or without machines, and their "⋯". */
export interface MapPerson {
  hub: string;
  account: string;
  /** Their box's name, as a machine of theirs is grouped. */
  label: string;
  locked: boolean;
  /** A visitor, whose machines are their own alone; one asked to become a
   *  member who has not agreed yet. */
  visitor?: boolean;
  promotion?: boolean;
}
