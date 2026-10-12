// Where the machines page draws each hub and machine (`world-gui.md`, 账号、
// 机器、共享与借出): a tree with square forks, read from the top. The hub
// stands at the top; under it, joined by a bus, each group of machines by
// whose they are: the account's own first, then each other account's shared
// with it, then those an agent is lent on from another. Under each group's
// knot its machines hang from a trunk as in a file tree, one to a row, their
// names to the right; a group of many is laid in several such columns side
// by side, and a group can be folded to its knot alone, which tells how
// many it holds. The account's own end with the dashed ring that adds a
// machine, so one that comes stands where it waited. Positions are computed,
// never moved by hand; the order within a group is remembered on this
// device, so machines keep their order while others come and go.

import type { Bounds, Point } from "../map-canvas";
import type { MapHub, MapLink, MapNode } from "./network-map";

/** A machine's ring and a hub's knot, in the drawing's units. */
export const RING = 26;
export const KNOT = 8;
/** A group's knot on the tree. */
export const GROUP_KNOT = 6;
/** From the hub down to the bus, and from the bus down to the groups'
 *  knots. */
const BUS_DROP = 90;
const GROUP_DROP = 70;
/** From a group's knot down to its first machine, and between machines. */
const FIRST_ROW = 112;
const ROW = 84;
/** A column of machines: its trunk, the ring to its right, and the name
 *  beside the ring. */
const COLUMN = 280;
const TWIG = 44;
/** At most this many machines stand in one column before the next. */
const ROWS = 10;
/** Room between groups, and between the trees of two hubs. */
const GROUP_GAP = 70;
const HUB_GAP = 220;

/** A group of machines under a hub: its knot, and what its label says. */
export interface GroupNode {
  key: string;
  hub: string;
  label: string;
  count: number;
  online: number;
  folded: boolean;
  point: Point;
}

/** A line of the tree, square at its corners; dashed where the machine at
 *  its end is out of reach. */
export interface Branch {
  points: Point[];
  /** The machine at its end, when it is a machine's twig. */
  machine?: string | undefined;
  hub: string;
  out: boolean;
}

export interface Placement {
  hubs: ReadonlyMap<string, Point>;
  machines: ReadonlyMap<string, Point>;
  groups: GroupNode[];
  branches: Branch[];
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

/** The order of groups under a hub: the account's own, others', lent. */
function groupRank(key: string): number {
  return key === "mine" ? 0 : key.startsWith("of:") ? 1 : 2;
}

/** Puts machines linked directly next to each other, keeping the order
 *  they came in otherwise. */
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

