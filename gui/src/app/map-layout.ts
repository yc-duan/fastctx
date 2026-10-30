// Where the machines page draws each hub and machine (`world-gui.md`, 账号、
// 机器、共享与协助), read from the top left. The hub stands at the top left
// and a trunk runs down from it; the boxes, one for each group of machines
// by whose they are, stand in rows to its right, each row hanging from a bus
// off the trunk, as comment boxes gather nodes in a blueprint: the account's
// own first, then each other account's shared with it, then those whose
// agent assists from another. For the network's administrator every person
// has a box, a title strip alone while none of their machines has come, and
// a dashed box after them adds another person. A box has a title strip with
// whose it is; inside, its machines in no group, then each group its owner
// made (`world-network.md`, 机器的分组) under a band with the group's name,
// stand in columns of up to ten with their names to the right. These parts
// stand side by side and wrap into rows of their own where a box would be
// wider than the drawing's rows; the rows of the drawing are as wide as make
// it, whole, about the shape of its frame, so a network of thousands is laid
// out as readily as one of a few, and how much of it is drawn is the
// drawing's distance (`machine-map.ts`). A box lies under the lines and does
// not break them: the line down from the bus runs on through the title strip
// as the trunk of the first column, the other columns, a group's through its
// band, branch from a fork over them, and a short twig reaches each machine.
// The account's own machines in no group end with the dashed ring that adds
// a machine, so one that comes stands where it waited. Positions are
// computed, never moved by hand; the order within a group is remembered on
// this device, so machines keep their order while others come and go.

import type { Bounds, Point } from "../map-canvas";
import { compareBoxes } from "./machine-groups";
import type { MapHub, MapLink, MapNode, MapPerson } from "./map-nodes";

/** A machine's ring and a hub's knot, in the drawing's units. */
export const RING = 24;
export const KNOT = 8;
/** A box's title strip. */
export const TITLE = 34;
/** From the hub down to the first bus, and from a bus down to its boxes. */
const BUS_DROP = 70;
const BOX_DROP = 44;
/** Under a row of boxes, the room before the next row's bus. */
const ROW_GAP = 30;
/** Inside a box: the margin round its grid, the trunk down a column and
 *  the twig from it to a ring, the rows, the columns, and the room a name
 *  takes to the right of its ring. */
const PAD = 20;
export const TRUNK = 16;
const TWIG = 22;
export const ROW = 74;
const COLUMN = 230;
const NAME = 150;
/** At most this many machines stand in one column before the next. */
export const ROWS = 10;
/** How far right of the trunk the boxes stand. */
const SIDE = 34;
/** How wide the dashed box that adds a person is, past its trunk. */
const ADDING = 150;
/** Room between boxes in a row, between the rows of parts in a box, and
 *  between the trees of two hubs. */
const BOX_GAP = 36;
const PART_GAP = 10;
const HUB_GAP = 160;
/** A group's band in its owner's box: how far under the fork it hangs, and
 *  how far under it the first rings stand. */
export const BAND = 28;
const BAND_TOP = 18;
const BAND_UNDER = 14;
/** Under a group's machines, the room its band's box leaves to the next. */
const BAND_EDGE = 8;
/** The room on a strip or a band after its name: how many machines, how
 *  many online, and its "⋯". */
const COUNT_ROOM = 150;

/** A group of machines under a hub, drawn as a box; or a group in a box,
 *  drawn as a band over its machines. */
export interface GroupNode {
  key: string;
  hub: string;
  /** "mine", "of:<account>", "assist:<account>", or "adding" for the
   *  dashed box that adds a person; a group's, its owner's and its name
   *  (`boxParts`). */
  kind: string;
  label: string;
  count: number;
  online: number;
  /** How many of its machines are online with agents at work or waiting. */
  busy: number;
  /** The box; its title strip is its top. A group's is its band and its
   *  machines under it. */
  box: Bounds;
  /** How tall its title strip, or a group's band, is. */
  strip: number;
  /** Where the box is chosen: its title. */
  point: Point;
  /** The person whose box it is, for the network's administrator. */
  person?: MapPerson | undefined;
  /** For a group: the key of its owner's box, which it stands in. */
  within?: string | undefined;
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
  /** The hub's own: its trunk down and the line over each row of boxes,
   *  drawn however far the drawing is seen from; the rest only nearer. */
  trunk?: boolean;
}

