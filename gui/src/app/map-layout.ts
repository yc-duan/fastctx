// Where the machines page draws each hub and machine (`world-gui.md`, 账号、
// 机器、共享与协助), read from the top. The hub stands at the top; under it,
// joined by a bus of square lines, a box for each group of machines by
// whose they are, as a comment box gathers nodes in a blueprint: the
// account's own first, then each other account's shared with it, then those
// whose agent assists from another. For the network's administrator, every
// person has a box, a title strip alone while none of their machines has
// come, and a dashed box after them adds another person. A box has a title
// strip with the group's name, and its machines stand in a grid inside, one
// to a row and a
// new column every ten, their names to the right. The box lies under the
// lines and does not break them: the line down from the bus runs on through
// the title strip as the trunk of the first column, the other columns branch
// from it under the strip, and a short twig reaches each machine. A box folds to its title
// strip alone, as wide as it is open, so folding moves nothing but what lies below it. The account's own end with the dashed ring that adds a
// machine, so one that comes stands where it waited. On a narrow screen, a
// phone's, the boxes stand one under another instead, each its machines in
// one column, joined from the left by a spine down from the hub, so the
// drawing is read by moving down. Positions are computed,
// never moved by hand; the order within a group is remembered on this
// device, so machines keep their order while others come and go.

import type { Bounds, Point } from "../map-canvas";
import type { MapHub, MapLink, MapNode, MapPerson } from "./map-nodes";

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
export const TRUNK = 16;
const TWIG = 22;
const ROW = 74;
const COLUMN = 230;
const NAME = 150;
/** How tall a machine's name and the line under it stand, at their size. */
const LABEL = 28;
/** At most this many machines stand in one column before the next. */
export const ROWS = 10;
/** On a narrow screen: how far right of the spine the boxes stand. */
const SIDE = 34;
/** How wide the dashed box that adds a person is, past its trunk. */
const ADDING = 150;
/** Room between boxes, and between the trees of two hubs. */
const BOX_GAP = 36;
const HUB_GAP = 160;