  /** `folded`: the groups shown as their knot alone, by key. */
  update(hubs: readonly MapHub[], nodes: readonly MapNode[], links: readonly MapLink[] = [], folded: ReadonlySet<string> = new Set()): Placement {
    for (const hub of [...hubs].sort((a, b) => a.id.localeCompare(b.id))) if (!this.hubOrder.includes(hub.id)) this.hubOrder.push(hub.id);
    const live = new Set(hubs.map((hub) => hub.id));
    const order = this.hubOrder.filter((id) => live.has(id));
    for (const node of [...nodes].sort(firstOrder)) {
      if (this.order.has(node.key)) continue;
      this.order.set(node.key, this.next);
      this.next += 1;
    }
    const byOrder = (a: MapNode, b: MapNode) => this.order.get(a.key)! - this.order.get(b.key)!;
    // A machine on several hubs hangs under the first of them.
    const hubOf = (node: MapNode) => order.find((id) => node.hubs.includes(id)) ?? null;

    const hubPoints = new Map<string, Point>();
    const machines = new Map<string, Point>();
    const groups: GroupNode[] = [];
    const branches: Branch[] = [];
    this.vacancies = new Map();
    let left = 0;
    for (const id of order) {
      const own = nodes.filter((node) => hubOf(node) === id);
      const keys = [...new Set(["mine", ...own.map((node) => node.group?.key ?? "mine")])].sort((a, b) => groupRank(a) - groupRank(b) || a.localeCompare(b));
      const blocks = keys.map((key) => {
        const members = chained(own.filter((node) => (node.group?.key ?? "mine") === key).sort(byOrder), links);
        const groupKey = `group:${id}:${key}`;
        const fold = folded.has(groupKey) && members.length > 0;
        // The account's own end with the dashed ring.
        const rows = fold ? 0 : members.length + (key === "mine" ? 1 : 0);
        // A folded group keeps room for its name and count beside its knot.
        const columns = fold ? 2 : Math.max(1, Math.ceil(rows / ROWS));
        return { key, groupKey, members, fold, rows, columns, width: columns * COLUMN };
      });
      const width = blocks.reduce((sum, block) => sum + block.width, 0) + GROUP_GAP * (blocks.length - 1);
      const knot: Point = [left + width / 2, 0];
      hubPoints.set(id, knot);
      const busY = BUS_DROP;
      const groupY = BUS_DROP + GROUP_DROP;
      let x = left;
      const drops: number[] = [];
      for (const block of blocks) {
        // The group's knot stands over its first trunk.
        const trunk = x + 24;
        const point: Point = [trunk, groupY];
        drops.push(trunk);
        const online = block.members.filter((node) => node.online === true).length;
        groups.push({ key: block.groupKey, hub: id, label: block.members[0]?.group?.label ?? "", count: block.members.length, online, folded: block.fold, point });
        branches.push({ points: [[trunk, busY], [trunk, groupY - GROUP_KNOT - 3]], hub: id, out: false });
        if (!block.fold) {
          const slots: (MapNode | null)[] = [...block.members, ...(block.key === "mine" ? [null] : [])];
          for (let column = 0; column < block.columns; column += 1) {
            const inColumn = slots.slice(column * ROWS, (column + 1) * ROWS);
            if (!inColumn.length) continue;
            const columnX = trunk + column * COLUMN;
            const top = groupY + GROUP_KNOT + 3;
            // Columns after the first are reached by a short bus under the
            // group's knot.
            const start = column === 0 ? top : groupY + FIRST_ROW / 2;
            if (column > 0) branches.push({ points: [[trunk, groupY + FIRST_ROW / 2], [columnX, groupY + FIRST_ROW / 2]], hub: id, out: false });
            const lastY = groupY + FIRST_ROW + (inColumn.length - 1) * ROW;
            branches.push({ points: [[columnX, start], [columnX, lastY]], hub: id, out: false });
            inColumn.forEach((slot, row) => {
              const y = groupY + FIRST_ROW + row * ROW;
              const at: Point = [columnX + TWIG + RING, y];
              if (slot) machines.set(slot.key, at);
              else this.vacancies.set(id, at);
              branches.push({ points: [[columnX, y], [columnX + TWIG - 4, y]], machine: slot?.key, hub: id, out: !!slot && slot.online !== true });
            });
          }
        }
        x += block.width + GROUP_GAP;
      }
      // The hub down to the bus, and the bus across the groups' knots.
      branches.push({ points: [[knot[0], KNOT + 4], [knot[0], busY]], hub: id, out: false });
      if (drops.length) branches.push({ points: [[Math.min(knot[0], ...drops), busY], [Math.max(knot[0], ...drops), busY]], hub: id, out: false });
      left += width + HUB_GAP;
    }
    // No hub to hang on: a row, as before any network is joined.
    nodes.filter((node) => !hubOf(node)).sort(byOrder).forEach((node, index) => machines.set(node.key, [index * COLUMN, FIRST_ROW]));

    this.write(new Set(nodes.map((node) => node.key)));
    // Room round everything: names stand to the right of the rings, and
    // the hub's name above it.
    const points = [...hubPoints.values(), ...machines.values(), ...this.vacancies.values(), ...groups.map((group) => group.point)];
    const xs = points.map((p) => p[0]);
    const ys = points.map((p) => p[1]);
    const bounds = points.length
      ? { x: Math.min(...xs) - 80, y: Math.min(...ys) - 90, width: Math.max(...xs) - Math.min(...xs) + 80 + COLUMN - TWIG, height: Math.max(...ys) - Math.min(...ys) + 90 + 80 }
      : { x: -100, y: -100, width: 200, height: 200 };
    return { hubs: hubPoints, machines, groups, branches, bounds };
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