export interface Placement {
  hubs: ReadonlyMap<string, Point>;
  machines: ReadonlyMap<string, Point>;
  groups: GroupNode[];
  branches: Branch[];
  /** The box each machine stands in, by their keys. */
  boxOf: ReadonlyMap<string, string>;
  /** Where each hub's dashed ring that adds a machine waits: where the next
   *  machine to come to the hub alone will stand. */
  vacant: ReadonlyMap<string, Point>;
  bounds: Bounds;
  /** The room a machine's name takes beside its ring. */
  room: number;
}

/** How a drawing is laid out, past its hubs and machines. */
export interface LayoutChoices {
  /** Direct links, which put linked machines side by side. */
  links?: readonly MapLink[];
  /** How many machines stand in a column before the next. */
  rows?: number;
  /** Whether the account's own box ends with the place the dashed ring that
   *  adds a machine waits in. */
  vacant?: boolean;
  /** The room a name takes beside its ring: what a still drawing measures of
   *  its own names, the widest a name may be otherwise. */
  room?: number;
  /** How wide a box's or a group's name is at its own size: its strip is
   *  never shorter than its name and what follows it. */
  label?: (text: string) => number;
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

/** The order of boxes under a hub: the account's own, others', the box
 *  that adds a person, those assisting; inside each, its owner's groups by
 *  name (`machine-groups.ts`). */
function groupRank(owner: string): number {
  return owner === "mine" ? 0 : owner.startsWith("of:") ? 1 : owner === "adding" ? 2 : 3;
}

/** A box's machines in no group, or one of its groups: machines in columns,
 *  the slot of the dashed ring among them. */
interface Part {
  key: string;
  label: string;
  /** A group's, under its band; else its box's machines in no group. */
  band: boolean;
  slots: (MapNode | null)[];
  perColumn: number;
  columns: number;
  /** How far across it reaches: its columns, and for a group no less than
   *  its name and what follows it on its band. */
  span: number;
  count: number;
  online: number;
  busy: number;
}

/** A box as prepared, before it is placed. */
interface Block {
  kind: string;
  key: string;
  label: string;
  person?: MapPerson | undefined;
  parts: Part[];
  /** A person with no machine yet, or the box that adds one: the title
   *  strip alone. */
  strip: boolean;
  /** As narrow as its name and what follows it let it be. */
  least: number;
  count: number;
  online: number;
  busy: number;
}

/** The hubs and their boxes, ready to be placed at any width. */
export interface Prepared {
  trees: { hub: string; blocks: Block[] }[];
  /** Machines on no hub, which stand in a row. */
  homeless: MapNode[];
  /** A column's pitch, and the room a name takes beside its ring. */
  column: number;
  room: number;
}

/** A row of parts in a box. */
interface Shelf {
  parts: Part[];
  width: number;
  height: number;
  banded: boolean;
}

/** A box as it is placed in a row: its rows of parts and its size. */
interface Sized {
  block: Block;
  shelves: Shelf[];
  width: number;
  height: number;
}

/** A row of boxes, hanging from its bus. */
interface Row {
  bus: number;
  top: number;
  boxes: { sized: Sized; x: number }[];
}

/** Each machine's direct links, by the key of the machine at the other end. */
function nearOf(links: readonly MapLink[]): Map<string, Set<string>> {
  const near = new Map<string, Set<string>>();
  const add = (from: string, to: string) => {
    const known = near.get(from);
    if (known) known.add(to);
    else near.set(from, new Set([to]));
  };
  for (const link of links) {
    add(link.a, link.b);
    add(link.b, link.a);
  }
  return near;
}

/** Puts machines linked directly next to each other, keeping the order
 *  they came in otherwise. */
function chained(ordered: MapNode[], near: Map<string, Set<string>>): MapNode[] {
  if (!ordered.some((node) => near.has(node.key))) return [...ordered];
  const left = [...ordered];
  const chain: MapNode[] = [];
  while (left.length) {
    const last = chain[chain.length - 1];
    const at = last ? left.findIndex((node) => near.get(last.key)?.has(node.key)) : -1;
    chain.push(...left.splice(at >= 0 ? at : 0, 1));
  }
  return chain;
}

const onlineOf = (nodes: readonly MapNode[]) => nodes.filter((node) => node.online === true).length;
const busyOf = (nodes: readonly MapNode[]) => nodes.filter((node) => node.online === true && node.busy > 0).length;

export class MapLayout {
  /** Hub order, first seen first; a hub keeps its place. */
  private readonly hubOrder: string[] = [];
  /** Each machine's number in the order; lower stands earlier. Kept on
   *  the device when it changed, or the machines there did. */
  private readonly order = new Map<string, number>();
  private next = 0;
  private changed = true;
  private kept = -1;

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

