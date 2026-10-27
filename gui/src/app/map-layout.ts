// Where the machines page draws each hub and machine (`world-gui.md`, 账号、
// 机器、共享与协助), read from the top. The hub stands at the top; under it,
// joined by a bus of square lines, a box for each group of machines by
// whose they are, as a comment box gathers nodes in a blueprint: the
// account's own first, then each other account's shared with it, then those
// whose agent assists from another. For the network's administrator, every
// person has a box, a title strip alone while none of their machines has
// come, and a dashed box after them adds another person. A box has a title
// strip with the group's name, and its machines stand in a grid inside, one
// to a row and a new column every ten, their names to the right. The
// machines their owner put in groups (`world-network.md`, 机器的分组) stand
// in the same box, each group in columns of its own to the right of the
// rest, under a band with the group's name, their rows level with the
// rest's. A box lies under the lines and does not break them: the line down
// from the bus runs on through the title strip as the trunk of the first
// column, the other columns, a group's through its band, branch from it
// under the strip, and a short twig reaches each machine. A box, and a group
// in it, folds to its strip or band alone, so folding moves nothing but what
// lies below it. The account's own machines in no group end with the dashed
// ring that adds a machine, so one that comes stands where it waited. On a
// narrow screen, a phone's, the boxes stand one under another instead, each
// its machines in one column and its groups under them, each under its band
// across the box, joined from the left by a spine down from the hub, so the
// drawing is read by moving down. Positions are computed,
// never moved by hand; the order within a group is remembered on this
// device, so machines keep their order while others come and go.

import type { Bounds, Point } from "../map-canvas";
import { compareBoxes } from "./machine-groups";
import type { MapHub, MapLink, MapNode, MapPerson } from "./map-nodes";

/** A machine's ring and a hub's knot, in the drawing's units. */
export const RING = 24;
export const KNOT = 8;
/** A box's title strip. */
export const TITLE = 34;
/** How large a box's name is written on its strip, at its own size. */
const TITLE_NAME = 14;
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
/** Past this many people on a hub, their boxes stand in columns after the
 *  rest, each column hanging from a spine as on a narrow screen: a row of
 *  hundreds is a line too long to see. At least this many stand in a
 *  column, with this much room between two, as many as keep the columns
 *  about as wide as a screen is to its height. */
const STACK_PAST = 8;
const STACK_LEAST = 6;
const STACK_GAP = 14;
const STACK_ASPECT = 1.6;
/** A group's band in its owner's box; on a wide screen, how far under the
 *  box's strip the bands hang and how far under them the first rings stand;
 *  the room left under a band with nothing under it. */
export const BAND = 28;
const BAND_TOP = 18;
const BAND_UNDER = 14;
const BAND_EDGE = 8;
/** The room on a group's band after its name: how many machines, how many
 *  online, and its "⋯". */
const COUNT_ROOM = 150;

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
  /** How wide a box's name is at its own size, where a still drawing
   *  measures it: its box is never narrower than its name, grown. */
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

/** Machines standing in columns under a strip or a band: them and the slot
 *  of the dashed ring, how many to a column, and how many columns and how
 *  tall with the room round them, none without machines. */
interface Grid {
  slots: (MapNode | null)[];
  perColumn: number;
  columns: number;
  height: number;
}

/** A group in its owner's box: its band, and its machines under it unless
 *  it is folded. */
interface Band {
  kind: string;
  key: string;
  label: string;
  grid: Grid;
  fold: boolean;
  count: number;
  online: number;
  /** How far it reaches across its owner's box: its columns, or, folded,
   *  the room its strip's words take. */
  span: number;
}