/** A group of machines under a hub, drawn as a box. */
export interface GroupNode {
  key: string;
  hub: string;
  /** "mine", "of:<account>", "assist:<account>", or "adding" for the
   *  dashed box that adds a person. */
  kind: string;
  label: string;
  count: number;
  online: number;
  folded: boolean;
  /** The box; its title strip is its top. */
  box: Bounds;
  /** Where the box is chosen: its title. */
  point: Point;
  /** The person whose box it is, for the network's administrator. */
  person?: MapPerson | undefined;
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

/** How a drawing is laid out, past its hubs and machines. */
export interface LayoutChoices {
  /** Direct links, which put linked machines side by side. */
  links?: readonly MapLink[];
  /** The boxes shown as their title strip alone, by key. */
  folded?: ReadonlySet<string>;
  /** Laid one under another for a phone. */
  narrow?: boolean;
  /** How many machines stand in a column before the next. */
  rows?: number;
  /** Whether the account's own box ends with the place the dashed ring that
   *  adds a machine waits in. */
  vacant?: boolean;
  /** How many times their size the names are drawn: a still drawing drawn
   *  small draws them larger to keep them readable, and they take the room
   *  they need. */
  grow?: number;
  /** The room a name takes beside its ring at its size: what a still
   *  drawing measures of its own names, the widest a name may be otherwise. */
  room?: number;
  /** For the network's administrator: every person, each a box even with
   *  no machine yet. */
  people?: readonly MapPerson[];
  /** The hubs whose people end with the dashed box that adds a person. */
  adding?: ReadonlySet<string>;
}

interface Remembered {
  hubs: string[];
  /** Each machine's place in its group's order. */
  places: Record<string, number>;
}

const HOLD_RANK: Record<MapNode["hold"], number> = { mine: 0, others: 1, visitor: 1, assist: 2 };

/** The order a machine first takes in its group: this computer, then by
 *  name. */
function firstOrder(a: MapNode, b: MapNode): number {
  return Number(b.here) - Number(a.here) || HOLD_RANK[a.hold] - HOLD_RANK[b.hold] || a.name.localeCompare(b.name);
}

/** The order of groups under a hub: the account's own, others', the box
 *  that adds a person, those assisting. */
function groupRank(key: string): number {
  return key === "mine" ? 0 : key.startsWith("of:") ? 1 : key === "adding" ? 2 : 3;
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

  update(hubs: readonly MapHub[], nodes: readonly MapNode[], choices: LayoutChoices = {}): Placement {
    const { links = [], folded = new Set<string>(), narrow = false, rows = ROWS, vacant = true, grow = 1, room = NAME, people = [], adding = new Set<string>() } = choices;
    // The room a name takes beside its ring, across a column and down a
    // row, as large as it is drawn.
    const NAME_AT = room * grow;
    const COLUMN_AT = TWIG + RING * 2 + (room + COLUMN - TWIG - RING * 2 - NAME) * grow;
    const ROW_AT = Math.max(ROW, LABEL * grow + ROW - RING * 2);
    const ending = (kind: string) => vacant && kind === "mine";
    for (const hub of [...hubs].sort((a, b) => a.id.localeCompare(b.id))) if (!this.hubOrder.includes(hub.id)) this.hubOrder.push(hub.id);
    const live = new Set(hubs.map((hub) => hub.id));
    const order = this.hubOrder.filter((id) => live.has(id));
    for (const node of [...nodes].sort(firstOrder)) {
      if (this.order.has(node.key)) continue;
      // Renamed, it keeps the place it had under a name it had.
      const was = node.was?.find((key) => this.order.has(key) && !nodes.some((other) => other.key === key));
      if (was !== undefined) {
        this.order.set(node.key, this.order.get(was)!);
        this.order.delete(was);
        continue;
      }
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
    let below = 0;
    for (const id of order) {
      const own = nodes.filter((node) => hubOf(node) === id);
      const persons = people.filter((person) => person.hub === id);
      const kinds = [
        ...new Set([
          "mine",
          ...own.map((node) => node.group?.key ?? "mine"),
          ...persons.map((person) => `of:${person.account}`),
          ...(adding.has(id) ? ["adding"] : []),
        ]),
      ].sort((a, b) => groupRank(a) - groupRank(b) || a.localeCompare(b));
      const blocks = kinds.map((kind) => {
        const members = chained(own.filter((node) => (node.group?.key ?? "mine") === kind).sort(byOrder), links);
        const person = persons.find((one) => kind === `of:${one.account}`);
        const label = members[0]?.group?.label ?? person?.label ?? "";
        // The dashed box that adds a person is a title strip alone.
        if (kind === "adding") return { kind, key: `adding:${id}`, members, label, person, fold: false, strip: true, width: TRUNK + 20 + ADDING * grow, height: TITLE, perColumn: 1 };
        const key = `group:${id}:${kind}`;
        const fold = folded.has(key) && members.length > 0;
        // The account's own end with the dashed ring.
        const cells = members.length + (ending(kind) ? 1 : 0);
        const perColumn = narrow ? Math.max(1, cells) : rows;
        const columns = Math.max(1, Math.ceil(cells / perColumn));
        const tall = Math.min(cells, perColumn);
        const width = TRUNK + TWIG + PAD + (columns - 1) * COLUMN_AT + RING * 2 + NAME_AT;
        // A person none of whose machines has come is the strip alone.
        const strip = fold || cells === 0;
        const height = strip ? TITLE : TITLE + PAD * 2 + (tall - 1) * ROW_AT + RING * 2;
        return { kind, key, members, label, person, fold, strip, width, height, perColumn };
      });
      if (narrow) {
        // The hub at the top left, a spine down its left, and each box to
        // its right, joined at the middle of its title strip.
        const knot: Point = [0, below];
        hubPoints.set(id, knot);
        let y = below + BUS_DROP;
        let join = y;
        for (const block of blocks) {
          const box: Bounds = { x: SIDE, y, width: block.width, height: block.height };
          const online = block.members.filter((node) => node.online === true).length;
          groups.push({ key: block.key, hub: id, kind: block.kind, label: block.label, count: block.members.length, online, folded: block.fold, box, point: [SIDE + TRUNK + 50, y + TITLE / 2], person: block.person });
          join = y + TITLE / 2;
          const trunk = SIDE + TRUNK;
          branches.push({ points: [[0, join], [trunk, join]], hub: id, out: block.kind === "adding" });
          if (!block.strip) {
            const slots: (MapNode | null)[] = [...block.members, ...(ending(block.kind) ? [null] : [])];
            branches.push({ points: [[trunk, join], [trunk, y + TITLE + PAD + RING + (slots.length - 1) * ROW_AT]], hub: id });
            slots.forEach((slot, row) => {
              const at: Point = [trunk + TWIG + RING, y + TITLE + PAD + RING + row * ROW_AT];
              branches.push({ points: [[trunk, at[1]], [trunk + TWIG - 3, at[1]]], hub: id, machine: slot?.key, out: !!slot && slot.online !== true });
              if (slot) machines.set(slot.key, at);
              else this.vacancies.set(id, at);
            });
          }
          y += block.height + BOX_GAP;
        }
        branches.push({ points: [[0, below + KNOT + 4], [0, join]], hub: id });
        below = y + HUB_GAP;
        continue;
      }
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
        groups.push({ key: block.key, hub: id, kind: block.kind, label: block.label, count: block.members.length, online, folded: block.fold, box, point: [x + TRUNK + 50, top + TITLE / 2], person: block.person });
        // The line from the bus comes down at the first trunk; to a strip
        // alone it stops in the middle of it, broken to the box that adds a
        // person.
        const drop = x + TRUNK;
        drops.push(drop);
        if (block.strip) branches.push({ points: [[drop, busY], [drop, top + TITLE / 2]], hub: id, out: block.kind === "adding" });
        else {
          const slots: (MapNode | null)[] = [...block.members, ...(ending(block.kind) ? [null] : [])];
          const columns = Math.ceil(slots.length / rows);
          const fork = top + TITLE + PAD * 0.55;
          for (let column = 0; column < columns; column += 1) {
            const trunk = x + TRUNK + column * COLUMN_AT;
            const last = Math.min(rows, slots.length - column * rows) - 1;
            branches.push({ points: [[trunk, column ? fork : busY], [trunk, top + TITLE + PAD + RING + last * ROW_AT]], hub: id });
          }
          if (columns > 1) branches.push({ points: [[drop, fork], [x + TRUNK + (columns - 1) * COLUMN_AT, fork]], hub: id });
          slots.forEach((slot, index) => {
            const column = Math.floor(index / rows);
            const row = index % rows;
            const trunk = x + TRUNK + column * COLUMN_AT;
            const at: Point = [trunk + TWIG + RING, top + TITLE + PAD + RING + row * ROW_AT];
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
    nodes.filter((node) => !hubOf(node)).sort(byOrder).forEach((node, index) => machines.set(node.key, [index * COLUMN_AT, TITLE]));

    this.write(new Set(nodes.map((node) => node.key)));
    // Room round everything: the hub's name above it, as large as it is
    // drawn, the boxes below.
    const above = 40 + 30 * grow;
    const xs = [...hubPoints.values(), ...machines.values()].map((p) => p[0]).concat(groups.flatMap((group) => [group.box.x, group.box.x + group.box.width]));
    const ys = [...hubPoints.values(), ...machines.values()].map((p) => p[1]).concat(groups.flatMap((group) => [group.box.y, group.box.y + group.box.height]));
    const bounds = xs.length
      ? { x: Math.min(...xs) - 40, y: Math.min(...ys) - above, width: Math.max(...xs) - Math.min(...xs) + 80, height: Math.max(...ys) - Math.min(...ys) + above + 40 }
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