  /** The boxes and their parts, in their order, sized but not placed. */
  prepare(hubs: readonly MapHub[], nodes: readonly MapNode[], choices: LayoutChoices = {}): Prepared {
    const { links = [], rows = ROWS, vacant = true, room = NAME, people = [], adding = new Set<string>(), label } = choices;
    const column = COLUMN - NAME + room;
    for (const hub of [...hubs].sort((a, b) => a.id.localeCompare(b.id))) if (!this.hubOrder.includes(hub.id)) this.hubOrder.push(hub.id);
    const live = new Set(hubs.map((hub) => hub.id));
    const order = this.hubOrder.filter((id) => live.has(id));
    const present = new Set(nodes.map((node) => node.key));
    for (const node of nodes.filter((one) => !this.order.has(one.key)).sort(firstOrder)) {
      this.changed = true;
      // Renamed, it keeps the place it had under a name it had.
      const was = node.was?.find((key) => this.order.has(key) && !present.has(key));
      if (was !== undefined) {
        this.order.set(node.key, this.order.get(was)!);
        this.order.delete(was);
        continue;
      }
      this.order.set(node.key, this.next);
      this.next += 1;
    }
    if (this.changed || present.size !== this.kept) {
      this.write(present);
      this.changed = false;
      this.kept = present.size;
    }
    const byOrder = (a: MapNode, b: MapNode) => this.order.get(a.key)! - this.order.get(b.key)!;
    // A machine on several hubs stands under the first of them.
    const hubOf = (node: MapNode) => order.find((id) => node.hubs.includes(id)) ?? null;
    const near = nearOf(links);
    // A strip's name, from the trunk to a little air at its end, and what
    // follows it.
    const named = (text: string) => (label && text ? TRUNK + 14 + label(text) + 14 : 0) + COUNT_ROOM;
    const partOf = (key: string, text: string, band: boolean, slots: (MapNode | null)[]): Part => {
      const machines = slots.filter((slot): slot is MapNode => slot !== null);
      const perColumn = Math.max(1, rows);
      const columns = Math.max(1, Math.ceil(slots.length / perColumn));
      const span = Math.max(columns * column, band ? named(text) + 12 : 0);
      return { key, label: text, band, slots, perColumn, columns, span, count: machines.length, online: onlineOf(machines), busy: busyOf(machines) };
    };
    // A box with nothing in it is as wide as one with one column.
    const plain = TRUNK + TWIG + PAD + RING * 2 + room;

    const trees = order.map((id) => {
      const own = nodes.filter((node) => hubOf(node) === id);
      const persons = people.filter((person) => person.hub === id);
      // The machines of each box, sorted into once: those in no group by
      // their box, those in a group by the box the group stands in.
      const looseIn = new Map<string, MapNode[]>();
      const groupedIn = new Map<string, MapNode[]>();
      const put = (into: Map<string, MapNode[]>, key: string, node: MapNode) => {
        const known = into.get(key);
        if (known) known.push(node);
        else into.set(key, [node]);
      };
      for (const node of own) {
        put(looseIn, node.group?.key ?? "mine", node);
        const within = node.group?.within?.key;
        if (within !== undefined) put(groupedIn, within, node);
      }
      const personOf = new Map<string, MapPerson>();
      for (const person of persons) if (!personOf.has(`of:${person.account}`)) personOf.set(`of:${person.account}`, person);
      // Each box by whose it is; a group stands inside its owner's.
      const ownerOf = (node: MapNode) => node.group?.within?.key ?? node.group?.key ?? "mine";
      const kinds = [
        ...new Set([
          "mine",
          ...own.map(ownerOf),
          ...persons.map((person) => `of:${person.account}`),
          ...(adding.has(id) ? ["adding"] : []),
        ]),
      ].sort((a, b) => compareBoxes(a, b, groupRank));
      const blocks = kinds.map((kind): Block => {
        const key = `group:${id}:${kind}`;
        // The dashed box that adds a person is a title strip alone.
        if (kind === "adding") return { kind, key: `adding:${id}`, label: "", parts: [], strip: true, least: TRUNK + 20 + ADDING, count: 0, online: 0, busy: 0 };
        const loose = chained([...(looseIn.get(kind) ?? [])].sort(byOrder), near);
        const person = personOf.get(kind);
        const inside = groupedIn.get(kind) ?? [];
        const bands = [...new Set(inside.map((node) => node.group!.key))]
          .sort((a, b) => compareBoxes(a, b, groupRank))
          .map((group) => {
            const members = chained(inside.filter((node) => node.group!.key === group).sort(byOrder), near);
            return partOf(`group:${id}:${group}`, members[0]!.group!.label, true, members);
          });
        // The account's own in no group end with the dashed ring.
        const looseSlots: (MapNode | null)[] = [...loose, ...(vacant && kind === "mine" ? [null] : [])];
        const parts = [...(looseSlots.length ? [partOf(`loose:${id}:${kind}`, "", false, looseSlots)] : []), ...bands];
        const all = [...loose, ...inside];
        const title = loose[0]?.group?.label ?? inside[0]?.group?.within?.label ?? person?.label ?? "";
        return { kind, key, label: title, person, parts, strip: !parts.length, least: Math.max(plain, named(title)), count: all.length, online: onlineOf(all), busy: busyOf(all) };
      });
      return { hub: id, blocks };
    });
    return { trees, homeless: nodes.filter((node) => !hubOf(node)).sort(byOrder), column, room };
  }

