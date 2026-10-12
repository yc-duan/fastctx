// Where the machines page draws each hub and machine (`world-gui.md`, 账号、
// 机器、共享与借出): computed, never moved by hand, and calm. Round each hub
// its machines stand evenly spaced on one orbit, grouped by whose they are:
// the account's own first, this computer at their head at the top, then
// each other account's shared with it, then those an agent is lent on from
// another, clockwise. Groups are parted by an empty place, and the drawing
// writes each one's name along an arc outside it when there is more than
// one. Within a group, machines linked directly stand side by side, so the
// straight line between them is short. The orbit widens with the number of
// places, so every ring keeps room for its name. The dashed ring that adds
// a machine holds the place after the account's own, so a machine that
// comes stands where it waited. The order within each group is remembered
// on this device: machines keep their order while others come and go, only
// the spacing evens out. With several hubs side by side, each one's
// machines keep clear of the side toward its neighbours, and a machine on
// more than one stands between them.

import type { Bounds, Point } from "../map-canvas";
import type { MapHub, MapLink, MapNode } from "./network-map";

/** A machine's ring and a hub's knot, in the drawing's units. */
export const RING = 26;
export const KNOT = 8;
/** The smallest orbit, and the room along an orbit for a ring and its
 *  name, two lines under it. */
const FIRST = 230;
const SPACING = 200;
/** Hubs stand at least this far apart, more when their orbits are wide. */
const HUB_GAP = 860;
/** Each hub's machines leave this much of the circle free toward each
 *  neighbouring hub, for the machines on both. */
const TOWARD = (Math.PI * 2) / 3;
/** Machines on several hubs stand in a column between them, this far
 *  apart. */
const COLUMN = 160;

/** A group of machines round a hub, for its arc and name: the stretch of
 *  the orbit its places take, as angles clockwise from the right. */
export interface GroupArc {
  key: string;
  label: string;
  hub: string;
  centre: Point;
  radius: number;
  from: number;
  to: number;
}

export interface Placement {
  hubs: ReadonlyMap<string, Point>;
  machines: ReadonlyMap<string, Point>;
  /** The groups round each hub with more than one. */
  groups: GroupArc[];
  bounds: Bounds;
}

interface Remembered {
  hubs: string[];
  /** Each machine's place in its group's order. */
  places: Record<string, number>;
}

const HOLD_RANK: Record<MapNode["hold"], number> = { mine: 0, others: 1, lent: 2 };

/** The order a machine first takes in its group: this computer, then by
 *  name. */
function firstOrder(a: MapNode, b: MapNode): number {
  return Number(b.here) - Number(a.here) || HOLD_RANK[a.hold] - HOLD_RANK[b.hold] || a.name.localeCompare(b.name);
}

/** The arcs of a hub's circle its machines may stand on, as [start,
 *  length] in radians clockwise from the right; free toward each
 *  neighbour. */
function arcsOf(left: boolean, right: boolean): [number, number][] {
  if (left && right) {
    // Above and below, each a third of the circle.
    const length = Math.PI - TOWARD;
    return [
      [-Math.PI / 2 - length / 2, length],
      [Math.PI / 2 - length / 2, length],
    ];
  }
  if (right) return [[TOWARD / 2, Math.PI * 2 - TOWARD]];
  if (left) return [[Math.PI + TOWARD / 2, Math.PI * 2 - TOWARD]];
  return [[-Math.PI / 2, Math.PI * 2]];
}

/** The angle of the `index`th of `count` places spread evenly along
 *  `arcs`. A whole circle starts at its top; an arc keeps half a place
 *  free at each end. */
function angleOn(arcs: [number, number][], index: number, count: number): number {
  const total = arcs.reduce((sum, [, length]) => sum + length, 0);
  const whole = arcs.length === 1 && Math.abs(arcs[0]![1] - Math.PI * 2) < 1e-9;
  let along = whole ? (index / count) * total : ((index + 0.5) / count) * total;
  for (const [start, length] of arcs) {
    if (along <= length + 1e-9) return start + along;
    along -= length;
  }
  const [start, length] = arcs[arcs.length - 1]!;
  return start + length;
}

