// Where the machines page draws each hub and machine (`world-gui.md`, 账号、
// 机器、共享与借出): computed, never moved by hand, and stable. Each machine
// holds a numbered place round the hub it hangs on, kept by its key, so a
// machine stays where it was while others come and go; one on several hubs
// stands between them. The places are remembered on this device, so the
// drawing looks the same the next time it opens.

import type { Bounds, Point } from "../map-canvas";
import type { MapHub, MapNode } from "./network-map";

/** A machine's ring and a hub's knot, in the drawing's units. */
export const RING = 26;
export const KNOT = 8;
/** How far the first orbit is from its hub, and how far apart orbits are. */
const FIRST = 150;
const PITCH = 110;
/** Room along an orbit for a ring and its name. */
const SPACING = 128;
/** Hubs stand this far apart: two full orbits each, with the space between
 *  for the machines on both. */
const HUB_GAP = 780;
/** The golden angle spreads the first few places evenly round a hub, and
 *  every later one between them, without moving any. */
const GOLDEN = Math.PI * (3 - Math.sqrt(5));
const START = Math.PI * 1.1;

export interface Placement {
  hubs: ReadonlyMap<string, Point>;
  machines: ReadonlyMap<string, Point>;
  bounds: Bounds;
}

interface Remembered {
  hubs: string[];
  places: Record<string, number>;
}

/** Where the `index`th place round a hub is, relative to the hub. */
function around(index: number): Point {
  let rest = index;
  for (let orbit = 0; ; orbit += 1) {
    const radius = FIRST + orbit * PITCH;
    const room = Math.max(5, Math.floor((2 * Math.PI * radius) / SPACING));
    if (rest < room) {
      const angle = START + rest * GOLDEN;
      return [Math.cos(angle) * radius, Math.sin(angle) * radius];
    }
    rest -= room;
  }
}

/** The `index`th place between two or more hubs, relative to their middle:
 *  in a column, alternately below and above. */
function between(index: number): Point {
  const step = Math.ceil(index / 2) * 120;
  return [0, index % 2 ? step : -step];
}

export class MapLayout {
  /** Hub order, first seen first; a hub keeps its place on the line. */
  private readonly hubOrder: string[] = [];
  /** A machine's place: its group (a hub, or several) and its number there. */
  private readonly places = new Map<string, { group: string; index: number }>();
  /** As of the last update: each group's numbers in use, and the hubs. */
  private taken = new Map<string, Set<number>>();
  private hubPoints = new Map<string, Point>();

  constructor(private readonly storageKey: string | null) {
    const remembered = this.read();
    if (remembered) {
      this.hubOrder.push(...remembered.hubs);
      for (const [key, index] of Object.entries(remembered.places)) this.places.set(key, { group: "", index });
    }
  }

  update(hubs: readonly MapHub[], nodes: readonly MapNode[]): Placement {
    for (const hub of [...hubs].sort((a, b) => a.id.localeCompare(b.id))) if (!this.hubOrder.includes(hub.id)) this.hubOrder.push(hub.id);
    const live = new Set(hubs.map((hub) => hub.id));
    const order = this.hubOrder.filter((id) => live.has(id));
    const hubPoints = new Map<string, Point>(order.map((id, index) => [id, [index * HUB_GAP, 0]]));
    const groupOf = (node: MapNode) => node.hubs.filter((id) => live.has(id)).sort().join("+");
    // A place remembered for a machine stays its own while its group is the
    // same; a machine that moved to another hub takes a new one there.
    const taken = new Map<string, Set<number>>();
    for (const node of nodes) {
      const place = this.places.get(node.key);
      if (!place) continue;
      const group = groupOf(node);
      if (place.group && place.group !== group) {
        this.places.delete(node.key);
        continue;
      }
      place.group = group;
      const used = taken.get(group) ?? new Set<number>();
      if (used.has(place.index)) this.places.delete(node.key);
      else used.add(place.index);
      taken.set(group, used);
    }
    // Places this page gave out keep their number while it is open, so one
    // that comes back finds its place free.
    for (const [key, place] of this.places) {
      if (!nodes.some((node) => node.key === key) && place.group) taken.set(place.group, (taken.get(place.group) ?? new Set()).add(place.index));
    }
    for (const node of [...nodes].sort((a, b) => a.key.localeCompare(b.key))) {
      if (this.places.has(node.key)) continue;
      const group = groupOf(node);
      const used = taken.get(group) ?? new Set<number>();
      let index = 0;
      while (used.has(index)) index += 1;
      used.add(index);
      taken.set(group, used);
      this.places.set(node.key, { group, index });
    }
    const machines = new Map<string, Point>();
    const loose = nodes.filter((node) => !groupOf(node));
    for (const node of nodes) {
      const { group, index } = this.places.get(node.key)!;
      const ids = group ? group.split("+") : [];
      if (!ids.length) {
        // No hub to hang on: a row, as before any World is joined.
        const at = loose.indexOf(node);
        machines.set(node.key, [at * (RING * 2 + SPACING), FIRST]);
        continue;
      }
      const knots = ids.map((id) => hubPoints.get(id)!);
      const middle: Point = [knots.reduce((sum, p) => sum + p[0], 0) / knots.length, knots.reduce((sum, p) => sum + p[1], 0) / knots.length];
      const offset = ids.length > 1 ? between(index) : around(index);
      machines.set(node.key, [middle[0] + offset[0], middle[1] + offset[1]]);
    }
    this.taken = taken;
    this.hubPoints = hubPoints;
    this.write(new Set(nodes.map((node) => node.key)));
    const points = [...hubPoints.values(), ...machines.values()];
    const pad = RING + 60;
    const xs = points.map((p) => p[0]);
    const ys = points.map((p) => p[1]);
    const bounds = points.length
      ? { x: Math.min(...xs) - pad, y: Math.min(...ys) - pad, width: Math.max(...xs) - Math.min(...xs) + pad * 2, height: Math.max(...ys) - Math.min(...ys) + pad * 2 }
      : { x: -pad, y: -pad, width: pad * 2, height: pad * 2 };
    return { hubs: hubPoints, machines, bounds };
  }

  /** Where the next machine to come to `hub` alone will stand, as of the
   *  last update: the place the dashed ring that adds one waits in. */
  vacant(hub: string): Point | null {
    const knot = this.hubPoints.get(hub);
    if (!knot) return null;
    const used = this.taken.get(hub) ?? new Set<number>();
    let index = 0;
    while (used.has(index)) index += 1;
    const offset = around(index);
    return [knot[0] + offset[0], knot[1] + offset[1]];
  }

  private read(): Remembered | null {
    if (!this.storageKey) return null;
    try {
      const value = JSON.parse(localStorage.getItem(this.storageKey) ?? "null") as Remembered | null;
      return value && Array.isArray(value.hubs) && value.places && typeof value.places === "object" ? value : null;
    } catch {
      return null;
    }
  }

  /** Keeps the places of the machines there now; one gone is laid out
   *  afresh if it ever comes back on another day. */
  private write(present: ReadonlySet<string>): void {
    if (!this.storageKey) return;
    const places: Record<string, number> = {};
    for (const [key, place] of this.places) if (present.has(key)) places[key] = place.index;
    try {
      localStorage.setItem(this.storageKey, JSON.stringify({ hubs: this.hubOrder, places } satisfies Remembered));
    } catch {
      // Not kept: the drawing is laid out afresh the next time.
    }
  }
}