  /** How large the drawing is with rows no wider than `width`. */
  extent(prepared: Prepared, width: number): { width: number; height: number } {
    let right = 0;
    let bottom = 0;
    let top = 0;
    for (const tree of prepared.trees) {
      const rows = this.rows(prepared, tree.blocks, width, top);
      for (const row of rows) for (const { sized, x } of row.boxes) right = Math.max(right, x + sized.width);
      const last = rows[rows.length - 1];
      const end = last ? last.top + Math.max(0, ...last.boxes.map(({ sized }) => sized.height)) : top;
      bottom = Math.max(bottom, end);
      top = end + HUB_GAP;
    }
    return { width: right + 80, height: bottom + 110 };
  }

  /** How wide the drawing's rows are for it to be drawn largest in a frame
   *  `frame` across and down, with `pad` round it. */
  widthFor(prepared: Prepared, frame: { width: number; height: number }, pad: number): number {
    if (frame.width <= 2 * pad || frame.height <= 2 * pad) return Infinity;
    const scaleAt = (width: number) => {
      const size = this.extent(prepared, width);
      return Math.min((frame.width - 2 * pad) / size.width, (frame.height - 2 * pad) / size.height);
    };
    const narrowest = this.extent(prepared, 0).width;
    const widest = this.extent(prepared, Infinity).width;
    let best = { width: Infinity, scale: scaleAt(Infinity) };
    if (widest <= narrowest * 1.01) return best.width;
    // From the narrowest the rows may be to the whole drawing in one, each
    // a little wider than the last.
    for (let at = 0; at < 32; at += 1) {
      const width = narrowest * (widest / narrowest) ** (at / 32);
      const scale = scaleAt(width);
      if (scale > best.scale * 1.002) best = { width, scale };
    }
    return best.width;
  }