/** Puts machines linked directly side by side, keeping the order they
 *  came in otherwise: each next is one linked with the last if there is
 *  one, else the earliest left. */
function chained(ordered: MapNode[], links: readonly MapLink[]): MapNode[] {
  const linked = (a: string, b: string) => links.some((link) => (link.a === a && link.b === b) || (link.a === b && link.b === a));
  const left = [...ordered];
  const chain: MapNode[] = [];
  while (left.length) {
    const last = chain[chain.length - 1];
    const at = last ? left.findIndex((node) => linked(last.key, node.key)) : -1;
    chain.push(...left.splice(at >= 0 ? at : 0, 1));
  }
  return chain;
}

export class MapLayout {
  /** Hub order, first seen first; a hub keeps its place on the line. */
  private readonly hubOrder: string[] = [];
  /** Each machine's number in the order; lower stands earlier. */
  private readonly order = new Map<string, number>();
  private next = 0;
  /** As of the last update: where each hub's dashed ring waits. */
  private vacancies = new Map<string, Point>();

  constructor(private readonly storageKey: string | null) {
    const remembered = this.read();
    if (remembered) {
      this.hubOrder.push(...remembered.hubs);
      for (const [key, index] of Object.entries(remembered.places)) {
        this.order.set(key, index);
        this.next = Math.max(this.next, index + 1);
      }
    }
  }