/** A box as it is sized before it is placed. */
interface Block {
  kind: string;
  key: string;
  label: string;
  person?: MapPerson | undefined;
  /** Its machines in no group, and its groups under them. */
  grid: Grid;
  bands: Band[];
  fold: boolean;
  /** Folded or empty: the title strip alone. */
  strip: boolean;
  width: number;
  height: number;
  count: number;
  online: number;
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

export class MapLayout {
  /** Hub order, first seen first; a hub keeps its place on the line. */
  private readonly hubOrder: string[] = [];
  /** Each machine's number in the order; lower stands earlier. Kept on
   *  the device when it changed, or the machines there did. */
  private readonly order = new Map<string, number>();
  private next = 0;
  private changed = true;
  private kept = -1;
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
    const { links = [], folded = new Set<string>(), narrow = false, rows = ROWS, vacant = true, grow = 1, room = NAME, people = [], adding = new Set<string>(), label: labelWidth } = choices;
    // The room a name takes beside its ring, across a column and down a
    // row, as large as it is drawn.
    const NAME_AT = room * grow;
    const COLUMN_AT = TWIG + RING * 2 + (room + COLUMN - TWIG - RING * 2 - NAME) * grow;
    const ROW_AT = Math.max(ROW, LABEL * grow + ROW - RING * 2);
    // A box's title strip holds its name as large as it is drawn.
    const TITLE_AT = Math.max(TITLE, Math.round(TITLE_NAME * grow + 8));
    const BAND_AT = Math.max(BAND, Math.round(TITLE_NAME * grow + 6));
    const ending = (kind: string) => vacant && kind === "mine";
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
    const near = nearOf(links);
    for (const id of order) {
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
      const gridOf = (slots: (MapNode | null)[]): Grid => {
        const perColumn = narrow ? Math.max(1, slots.length) : rows;
        const tall = Math.min(slots.length, perColumn);
        return { slots, perColumn, columns: Math.max(1, Math.ceil(slots.length / perColumn)), height: slots.length ? PAD * 2 + (tall - 1) * ROW_AT + RING * 2 : 0 };
      };
      const widthOf = (grid: Grid) => TRUNK + TWIG + PAD + (grid.columns - 1) * COLUMN_AT + RING * 2 + NAME_AT;
      const onlineOf = (nodes: MapNode[]) => nodes.filter((node) => node.online === true).length;
      // A strip's name, from the trunk to a little air at its end, as large
      // as it is drawn.
      const named = (text: string) => (labelWidth && text ? TRUNK + 14 + labelWidth(text) * grow + 14 : 0);
      const blocks = kinds.map((kind): Block => {
        const key = `group:${id}:${kind}`;
        // The dashed box that adds a person is a title strip alone.
        if (kind === "adding") {
          const grid = gridOf([]);
          return { kind, key: `adding:${id}`, label: "", grid, bands: [], fold: false, strip: true, width: TRUNK + 20 + ADDING * grow, height: TITLE_AT, count: 0, online: 0 };
        }
        const loose = chained([...(looseIn.get(kind) ?? [])].sort(byOrder), near);
        const person = personOf.get(kind);
        const inside = groupedIn.get(kind) ?? [];
        const bands = [...new Set(inside.map((node) => node.group!.key))]
          .sort((a, b) => compareBoxes(a, b, groupRank))
          .map((group): Band => {
            const members = chained(inside.filter((node) => node.group!.key === group).sort(byOrder), near);
            const key = `group:${id}:${group}`;
            const fold = folded.has(key);
            const label = members[0]!.group!.label;
            const grid = gridOf(fold ? [] : members);
            const span = Math.max(grid.columns * COLUMN_AT, fold ? named(label) + COUNT_ROOM * grow : 0);
            return { kind: group, key, label, grid, fold, count: members.length, online: onlineOf(members), span };
          });
        // The account's own in no group end with the dashed ring.
        const grid = gridOf([...loose, ...(ending(kind) ? [null] : [])]);
        const all = [...loose, ...inside];
        const fold = folded.has(key) && all.length > 0;
        // A person none of whose machines has come is the strip alone.
        const strip = fold || (!grid.slots.length && !bands.length);
        const size = (): { width: number; height: number } => {
          if (narrow) {
            // The groups under the rest, each under its band.
            const below = bands.reduce((sum, band) => sum + BAND_AT + band.grid.height, 0) + (bands.length && !grid.slots.length ? BAND_EDGE : 0) + (bands.length && !bands[bands.length - 1]!.grid.height ? BAND_EDGE : 0);
            return { width: Math.max(widthOf(grid), ...bands.map((band) => Math.max(widthOf(band.grid), named(band.label) + COUNT_ROOM * grow))), height: TITLE_AT + grid.height + below };
          }
          // The groups to the right of the rest, the rows level.
          const columns = (grid.slots.length ? grid.columns : 0) + bands.reduce((sum, band) => sum + band.grid.columns, 0);
          // What folded bands reach past their one column each.
          const wider = bands.reduce((sum, band) => sum + band.span - band.grid.columns * COLUMN_AT, 0);
          const tallest = Math.max(0, ...[grid, ...bands.map((band) => band.grid)].map((one) => Math.min(one.slots.length, one.perColumn)));
          const head = bands.length ? BAND_TOP + BAND_AT + BAND_UNDER - PAD : 0;
          return { width: TRUNK + TWIG + PAD + (Math.max(1, columns) - 1) * COLUMN_AT + wider + RING * 2 + NAME_AT, height: TITLE_AT + head + (tallest ? PAD * 2 + (tallest - 1) * ROW_AT + RING * 2 : BAND_EDGE - head + BAND_TOP + BAND_AT) };
        };
        const label = loose[0]?.group?.label ?? inside[0]?.group?.within?.label ?? person?.label ?? "";
        const sized = strip ? { width: widthOf(grid), height: TITLE_AT } : size();
        // Its name on its strip; on a wide screen its last group's name on
        // that group's band too, which runs to the box's end.
        const shown = fold ? [] : bands;
        const last = shown[shown.length - 1];
        const before = (grid.slots.length ? grid.columns * COLUMN_AT : 0) + shown.slice(0, -1).reduce((sum, band) => sum + band.span, 0);
        const banded = !last ? 0 : narrow ? Math.max(...shown.map((band) => named(band.label))) : before + named(last.label) + BAND_EDGE;
        const { width, height } = { width: Math.max(sized.width, named(label), strip ? 0 : banded), height: sized.height };
        return {
          kind,
          key,
          label,
          person,
          grid,
          bands: fold ? [] : bands,
          fold,
          strip,
          width,
          height,
          count: all.length,
          online: onlineOf(all),
        };
      });
      const twigs = (grid: Grid, drop: number, ringAt: (row: number) => number) =>
        grid.slots.forEach((slot, index) => {
          const trunk = drop + Math.floor(index / grid.perColumn) * COLUMN_AT;
          const point: Point = [trunk + TWIG + RING, ringAt(index % grid.perColumn)];
          branches.push({ points: [[trunk, point[1]], [trunk + TWIG - 3, point[1]]], hub: id, machine: slot?.key, out: !!slot && slot.online !== true });
          if (slot) machines.set(slot.key, point);
          else this.vacancies.set(id, point);
        });
      // On a wide screen: the machines in no group in the first columns,
      // then each group's to the right under its band, every column
      // hanging from the fork under the strip, the rows level.
      const side = (block: Block, x: number, y: number, from: number) => {
        const banded = block.bands.length > 0;
        const fork = y + TITLE_AT + (banded ? BAND_TOP / 2 : PAD * 0.55);
        const bandTop = y + TITLE_AT + BAND_TOP;
        const first = banded ? bandTop + BAND_AT + BAND_UNDER + RING : y + TITLE_AT + PAD + RING;
        const ringAt = (row: number) => first + row * ROW_AT;
        const sections = [...(block.grid.slots.length ? [{ grid: block.grid, band: null }] : []), ...block.bands.map((band) => ({ grid: band.grid, band }))];
        const trunks: number[] = [];
        let column = 0;
        let across = 0;
        for (const [at, { grid, band }] of sections.entries()) {
          const left = x + across;
          if (band) {
            const width = at === sections.length - 1 ? x + block.width - BAND_EDGE - left : band.span - 12;
            groups.push({ key: band.key, hub: id, kind: band.kind, label: band.label, count: band.count, online: band.online, folded: band.fold, box: { x: left, y: bandTop, width, height: y + block.height - bandTop - BAND_EDGE }, strip: BAND_AT, point: [left + TRUNK + 50, bandTop + BAND_AT / 2], within: block.key });
          }
          for (let one = 0; one < grid.columns; one += 1) {
            const trunk = left + TRUNK + one * COLUMN_AT;
            const tall = Math.min(grid.perColumn, grid.slots.length - one * grid.perColumn);
            trunks.push(trunk);
            branches.push({ points: [[trunk, column + one === 0 ? from : fork], [trunk, tall > 0 ? ringAt(tall - 1) : bandTop + BAND_AT / 2]], hub: id });
          }
          twigs(grid, left + TRUNK, ringAt);
          column += grid.columns;
          across += band ? band.span : grid.columns * COLUMN_AT;
        }
        if (trunks.length > 1) branches.push({ points: [[trunks[0]!, fork], [trunks[trunks.length - 1]!, fork]], hub: id });
      };
      // A box at `x`, `y`, its first trunk from `from` down; on a narrow
      // screen its machines in no group under its strip, then each group's
      // band and machines.
      const place = (block: Block, x: number, y: number, from: number) => {
        groups.push({ key: block.key, hub: id, kind: block.kind, label: block.label, count: block.count, online: block.online, folded: block.fold, box: { x, y, width: block.width, height: block.height }, strip: TITLE_AT, point: [x + TRUNK + 50, y + TITLE_AT / 2], person: block.person });
        const drop = x + TRUNK;
        // To a strip alone the line stops in the middle of it, broken to
        // the box that adds a person.
        if (block.strip) {
          branches.push({ points: [[drop, from], [drop, y + TITLE_AT / 2]], hub: id, out: block.kind === "adding" });
          return;
        }
        if (!narrow) return side(block, x, y, from);
        const regions: { grid: Grid; top: number }[] = [{ grid: block.grid, top: y + TITLE_AT }];
        let at = y + TITLE_AT + (block.grid.slots.length ? block.grid.height : BAND_EDGE);
        let end = y + TITLE_AT / 2;
        for (const band of block.bands) {
          const height = BAND_AT + band.grid.height;
          groups.push({ key: band.key, hub: id, kind: band.kind, label: band.label, count: band.count, online: band.online, folded: band.fold, box: { x, y: at, width: block.width, height }, strip: BAND_AT, point: [x + TRUNK + 50, at + BAND_AT / 2], within: block.key });
          end = at + BAND_AT / 2;
          regions.push({ grid: band.grid, top: at + BAND_AT });
          at += height;
        }
        // Each region's machines in their columns; the first column's trunk
        // runs on through them all, to the last band or machine.
        // One column each, the first's trunk running on through them all.
        for (const { grid, top } of regions) {
          const ringAt = (row: number) => top + PAD + RING + row * ROW_AT;
          if (grid.slots.length) end = Math.max(end, ringAt(grid.slots.length - 1));
          twigs(grid, drop, ringAt);
        }
        branches.push({ points: [[drop, from], [drop, end]], hub: id });
      };
      if (narrow) {
        // The hub at the top left, a spine down its left, and each box to
        // its right, joined at the middle of its title strip.
        const knot: Point = [0, below];
        hubPoints.set(id, knot);
        let y = below + BUS_DROP;
        let join = y;
        for (const block of blocks) {
          join = y + TITLE_AT / 2;
          branches.push({ points: [[0, join], [SIDE + TRUNK, join]], hub: id, out: block.kind === "adding" });
          place(block, SIDE, y, join);
          y += block.height + BOX_GAP;
        }
        branches.push({ points: [[0, below + KNOT + 4], [0, join]], hub: id });
        below = y + HUB_GAP;
        continue;
      }
      // Many people: theirs, and the box that adds one, in columns after
      // the rest; one opened stands open where it is, those under it moved
      // down.
      const peopled = blocks.filter((block) => block.kind.startsWith("of:")).length > STACK_PAST;
      const stacked = peopled ? blocks.filter((block) => block.kind.startsWith("of:") || block.kind === "adding") : [];
      const row = blocks.filter((block) => !stacked.includes(block));
      const columns: Block[][] = [];
      if (stacked.length) {
        const wide = SIDE + Math.max(...stacked.map((block) => block.width));
        const each = Math.max(STACK_LEAST, Math.ceil(Math.sqrt((stacked.length * wide) / (STACK_ASPECT * (TITLE_AT + STACK_GAP)))));
        for (let at = 0; at < stacked.length; at += each) columns.push(stacked.slice(at, at + each));
      }
      const columnWidth = (column: Block[]) => SIDE + Math.max(...column.map((block) => block.width));
      const widths = [...row.map((block) => block.width), ...columns.map(columnWidth)];
      const width = widths.reduce((sum, one) => sum + one, 0) + BOX_GAP * (widths.length - 1);
      const knot: Point = [left + width / 2, 0];
      hubPoints.set(id, knot);
      const busY = BUS_DROP;
      const top = BUS_DROP + BOX_DROP;
      let x = left;
      const drops: number[] = [];
      for (const block of row) {
        // The line from the bus comes down at the first trunk.
        drops.push(x + TRUNK);
        place(block, x, top, busY);
        x += block.width + BOX_GAP;
      }
      for (const column of columns) {
        // A spine down from the bus, each box to its right, joined at the
        // middle of its strip.
        drops.push(x);
        let y = top;
        let join = top;
        for (const block of column) {
          join = y + TITLE_AT / 2;
          branches.push({ points: [[x, join], [x + SIDE + TRUNK, join]], hub: id, out: block.kind === "adding" });
          place(block, x + SIDE, y, join);
          y += block.height + STACK_GAP;
        }
        branches.push({ points: [[x, busY], [x, join]], hub: id });
        x += columnWidth(column) + BOX_GAP;
      }
      // The hub down to the bus, and the bus across the drops to the boxes.
      branches.push({ points: [[knot[0], KNOT + 4], [knot[0], busY]], hub: id });
      if (drops.length) branches.push({ points: [[Math.min(knot[0], ...drops), busY], [Math.max(knot[0], ...drops), busY]], hub: id });
      left += width + HUB_GAP;
    }
    // No hub to hang on: a row, as before any network is joined.
    nodes.filter((node) => !hubOf(node)).sort(byOrder).forEach((node, index) => machines.set(node.key, [index * COLUMN_AT, TITLE_AT]));

    if (this.changed || present.size !== this.kept) {
      this.write(present);
      this.changed = false;
      this.kept = present.size;
    }
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