  /** The drawing placed with rows no wider than `width`. */
  place(prepared: Prepared, width = Infinity): Placement {
    const hubPoints = new Map<string, Point>();
    const machines = new Map<string, Point>();
    const groups: GroupNode[] = [];
    const branches: Branch[] = [];
    const boxOf = new Map<string, string>();
    const vacant = new Map<string, Point>();
    const into = { machines, groups, branches, boxOf, vacant };
    let top = 0;
    for (const tree of prepared.trees) {
      const knot: Point = [0, top];
      hubPoints.set(tree.hub, knot);
      const rows = this.rows(prepared, tree.blocks, width, top);
      for (const row of rows) {
        for (const { sized, x } of row.boxes) this.placeBox(prepared, sized, { x, y: row.top, width: sized.width, height: sized.height }, row.bus, tree.hub, into);
        // Each bus across from the trunk to its last box, broken to the box
        // that adds a person.
        const end = Math.max(...row.boxes.map(({ x }) => x + TRUNK));
        branches.push({ points: [[0, row.bus], [end, row.bus]], hub: tree.hub, trunk: true, out: row.boxes.every(({ sized }) => sized.block.kind === "adding") });
      }
      // The trunk from the hub down to the last bus.
      const last = rows[rows.length - 1];
      branches.push({ points: [[0, top + KNOT + 4], [0, last ? last.bus : top + BUS_DROP]], hub: tree.hub, trunk: true });
      top = (last ? last.top + Math.max(0, ...last.boxes.map(({ sized }) => sized.height)) : top) + HUB_GAP;
    }
    // No hub to hang on: a row, as before any network is joined.
    prepared.homeless.forEach((node, index) => machines.set(node.key, [index * prepared.column, TITLE]));

    // Room round everything: the hub's knot, with its name on its line, the
    // boxes below and the names right of the last rings.
    const xs = [...hubPoints.values(), ...machines.values()].map((p) => p[0]).concat(groups.flatMap((group) => [group.box.x, group.box.x + group.box.width]));
    const ys = [...hubPoints.values(), ...machines.values()].map((p) => p[1]).concat(groups.flatMap((group) => [group.box.y, group.box.y + group.box.height]));
    const bounds = xs.length
      ? { x: Math.min(...xs) - 40, y: Math.min(...ys) - 30, width: Math.max(...xs) - Math.min(...xs) + 80, height: Math.max(...ys) - Math.min(...ys) + 70 }
      : { x: -100, y: -100, width: 200, height: 200 };
    return { hubs: hubPoints, machines, groups, branches, boxOf, vacant, bounds, room: prepared.room };
  }

  /** A hub's boxes in rows no wider than `width`, its knot at `top`. */
  private rows(prepared: Prepared, blocks: readonly Block[], width: number, top: number): Row[] {
    const rows: Row[] = [];
    // The rows hang from buses under the hub.
    let rowTop = top + BUS_DROP + BOX_DROP;
    let row: Row = { bus: top + BUS_DROP, top: rowTop, boxes: [] };
    let x = SIDE;
    let tallest = 0;
    for (const block of blocks) {
      const sized = this.size(prepared, block, width - SIDE);
      if (row.boxes.length && x + sized.width > width) {
        rows.push(row);
        rowTop = rowTop + tallest + ROW_GAP + BOX_DROP;
        row = { bus: rowTop - BOX_DROP, top: rowTop, boxes: [] };
        x = SIDE;
        tallest = 0;
      }
      row.boxes.push({ sized, x });
      x += sized.width + BOX_GAP;
      tallest = Math.max(tallest, sized.height);
    }
    if (row.boxes.length) rows.push(row);
    return rows;
  }

  /** A box's parts in rows no wider than `room`, a part always whole, and
   *  the box's size. */
  private size(prepared: Prepared, block: Block, room: number): Sized {
    const { column, room: names } = prepared;
    const shelves: Shelf[] = [];
    for (const part of block.strip ? [] : block.parts) {
      const last = shelves[shelves.length - 1];
      if (last && last.width + part.span <= room) {
        last.parts.push(part);
        last.width += part.span;
      } else shelves.push({ parts: [part], width: TRUNK + part.span - column + TWIG + RING * 2 + names + PAD, height: 0, banded: false });
    }
    for (const shelf of shelves) {
      shelf.banded = shelf.parts.some((part) => part.band);
      const tallest = Math.max(0, ...shelf.parts.map((part) => Math.min(part.slots.length, part.perColumn)));
      const head = shelf.banded ? BAND_TOP + BAND + BAND_UNDER : PAD;
      shelf.height = head + (tallest ? (tallest - 1) * ROW + RING * 2 + PAD : BAND_EDGE);
    }
    const inner = shelves.reduce((sum, shelf) => sum + shelf.height, 0) + Math.max(0, shelves.length - 1) * PART_GAP;
    return { block, shelves, width: Math.max(block.least, ...shelves.map((shelf) => shelf.width)), height: TITLE + inner };
  }