  update(hubs: readonly MapHub[], nodes: readonly MapNode[], links: readonly MapLink[] = []): Placement {
    for (const hub of [...hubs].sort((a, b) => a.id.localeCompare(b.id))) if (!this.hubOrder.includes(hub.id)) this.hubOrder.push(hub.id);
    const live = new Set(hubs.map((hub) => hub.id));
    const order = this.hubOrder.filter((id) => live.has(id));
    // A machine seen for the first time takes the next number, in the order
    // that reads best; one seen before keeps its own.
    for (const node of [...nodes].sort(firstOrder)) {
      if (this.order.has(node.key)) continue;
      this.order.set(node.key, this.next);
      this.next += 1;
    }
    const groupOf = (node: MapNode) => node.hubs.filter((id) => live.has(id));
    const byOrder = (a: MapNode, b: MapNode) => this.order.get(a.key)! - this.order.get(b.key)!;
    const shared = nodes.filter((node) => groupOf(node).length > 1).sort(byOrder);
    const loose = nodes.filter((node) => !groupOf(node).length).sort(byOrder);

    // Round each hub: its groups in order, each a run of places, the
    // dashed ring after the account's own, and an empty place between
    // groups when there are several.
    const rounds = new Map(
      order.map((id, index) => {
        const own = nodes.filter((node) => groupOf(node).length === 1 && groupOf(node)[0] === id);
        const keys = [...new Set(own.map((node) => node.group?.key ?? "mine"))].sort((a, b) => {
          const rank = (key: string) => (key === "mine" ? 0 : key.startsWith("of:") ? 1 : 2);
          return rank(a) - rank(b) || a.localeCompare(b);
        });
        const groups = keys.map((key) => {
          const members = chained(own.filter((node) => (node.group?.key ?? "mine") === key).sort(byOrder), links);
          return { key, label: members[0]?.group?.label ?? "", members };
        });
        if (!keys.includes("mine")) groups.unshift({ key: "mine", label: "", members: [] });
        const slots: ({ node: MapNode } | { vacancy: true } | { gap: true })[] = [];
        const several = groups.filter((group) => group.members.length).length > 1;
        for (const group of groups) {
          slots.push(...group.members.map((node) => ({ node })));
          if (group.key === "mine") slots.push({ vacancy: true });
          if (several && (group.members.length || group.key === "mine")) slots.push({ gap: true });
        }
        const arcs = arcsOf(index > 0, index < order.length - 1);
        const total = arcs.reduce((sum, [, length]) => sum + length, 0);
        const radius = Math.max(FIRST, (slots.length * SPACING) / total);
        return [id, { arcs, slots, radius, groups: several ? groups.filter((group) => group.members.length) : [] }];
      }),
    );
    // Hubs on a line, far enough apart for the orbits on both sides and
    // the column between them.
    const hubPoints = new Map<string, Point>();
    let x = 0;
    order.forEach((id, index) => {
      if (index > 0) x += Math.max(HUB_GAP, rounds.get(order[index - 1]!)!.radius + rounds.get(id)!.radius + COLUMN * 2);
      hubPoints.set(id, [x, 0]);
    });

    const machines = new Map<string, Point>();
    const groups: GroupArc[] = [];
    this.vacancies = new Map();
    for (const id of order) {
      const knot = hubPoints.get(id)!;
      const round = rounds.get(id)!;
      const angles = new Map<string, number>();
      const step = (Math.PI * 2 * (round.arcs.reduce((sum, [, length]) => sum + length, 0) / (Math.PI * 2))) / round.slots.length;
      round.slots.forEach((slot, index) => {
        const angle = angleOn(round.arcs, index, round.slots.length);
        const at: Point = [knot[0] + Math.cos(angle) * round.radius, knot[1] + Math.sin(angle) * round.radius];
        if ("node" in slot) {
          machines.set(slot.node.key, at);
          angles.set(slot.node.key, angle);
        } else if ("vacancy" in slot) this.vacancies.set(id, at);
      });
      for (const group of round.groups) {
        const first = angles.get(group.members[0]!.key)!;
        const last = angles.get(group.members[group.members.length - 1]!.key)!;
        groups.push({ key: group.key, label: group.label, hub: id, centre: knot, radius: round.radius, from: first - step * 0.42, to: last + step * 0.42 });
      }
    }
    // On several hubs: in a column at the middle of their knots, alternately
    // below and above the line between them.
    const columns = new Map<string, number>();
    for (const node of shared) {
      const knots = groupOf(node).map((id) => hubPoints.get(id)!);
      const middle: Point = [knots.reduce((sum, p) => sum + p[0], 0) / knots.length, knots.reduce((sum, p) => sum + p[1], 0) / knots.length];
      const column = groupOf(node).sort().join("+");
      const index = columns.get(column) ?? 0;
      columns.set(column, index + 1);
      const step = Math.ceil((index + 1) / 2) * COLUMN;
      machines.set(node.key, [middle[0], middle[1] + (index % 2 ? -step : step)]);
    }
    // No hub to hang on: a row, as before any network is joined.
    loose.forEach((node, index) => machines.set(node.key, [index * (RING * 2 + SPACING), FIRST]));

    this.write(new Set(nodes.map((node) => node.key)));
    // Room round everything for the names, and for the groups' names
    // written outside their arcs.
    const points = [...hubPoints.values(), ...machines.values(), ...this.vacancies.values()];
    const pad = RING + (groups.length ? 150 : 70);
    const xs = points.map((p) => p[0]);
    const ys = points.map((p) => p[1]);
    const bounds = points.length
      ? { x: Math.min(...xs) - pad, y: Math.min(...ys) - pad, width: Math.max(...xs) - Math.min(...xs) + pad * 2, height: Math.max(...ys) - Math.min(...ys) + pad * 2 }
      : { x: -pad, y: -pad, width: pad * 2, height: pad * 2 };
    return { hubs: hubPoints, machines, groups, bounds };
  }

  /** Where the next machine to come to `hub` alone will stand, as of the
   *  last update: the place the dashed ring that adds one waits in. */
  vacant(hub: string): Point | null {
    return this.vacancies.get(hub) ?? null;
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

  /** Keeps the order of the machines there now; one gone takes a new place
   *  if it ever comes back on another day. */
  private write(present: ReadonlySet<string>): void {
    if (!this.storageKey) return;
    const places: Record<string, number> = {};
    for (const [key, index] of this.order) if (present.has(key)) places[key] = index;
    try {
      localStorage.setItem(this.storageKey, JSON.stringify({ hubs: this.hubOrder, places } satisfies Remembered));
    } catch {
      // Not kept: the drawing is laid out afresh the next time.
    }
  }
}
