// Whether this device's interface shows the workbench's places
// (`world-layers.md`, 界面上有没有工作台): a switch of its own in the
// settings, kept on this device like the theme, apart from which logins the
// workbench is open in. It turns itself on as the user comes by a login to
// use there that this device has not seen yet: one of their own computers
// tells it has a login with the workbench open, or a machine begins to
// assist them. It never turns itself off, so the workbench never vanishes
// from under the user.

import type { WorldNow } from "../world/bridge";
import type { AssistSeen } from "../world/accounts";
import { signal, type Signal } from "./signals";

/** Where this device keeps the switch and the sources it has seen. */
const KEPT = "workbench.shown";
/** Where it kept the account's layer before there was a switch: its last
 *  value says whether this device showed the workbench then. */
const BEFORE = "layer";
/** The most sources kept: only a guard against a record that grows
 *  without end. */
const SEEN_MOST = 4096;

/** The source the user's own computers are, while one of them tells it
 *  has a login with the workbench open. */
const OWN = "own";

export interface Kept {
  on: boolean;
  /** The sources this device has seen, for as long as they last. */
  seen: string[];
}

/** What gives the user logins to use in the workbench, as a world tells
 *  it: the sources it has, and whether one seen before is known gone. */
export interface Sources {
  present: string[];
  gone(source: string): boolean;
}

function assistSource(hub: string, assist: AssistSeen): string {
  return JSON.stringify(["assist", hub, assist.account, assist.machine, assist.agent, assist.home]);
}

function hubOf(source: string): string | null {
  try {
    const said = JSON.parse(source) as unknown;
    return Array.isArray(said) && said[0] === "assist" && typeof said[1] === "string" ? said[1] : null;
  } catch {
    return null;
  }
}

/** The sources `world` tells. Only what is known to be gone is gone: the
 *  own computers while one of them has not told, or a hub whose view has
 *  not come, say nothing either way. */
export function sourcesOf(world: WorldNow): Sources {
  const told = world.nodes.workbench;
  const present = told ? [OWN] : [];
  const viewed = new Set<string>();
  for (const hub of world.accounts.hubs) {
    if (!hub.logins.length) continue;
    viewed.add(hub.hub);
    for (const assist of hub.assists) {
      if (hub.logins.includes(assist.account)) present.push(assistSource(hub.hub, assist));
    }
  }
  return {
    present,
    gone: (source) => {
      if (source === OWN) return told === false;
      const hub = hubOf(source);
      return hub !== null && viewed.has(hub) && !present.includes(source);
    },
  };
}

/** What is kept once `sources` were seen: those gone are forgotten, so that
 *  one coming back is new again; a new one turns the switch on. */
export function seeing(kept: Kept, sources: Sources): Kept {
  const seen = kept.seen.filter((source) => !sources.gone(source));
  const fresh = sources.present.filter((source) => !seen.includes(source));
  return { on: kept.on || fresh.length > 0, seen: [...seen, ...fresh].slice(-SEEN_MOST) };
}

/** The state the address asks for, as a specimen is shown: none where it
 *  asks for nothing. */
function asked(search: string): boolean | null {
  const asks = new URLSearchParams(search);
  if (asks.has("conversations") || asks.get("layer") === "workbench") return true;
  if (asks.get("layer") === "connect" || asks.has("unchosen")) return false;
  return null;
}

function isKept(value: unknown): value is Kept {
  if (typeof value !== "object" || value === null) return false;
  const kept = value as Partial<Kept>;
  return typeof kept.on === "boolean" && Array.isArray(kept.seen) && kept.seen.every((source) => typeof source === "string");
}

function read(): Kept {
  try {
    const kept = JSON.parse(localStorage.getItem(KEPT) ?? "null") as unknown;
    if (isKept(kept)) return kept;
    // Before the switch, the interface followed the account's layer: a
    // device that showed the workbench by it keeps showing it.
    return { on: localStorage.getItem(BEFORE) === "workbench", seen: [] };
  } catch {
    return { on: false, seen: [] };
  }
}

function same(a: Kept, b: Kept): boolean {
  return a.on === b.on && a.seen.length === b.seen.length && a.seen.every((source, at) => source === b.seen[at]);
}

class WorkbenchShown {
  /** Whether the interface shows the workbench's places. */
  readonly on: Signal<boolean>;
  private kept: Kept;
  /** The address asked for a state, as a specimen is shown: it is held as
   *  asked, followed by nothing and kept nowhere. */
  private readonly fixed: boolean;

  constructor() {
    const state = asked(location.search);
    this.fixed = state !== null;
    this.kept = state === null ? read() : { on: state, seen: [] };
    this.on = signal(this.kept.on);
  }

  /** The user's own choice, in the settings: what `world` tells now is
   *  seen with it, so that only a source coming after turns it on again. */
  choose(on: boolean, world: WorldNow | null): void {
    const seen = world ? seeing(this.kept, sourcesOf(world)).seen : this.kept.seen;
    this.keep({ on, seen });
  }

  /** The user opened the workbench in a login, or began a conversation in
   *  one: the interface shows it. */
  open(): void {
    if (!this.kept.on) this.keep({ ...this.kept, on: true });
  }

  /** What the world tells now: a source this device has not seen turns the
   *  switch on. */
  follow(world: WorldNow): void {
    if (!this.fixed) this.keep(seeing(this.kept, sourcesOf(world)));
  }

  private keep(kept: Kept): void {
    if (same(kept, this.kept)) return;
    this.kept = kept;
    this.on.set(kept.on);
    if (this.fixed) return;
    try {
      localStorage.setItem(KEPT, JSON.stringify(kept));
    } catch {
      // Not kept on this device: it is worked out again next time.
    }
  }
}

export const workbenchShown = new WorkbenchShown();