  /** A box at `at`, the line to it from the bus at `bus`: its title strip,
   *  its parts in their rows, each part's columns hanging from the fork over
   *  its row, the first column's trunk from the bus and on down the box's
   *  left to each row after. */
  private placeBox(
    prepared: Prepared,
    sized: Sized,
    at: Bounds,
    bus: number,
    hub: string,
    into: { machines: Map<string, Point>; groups: GroupNode[]; branches: Branch[]; boxOf: Map<string, string>; vacant: Map<string, Point> },
  ): void {
    const { block, shelves } = sized;
    const { column } = prepared;
    const { x, y, width } = at;
    into.groups.push({ key: block.key, hub, kind: block.kind, label: block.label, count: block.count, online: block.online, busy: block.busy, box: at, strip: TITLE, point: [x + TRUNK + 50, y + TITLE / 2], person: block.person });
    const drop = x + TRUNK;
    // To a strip alone the line stops in the middle of it, broken to the
    // box that adds a person.
    if (block.strip) {
      into.branches.push({ points: [[drop, bus], [drop, y + TITLE / 2]], hub, out: block.kind === "adding" });
      return;
    }
    let shelfTop = y + TITLE;
    // How far down the box's left the line has come.
    let spine = bus;
    for (const [index, shelf] of shelves.entries()) {
      const fork = shelfTop + (shelf.banded ? BAND_TOP / 2 : PAD * 0.55);
      const bandTop = shelfTop + BAND_TOP;
      const first = shelf.banded ? bandTop + BAND + BAND_UNDER + RING : shelfTop + PAD + RING;
      const ringAt = (row: number) => first + row * ROW;
      const bottom = shelfTop + shelf.height;
      // From the second row on, the line comes down the box's left to the
      // row's fork.
      if (index > 0) into.branches.push({ points: [[drop, spine], [drop, fork]], hub });
      const trunks: number[] = [];
      let left = x;
      for (const [order, part] of shelf.parts.entries()) {
        const last = order === shelf.parts.length - 1;
        const reach = last ? x + width - left : part.span - 12;
        if (part.band) into.groups.push({ key: part.key, hub, kind: part.key.slice(`group:${hub}:`.length), label: part.label, count: part.count, online: part.online, busy: part.busy, box: { x: left, y: bandTop, width: reach, height: bottom - bandTop - BAND_EDGE }, strip: BAND, point: [left + TRUNK + 50, bandTop + BAND / 2], within: block.key });
        for (let one = 0; one < part.columns; one += 1) {
          const trunk = left + TRUNK + one * column;
          const tall = Math.min(part.perColumn, part.slots.length - one * part.perColumn);
          const end = tall > 0 ? ringAt(tall - 1) : bandTop + BAND / 2;
          trunks.push(trunk);
          // The first column of the box's first row hangs from the bus.
          const fromBus = index === 0 && order === 0 && one === 0;
          into.branches.push({ points: [[trunk, fromBus ? bus : fork], [trunk, end]], hub });
          if (order === 0 && one === 0) spine = end;
        }
        part.slots.forEach((slot, place) => {
          const trunk = left + TRUNK + Math.floor(place / part.perColumn) * column;
          const point: Point = [trunk + TWIG + RING, ringAt(place % part.perColumn)];
          into.branches.push({ points: [[trunk, point[1]], [trunk + TWIG - 3, point[1]]], hub, machine: slot?.key, out: !!slot && slot.online !== true });
          if (slot) {
            into.machines.set(slot.key, point);
            into.boxOf.set(slot.key, block.key);
          } else into.vacant.set(hub, point);
        });
        left += part.span;
      }
      // The fork across the row's columns.
      if (trunks.length > 1) into.branches.push({ points: [[drop, fork], [Math.max(...trunks), fork]], hub });
      shelfTop = bottom + PART_GAP;
    }
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
