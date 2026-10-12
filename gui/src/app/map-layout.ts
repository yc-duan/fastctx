// Where the machines page draws each hub and machine (`world-gui.md`, 账号、
// 机器、共享与借出), read from the top. The hub stands at the top; under it,
// joined by a bus of square lines, a box for each group of machines by
// whose they are, as a comment box gathers nodes in a blueprint: the
// account's own first, then each other account's shared with it, then those
// an agent is lent on from another. A box has a title strip with the
// group's name, and its machines stand in a grid inside, one to a row and a
// new column every ten, their names to the right, each joined by a short twig
// to a trunk down the left of its column. A box folds to its title
// strip alone. The account's own end with the dashed ring that adds a
// machine, so one that comes stands where it waited. Positions are computed,
// never moved by hand; the order within a group is remembered on this
// device, so machines keep their order while others come and go.

import type { Bounds, Point } from "../map-canvas";
import type { MapHub, MapLink, MapNode } from "./network-map";

/** A machine's ring and a hub's knot, in the drawing's units. */
export const RING = 24;
export const KNOT = 8;
/** A box's title strip. */
export const TITLE = 34;
/** From the hub down to the bus, and from the bus down to the boxes. */
const BUS_DROP = 70;
const BOX_DROP = 44;
/** Inside a box: the margin round its grid, the trunk down a column and
 *  the twig from it to a ring, the rows, the columns, and the room a name
 *  takes to the right of its ring. */
const PAD = 20;
const TRUNK = 16;
const TWIG = 22;
const ROW = 74;
const COLUMN = 230;
const NAME = 150;
/** At most this many machines stand in one column before the next. */
const ROWS = 10;
/** A folded box: its title strip, this wide. */
const FOLDED = 200;
/** Room between boxes, and between the trees of two hubs. */
const BOX_GAP = 36;
const HUB_GAP = 160;

/** A group of machines under a hub, drawn as a box. */
export interface GroupNode {
  key: string;
  hub: string;
  /** "mine", "of:<account>" or "lent:<account>". */
  kind: string;
  label: string;
  count: number;
  online: number;
  folded: boolean;
  /** The box; its title strip is its top. */
  box: Bounds;
  /** Where the box is chosen: its title. */
  point: Point;
}

/** A line of the tree, square at its corners: the hub's down to the
 *  boxes, a column's trunk, and the twig to each machine, broken where the
 *  machine is out of reach. */
export interface Branch {
  points: Point[];
  hub: string;
  /** The machine a twig reaches. */
  machine?: string | undefined;
  out?: boolean;
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

  /** `folded`: the boxes shown as their title strip alone, by key. */
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
    // A machine on several hubs stands under the first of them.
    const hubOf = (node: MapNode) => order.find((id) => node.hubs.includes(id)) ?? null;

    const hubPoints = new Map<string, Point>();
    const machines = new Map<string, Point>();
    const groups: GroupNode[] = [];
    const branches: Branch[] = [];
    this.vacancies = new Map();
    let left = 0;
    for (const id of order) {
      const own = nodes.filter((node) => hubOf(node) === id);
      const kinds = [...new Set(["mine", ...own.map((node) => node.group?.key ?? "mine")])].sort((a, b) => groupRank(a) - groupRank(b) || a.localeCompare(b));
      const blocks = kinds.map((kind) => {
        const members = chained(own.filter((node) => (node.group?.key ?? "mine") === kind).sort(byOrder), links);
        const key = `group:${id}:${kind}`;
        const fold = folded.has(key) && members.length > 0;
        // The account's own end with the dashed ring.
        const cells = members.length + (kind === "mine" ? 1 : 0);
        const columns = Math.max(1, Math.ceil(cells / ROWS));
        const rows = Math.min(cells, ROWS);
        const width = fold ? FOLDED : TRUNK + TWIG + PAD + (columns - 1) * COLUMN + RING * 2 + NAME;
        const height = fold ? TITLE : TITLE + PAD * 2 + (rows - 1) * ROW + RING * 2;
        return { kind, key, members, fold, width, height };
      });
      const width = blocks.reduce((sum, block) => sum + block.width, 0) + BOX_GAP * (blocks.length - 1);
      const knot: Point = [left + width / 2, 0];
      hubPoints.set(id, knot);
      const busY = BUS_DROP;
      const top = BUS_DROP + BOX_DROP;
      let x = left;
      const drops: number[] = [];
      for (const block of blocks) {
        const box: Bounds = { x, y: top, width: block.width, height: block.height };
        const online = block.members.filter((node) => node.online === true).length;
        groups.push({ key: block.key, hub: id, kind: block.kind, label: block.members[0]?.group?.label ?? "", count: block.members.length, online, folded: block.fold, box, point: [x + PAD + 30, top + TITLE / 2] });
        const drop = x + block.width / 2;
        drops.push(drop);
        branches.push({ points: [[drop, busY], [drop, top]], hub: id });
        if (!block.fold) {
          const slots: (MapNode | null)[] = [...block.members, ...(block.kind === "mine" ? [null] : [])];
          const columns = Math.ceil(slots.length / ROWS);
          for (let column = 0; column < columns; column += 1) {
            const trunk = x + TRUNK + column * COLUMN;
            const last = Math.min(ROWS, slots.length - column * ROWS) - 1;
            branches.push({ points: [[trunk, top + TITLE], [trunk, top + TITLE + PAD + RING + last * ROW]], hub: id });
          }
          slots.forEach((slot, index) => {
            const column = Math.floor(index / ROWS);
            const row = index % ROWS;
            const trunk = x + TRUNK + column * COLUMN;
            const at: Point = [trunk + TWIG + RING, top + TITLE + PAD + RING + row * ROW];
            branches.push({ points: [[trunk, at[1]], [trunk + TWIG - 3, at[1]]], hub: id, machine: slot?.key, out: !!slot && slot.online !== true });
            if (slot) machines.set(slot.key, at);
            else this.vacancies.set(id, at);
          });
        }
        x += block.width + BOX_GAP;
      }
      // The hub down to the bus, and the bus across the drops to the boxes.
      branches.push({ points: [[knot[0], KNOT + 4], [knot[0], busY]], hub: id });
      if (drops.length) branches.push({ points: [[Math.min(knot[0], ...drops), busY], [Math.max(knot[0], ...drops), busY]], hub: id });
      left += width + HUB_GAP;
    }
    // No hub to hang on: a row, as before any network is joined.
    nodes.filter((node) => !hubOf(node)).sort(byOrder).forEach((node, index) => machines.set(node.key, [index * COLUMN, TITLE]));

    this.write(new Set(nodes.map((node) => node.key)));
    // Room round everything: the hub's name above it, the boxes below.
    const xs = [...hubPoints.values(), ...machines.values()].map((p) => p[0]).concat(groups.flatMap((group) => [group.box.x, group.box.x + group.box.width]));
    const ys = [...hubPoints.values(), ...machines.values()].map((p) => p[1]).concat(groups.flatMap((group) => [group.box.y, group.box.y + group.box.height]));
    const bounds = xs.length
      ? { x: Math.min(...xs) - 40, y: Math.min(...ys) - 70, width: Math.max(...xs) - Math.min(...xs) + 80, height: Math.max(...ys) - Math.min(...ys) + 110 }
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
