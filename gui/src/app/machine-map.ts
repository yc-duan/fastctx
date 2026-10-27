// The machines page's drawing as a canvas (`world-gui.md`, 账号、机器、共享与
// 协助): the account's World drawn by hand on paper (`map-layout.ts`), moved
// and zoomed (`../map-canvas`). The hub at the top left, square lines down
// to a box for each group of machines by whose they are, its name on a title
// strip, and the machines standing in a grid inside, names to the right.
//
// How much of it is drawn is how near it is seen; the places never change
// with the distance, only what is drawn there. Far, where a ring would be a
// speck, each box is a block of its wash and each machine a drop of ink where
// its ring stands: in ink while it is online, faint while it is away, in
// clay while something works on it; the lines are the hub's alone. Nearer,
// the drops open into the machines' rings, drawn along the very line the
// pen draws them nearer still, with the lines down to them, but no names
// yet: the box's and its groups' are on the blocks, and a machine's is said
// over it when the pointer rests there. Near, it is the paper: the rings in
// the pen's ink, the lines, the names and, nearer still, what each machine
// is. The names are never written smaller than the page writes, and grow
// with the paper only as it is brought nearer than its own size; one with
// no room beside its ring is cut short.
//
// The account's own end with a dashed ring that adds a machine. For the
// network's administrator every person has a box, a strip alone until a
// machine of theirs comes, with a "⋯" at its end and a small lock while they
// are locked, and a dashed box after them adds a person. A machine of
// another whose agent alone assists is a broken ring of clay; one out of
// reach is drawn with a dry pen. A machine others may connect to directly
// wears a dashed halo, in pencil until the hub has checked it: every machine
// that reaches it links with it, so the links are not drawn one by one;
// which are up is said in its details. What floats beside a ring chosen is
// the page's.

import { glyph } from "../conversation/glyphs";
import { reducedMotion } from "../motion";
import { MapCanvas, PenLayer, type Bounds, type CanvasEvent, type CanvasFrame, type CanvasItem, type PenStroke, type Point, type View } from "../map-canvas";
import { blot, circle, circlePoints, outline, ruled, seeded, ticks } from "../pen/shapes";
import type { MachinesWords } from "./account-words";
import { boxParts, groupMoreKey, nameInGroup } from "./machine-groups";
import { BAND, KNOT, MapLayout, RING, ROW, ROWS, TITLE, TRUNK, type GroupNode, type Placement, type Prepared } from "./map-layout";
import type { MapHub, MapLink, MapNode, MapPerson } from "./map-nodes";
import type { PeopleWords } from "./people-words";

export interface MachineMapOptions {
  words: MachinesWords;
  /** What is said of the people, where the drawing shows them. */
  people?: PeopleWords;
  /** Where this device remembers the places, or nowhere. */
  storageKey: string | null;
  event(event: CanvasEvent): void;
  /** Each painted frame, for what the page floats over the drawing. */
  frame?(frame: CanvasFrame): void;
  /** A picture on another page: fitted to its frame and never moved, its
   *  columns as tall as fill the frame best. */
  still?: boolean;
  /** What follows the hints under the drawing. */
  afterHints?: HTMLElement;
}

export interface MachineMapScene {
  hubs: MapHub[];
  nodes: MapNode[];
  /** Under each hub's name. */
  hubLabels: Map<string, string>;
  /** The hubs whose dashed ring adds a machine, and those a machine is on
   *  its way to. */
  adds: string[];
  waiting: ReadonlySet<string>;
  selected: string | null;
  /** Machines that just came: circled for a moment as they arrive. */
  fresh: ReadonlySet<string>;
  /** Direct links between machines, which put linked machines side by side
   *  in their box. */
  links: MapLink[];
  /** Machines direct connections were just allowed to, whose halo is in
   *  pencil until the hub has checked them. */
  opening: ReadonlySet<string>;
  /** Machines framed with the mouse, or inside the frame being drawn. */
  framed: readonly string[];
  /** While a new place for the server is chosen: the machines that can take
   *  it, by key, and whether each goes to sleep; the rest are faint. */
  lit?: ReadonlyMap<string, { sleeps: boolean }> | null;
  /** For the network's administrator: every person, and the hubs whose
   *  people end with the dashed box that adds one. */
  people?: readonly MapPerson[];
  adding?: readonly string[];
  /** The boxes of groups the user may rename or end, by key: each has a
   *  "⋯" at the end of its strip, as a person's box has. */
  grouped?: ReadonlySet<string>;
  /** While machines are looked for, those that match, by key: each ringed
   *  in clay, the other names faint; seen from afar, the boxes and groups
   *  holding none of them faint. */
  found?: ReadonlySet<string> | null;
}

/** Where this device remembers the order of the machines on the drawing,
 *  the same on every page that draws it. */
export const MAP_ORDER = "machines-map";

/** The key of a hub's dashed ring that adds a machine. */
export const addKey = (hub: string) => `add:${hub}`;

/** The key of a person's "⋯" on their box. */
export const personKey = (hub: string, account: string) => `who:${hub}:${account}`;

/** How wide a person's "⋯" is pressed at the end of their box's strip. */
const MORE = 40;

/** Nearer than this the drawing is the paper, its rings and names; further
 *  it is its boxes, each a block: a box's strip there would be lower than
 *  its name, and a name would have room for a letter or two. */
const PAPER = 0.55;

/** On a phone the paper comes from a little further: its drawing is laid
 *  so that each name has its room there, and its strips hold its words. */
const PHONE_PAPER = 0.45;

/** Nearer than this, and further than the paper, each machine is its ring
 *  drawn small, about ten pixels across or more; further, a drop of ink. */
const RINGED = 0.2;

/** Nearer than this, each machine says what it is under its name: its row
 *  has room for two lines. */
const DETAILED = 0.7;

/** The paper drawn round what is in view, this share of the view more on
 *  each side, so it moves without being drawn again until it is left; and
 *  the names on the blocks seen from afar. */
const AHEAD = 0.6;
const HEADS_AHEAD = 0.3;

/** The drawing is never fitted larger than its own size. */
const FIT_MOST = 1;

/** The gutter a phone's drawing is fitted within, as the page's. */
const NARROW_PAD = 16;

const SVG = "http://www.w3.org/2000/svg";

/** How long the drawing takes to glide, or fly, to where it is brought. */
const GLIDE_MS = 240;
const FLY_MS = 420;

/** Room round the drawing in its layers, for what they draw beyond it. */
const LAYER_ROOM = 160;

/** Seen from afar, a machine's dot is about a third of its ring across, in
 *  pixels no less than the first nor more than the second. */
const DOT_LEAST = 2.5;
const DOT_MOST = 8;

/** A name on a block seen from afar: its ground starts this far in from
 *  the block's left and, at the block's top, this far down, and reaches
 *  this far round its words; it keeps this far clear of another on its
 *  line; none is written in less room than the last. In pixels, as the
 *  styles have them (`machines.css`). */
const HEAD_LEFT = 4;
const HEAD_TOP = 6;
const HEAD_PAD = 6;
const HEAD_CLEAR = 8;
const HEAD_LEAST = 56;

/** A box with no machine yet is left out seen from afar while lower than
 *  this, in pixels: a sliver. */
const SLIVER = 12;

/** The air a machine's name leaves before the next column's line. */
const NAME_AIR = 12;

/** At most this many machines and a still drawing tries each height of its
 *  columns to fill its frame; more, and they stand ten to a column. */
const STILL_SEARCH = 300;

export class MachineMap {
  readonly element: HTMLElement;
  private readonly surface: MapCanvas;
  private readonly pens: PenLayer;
  private readonly layout: MapLayout;
  /** Under the ink: the groups' boxes. Over it: what is marked live. Both
   *  in the drawing's units, so they zoom with it. Over those, the boxes
   *  seen from afar, then the names, placed at the drawing's scale but
   *  written at the page's size. */
  private readonly boxes: SVGSVGElement;
  private readonly marks: SVGSVGElement;
  private readonly blocks: HTMLElement;
  private readonly words: HTMLElement;
  /** Seen from afar: the machines' dots and the hub's lines, drawn under the
   *  blocks; and what the pointer rests on, named over all. */
  private readonly dots: HTMLCanvasElement;
  private readonly tip: HTMLElement;
  private readonly themes: MutationObserver;
  private scene: MachineMapScene | null = null;
  private prepared: Prepared | null = null;
  private placement: Placement | null = null;
  /** What the boxes were prepared from, and the frame and width they were
   *  placed for, to do each again only when that changed; what a phone's
   *  drawing was filled from and for; and the frame the drawing was laid
   *  for, its shape on a wide page, its size where it fills its frame. */
  private preparedFrom: { from: readonly unknown[]; shape: string } | null = null;
  private placedFor = "";
  private filledFrom: { from: readonly unknown[]; frame: string } | null = null;
  private laidFor = "";
  /** How near the drawing is seen as of the last frame: 0, its machines
   *  drops of ink; 1, their rings drawn small; 2, the paper. Seen from afar
   *  is the first two, as its boxes. */
  private band = 2;
  private far = false;
  /** The scale the paper is drawn from, a phone's or a computer's. */
  private paper = PAPER;
  /** Whether the drawing is laid for a narrow screen, as of the last
   *  update: filling its frame, as a still drawing does. */
  private narrow = false;
  private composed = "";
  /** How many times the drawing has been composed, and what is marked on it
   *  changed: what is drawn over it is drawn again after each. */
  private compositions = 0;
  private markings = 0;
  private marked = "";
  /** What the names on the blocks were written for, and round which part
   *  of the drawing. */
  private blocksDrawn = "";
  private blocksRegion: Bounds | null = null;
  private blocksScale = 0;
  /** The part of the drawing the paper is drawn for, in its units, and the
   *  scale it was drawn at. */
  private region: Bounds | null = null;
  private regionScale = 1;
  /** The pens' colours as the theme names them, the paper's and the
   *  words', and how round a box's corners are, and a group's. */
  private inks = { graphite: "", clay: "", paper: "", text: "", box: 10, group: 6 };
  /** The machines' dots seen from afar, gathered by how they are inked, and
   *  their rings seen nearer; what they were gathered for; whether the
   *  canvas is bare. */
  private farDots: FarDots[] = [];
  private farRings: FarRing[] = [];
  private dotsFor = "";
  private dotsBare = true;
  /** Measures names for the room they take. */
  private measuring: CanvasRenderingContext2D | null = null;
  /** The faces are loading, and the drawing is laid again when they are in. */
  private awaitingFaces = false;
  /** The box of the drawing the layers over and under the ink are laid
   *  over, and the scale they are drawn at. */
  private layerBox: Bounds | null = null;
  private layerScale = NaN;
  /** What the pointer rests on, shown on the boxes seen from afar. */
  private pointed: string | null = null;
  private flight = 0;

  constructor(private readonly options: MachineMapOptions) {
    const { words } = options;
    this.layout = new MapLayout(options.storageKey);
    this.surface = new MapCanvas({
      label: words.map,
      controls: { minus: words.zoomOut, plus: words.zoomIn, fit: words.fit, scale: words.scale },
      hints: options.still ? undefined : { zoom: words.hintZoom, pan: words.hintPan, ...(options.afterHints ? { after: options.afterHints } : {}) },
      still: options.still ?? false,
      minScale: 1 / 40,
      maxScale: 4,
      fitMost: FIT_MOST,
      names: false,
      levels: [RINGED, PAPER],
      validTarget: () => false,
      event: (event) => {
        if (event.kind === "point") this.point(event.key);
        // Seen from afar a frame takes the machines whose dots are in it.
        if (event.kind === "marquee" && this.far) event = { ...event, keys: this.machinesIn(event.area) };
        options.event(event);
      },
      frame: (frame) => this.onFrame(frame),
    });
    this.element = this.surface.element;
    this.element.classList.add("machine-map");
    const layer = (name: string) => {
      const svg = document.createElementNS(SVG, "svg");
      svg.classList.add(name);
      svg.setAttribute("width", "1");
      svg.setAttribute("height", "1");
      svg.setAttribute("aria-hidden", "true");
      return svg;
    };
    // Only the pens' strokes are ink, blended with the paper; the boxes'
    // washes lie under them, the live marks over them, then the boxes seen
    // from afar, and the names over all.
    this.boxes = layer("map-boxes");
    this.surface.under.append(this.boxes);
    this.pens = new PenLayer(this.surface.ink, "texture");
    this.marks = layer("map-live");
    const plain = (name: string) => {
      const div = document.createElement("div");
      div.className = name;
      div.setAttribute("aria-hidden", "true");
      return div;
    };
    this.blocks = plain("map-blocks");
    this.words = plain("map-words");
    this.surface.over.append(this.marks, this.blocks, this.words);
    // Seen from afar the machines are dots and the lines plain, drawn on a
    // canvas of the view's size between the ink and the blocks.
    this.dots = document.createElement("canvas");
    this.dots.className = "map-dots";
    this.dots.setAttribute("aria-hidden", "true");
    this.surface.over.before(this.dots);
    this.tip = plain("map-tip");
    this.tip.hidden = true;
    this.element.append(this.tip);
    // The ink's colours are resolved for the textures: drawn again when the
    // theme changes.
    this.themes = new MutationObserver(() => {
      this.composed = "";
      this.sizes.clear();
      this.compose();
      this.place();
      this.mark();
    });
    this.themes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class"] });
  }

  /** The machine, box or hub's place on the screen, in the canvas's
   *  pixels. */
  screenOf(key: string): Point | null {
    const point = this.pointOf(key);
    return point ? this.surface.camera.toScreen(point) : null;
  }

  /** How wide the name written beside a machine or a hub is on the screen
   *  now, or nothing where none is written. */
  nameWidth(key: string): number {
    const named = this.words.querySelector<HTMLElement>(`[data-name="${CSS.escape(key)}"]`);
    const name = named?.classList.contains("mw-hub") ? named : named?.firstElementChild;
    return name ? name.getBoundingClientRect().width : 0;
  }

  /** Whether the drawing is seen from afar now: its boxes, not its
   *  machines. */
  get distant(): boolean {
    return this.far;
  }

  fit(): void {
    this.surface.fit();
  }

  /** Measures the drawing again and draws a frame at its size. */
  refresh(): void {
    this.surface.refresh();
  }

  /** Moves the drawing by `dx`, `dy` screen pixels, smoothly unless the
   *  interface moves less (`motion.ts`). */
  glide(dx: number, dy: number): void {
    if (Math.hypot(dx, dy) < 1) return;
    if (reducedMotion()) {
      this.surface.pan(dx, dy);
      return;
    }
    const start = performance.now();
    let moved = 0;
    const step = (now: number) => {
      const t = Math.min(1, (now - start) / GLIDE_MS);
      const eased = 1 - (1 - t) ** 3;
      this.surface.pan(dx * (eased - moved), dy * (eased - moved));
      moved = eased;
      if (t < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }

  /** Brings the box, group or machine `key` to fill the view, as large as
   *  it may be drawn up to its own size and near enough to read what each
   *  machine is, flying there unless the interface moves less. */
  flyTo(key: string): void {
    const target = this.boundsOf(key);
    if (!target) return;
    const view = this.surface.camera.view;
    if (!view.width || !view.height) return;
    const pad = view.width < 480 ? NARROW_PAD : 48;
    const scale = Math.max(DETAILED, Math.min(FIT_MOST, (view.width - 2 * pad) / target.width, (view.height - 2 * pad) / target.height));
    const centre: Point = [target.x + target.width / 2, target.y + target.height / 2];
    // A box taller than the view is shown from its top.
    const y = target.height * scale > view.height - 2 * pad ? pad - target.y * scale : view.height / 2 - centre[1] * scale;
    const x = target.width * scale > view.width - 2 * pad ? pad - target.x * scale : view.width / 2 - centre[0] * scale;
    this.flyView(scale, x, y);
  }

  /** Brings the drawing to `scale` with its origin at `x`, `y` on the screen. */
  private flyView(scale: number, x: number, y: number): void {
    cancelAnimationFrame(this.flight);
    const from = this.surface.camera.view;
    if (reducedMotion()) {
      this.surface.view(scale, x, y);
      return;
    }
    // The point under the view's middle moves in a straight line while the
    // scale changes evenly in its logarithm, as a flight in and out does.
    const middle: Point = [from.width / 2, from.height / 2];
    const start: Point = [(middle[0] - from.x) / from.scale, (middle[1] - from.y) / from.scale];
    const end: Point = [(middle[0] - x) / scale, (middle[1] - y) / scale];
    const began = performance.now();
    const step = (now: number) => {
      const t = Math.min(1, (now - began) / FLY_MS);
      const eased = t < 0.5 ? 4 * t ** 3 : 1 - (-2 * t + 2) ** 3 / 2;
      const at = from.scale * (scale / from.scale) ** eased;
      const world: Point = [start[0] + (end[0] - start[0]) * eased, start[1] + (end[1] - start[1]) * eased];
      this.surface.view(at, middle[0] - world[0] * at, middle[1] - world[1] * at);
      if (t < 1) this.flight = requestAnimationFrame(step);
    };
    this.flight = requestAnimationFrame(step);
  }

  dispose(): void {
    cancelAnimationFrame(this.flight);
    this.themes.disconnect();
    this.pens.dispose();
    this.surface.dispose();
  }

  update(scene: MachineMapScene): void {
    this.scene = scene;
    // The faces as the theme names them now, read once this drawing.
    this.faces.clear();
    this.narrow = !this.options.still && this.isNarrow();
    const paper = this.isNarrow() ? PHONE_PAPER : PAPER;
    if (paper !== this.paper) {
      this.paper = paper;
      this.surface.levels([RINGED, paper]);
    }
    const frame = { width: this.element.clientWidth, height: this.element.clientHeight };
    if (this.options.still || this.narrow) {
      // A still drawing, and a phone's, is laid to be drawn largest in its
      // frame: the whole of it on one screen, read nearer by zooming. A
      // phone's is laid again only when what it is laid from, or its
      // frame, changed.
      const from = [scene.hubs, scene.nodes, scene.links, scene.people ?? null, scene.adds, scene.adding ?? null] as const;
      const sized = `${frame.width}x${frame.height}`;
      const kept = this.filledFrom;
      if (this.options.still || !this.placement || !kept || kept.frame !== sized || kept.from.some((one, at) => one !== from[at])) {
        this.placement = this.fill(scene);
        this.filledFrom = { from, frame: sized };
        this.placedFor = "";
        this.region = null;
      }
      this.laidFor = sized;
    } else {
      // Prepared again only when what it is laid from changed: the page
      // draws itself every couple of seconds with the same machines.
      const from = [scene.hubs, scene.nodes, scene.links, scene.people ?? null] as const;
      const shape = JSON.stringify([[...(scene.adding ?? [])].sort(), scene.adds.length > 0, document.fonts.status]);
      const kept = this.preparedFrom;
      if (!this.prepared || !kept || kept.shape !== shape || kept.from.some((one, at) => one !== from[at])) {
        this.prepared = this.layout.prepare(scene.hubs, scene.nodes, { links: scene.links, vacant: scene.adds.length > 0, people: scene.people ?? [], adding: new Set(scene.adding ?? []), label: (text) => this.labelWidth(text) });
        this.preparedFrom = { from, shape };
        this.placedFor = "";
      }
      // Its rows as wide as draw it largest in its frame, found again when
      // the frame changes its shape.
      const shapeOf = frame.width && frame.height ? (frame.width / frame.height).toFixed(1) : "none";
      if (this.placedFor !== shapeOf || !this.placement || this.filledFrom) {
        const width = this.layout.widthFor(this.prepared, frame, this.padding());
        this.placement = this.layout.place(this.prepared, width);
        this.placedFor = shapeOf;
        this.filledFrom = null;
        this.region = null;
      }
      this.laidFor = shapeOf;
    }
    this.compose();
    this.place();
    this.mark();
  }

  private pointOf(key: string): Point | null {
    const placement = this.placement;
    if (!placement) return null;
    if (key.startsWith("hub:")) return placement.hubs.get(key.slice(4)) ?? null;
    if (key.startsWith("group:") || key.startsWith("adding:")) return placement.groups.find((group) => group.key === key)?.point ?? null;
    if (key.startsWith("who:") || key.startsWith("gm:")) {
      const group = placement.groups.find((one) => this.moreOf(one)?.key === key);
      return group ? [group.box.x + group.box.width - MORE / 2, group.box.y + group.strip / 2] : null;
    }
    // A hub's dashed ring waits where its next machine will stand.
    if (key.startsWith("add:")) return placement.vacant.get(key.slice(4)) ?? null;
    return placement.machines.get(key) ?? null;
  }

  /** What `key` takes on the drawing: a box or a group whole, a machine
   *  with its name. */
  private boundsOf(key: string): Bounds | null {
    const placement = this.placement;
    if (!placement) return null;
    const group = placement.groups.find((one) => one.key === key);
    if (group) return group.box;
    const point = this.pointOf(key);
    return point ? { x: point[0] - RING - 120, y: point[1] - RING - 90, width: 2 * RING + 360, height: 2 * RING + 180 } : null;
  }

  /** The machines standing in `area`, a part of the drawing. */
  private machinesIn(area: Bounds): string[] {
    const keys: string[] = [];
    for (const [key, [x, y]] of this.placement?.machines ?? []) if (x >= area.x && x <= area.x + area.width && y >= area.y && y <= area.y + area.height) keys.push(key);
    return keys;
  }

  /** A still drawing, or a phone's, laid with its columns as tall, and its
   *  rows as wide, as let it be drawn largest in its frame; ten to a column
   *  until the frame has a size. A name is written no smaller than the page
   *  writes, however small the drawing: drawn smaller than its own size, it
   *  gives each name the room that takes, as near as it can be seen with
   *  its names, or else as near as they come. */
  private fill(scene: MachineMapScene): Placement {
    const width = this.element.clientWidth;
    const height = this.element.clientHeight;
    const { name: named, under } = this.nameRoom(scene) ?? {};
    const people = scene.people ?? [];
    const adding = new Set(scene.adding ?? []);
    const prepareFor = (rows: number, room: number | undefined) =>
      this.layout.prepare(scene.hubs, scene.nodes, { links: scene.links, rows, vacant: scene.adds.length > 0, people, adding, label: (text) => this.labelWidth(text), ...(room === undefined ? {} : { room }) });
    if (!width || !height) return this.layout.place(prepareFor(ROWS, named));
    // What it is laid out from, but who is online: a machine coming or
    // going is laid out as the best size was last found for these, once.
    const shape = JSON.stringify([width, height, named, document.fonts.status, scene.hubs.map((hub) => hub.id), scene.nodes.map((node) => [node.key, node.name, node.sub, node.group?.key ?? null, node.hubs]), scene.links.map((link) => [link.a, link.b]), scene.adds, people.map((person) => [person.hub, person.account, person.label]), [...adding]]);
    const kept = this.found;
    if (kept?.shape === shape) return this.layout.place(prepareFor(kept.rows, kept.room), kept.width);
    // The frame the canvas fits it in: its padding round it, and the room
    // its controls take under it.
    const pad = this.padding();
    const below = this.options.still ? pad : this.surface.fitBelow();
    const frame = { width, height: height - Math.max(0, below - pad) };
    type Laid = { rows: number; width: number; scale: number; placement: Placement; room: number | undefined };
    // The best height of its columns for names given `room`.
    const best = (room: number | undefined): Laid => {
      let found: Laid | null = null;
      for (const rows of scene.nodes.length > STILL_SEARCH ? [ROWS] : [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]) {
        const prepared = prepareFor(rows, room);
        const across = this.layout.widthFor(prepared, frame, pad);
        const placement = this.layout.place(prepared, across);
        const scale = Math.min((frame.width - 2 * pad) / placement.bounds.width, (frame.height - 2 * pad) / placement.bounds.height);
        if (!found || scale > found.scale + 1e-6) found = { rows, width: across, scale, placement, room };
      }
      return found!;
    };
    const tight = best(named === undefined ? undefined : Math.max(named, under ?? 0));
    let laid = tight;
    if (named !== undefined && tight.scale < 1) {
      // Written at the page's size, a name seen at `scale` takes its width
      // over that in the drawing's units. The nearest it can be seen with
      // every name whole is found by halving; where it cannot be seen with
      // its names at all, they come whole once it is brought near enough.
      const at = (scale: number) => best((scale >= DETAILED ? Math.max(named, under ?? 0) : named) / scale);
      const loose = at(this.paper);
      laid = loose;
      if (loose.scale >= this.paper) {
        let low = this.paper;
        let high = Math.min(1, tight.scale);
        for (let step = 0; step < 5; step += 1) {
          const middle = (low + high) / 2;
          const tried = at(middle);
          if (tried.scale >= middle) {
            low = middle;
            laid = tried;
          } else high = middle;
        }
      }
    }
    this.found = { shape, rows: laid.rows, width: laid.width, room: laid.room };
    return laid.placement;
  }

  /** The best way last found to lay a still drawing, and what for. */
  private found: { shape: string; rows: number; width: number; room: number | undefined } | null = null;

  /** Room left round the drawing in its frame, as the canvas leaves it. */
  private padding(): number {
    const width = this.element.clientWidth;
    const height = this.element.clientHeight;
    return width < 480 ? NARROW_PAD : Math.max(24, Math.min(76, Math.min(width, height) * 0.1));
  }

  /** How wide a box's name is written on its strip, at its own size. */
  private labelWidth(text: string): number {
    return this.measured("--font-emphasis", this.textSize("--size-text"), text);
  }

  /** The size in pixels the design gives `name`, read once for each
   *  theme. */
  private textSize(name: string): number {
    let size = this.sizes.get(name);
    if (size === undefined) {
      const probe = document.createElement("span");
      probe.style.cssText = `position:absolute;visibility:hidden;font-size:var(${name})`;
      this.element.append(probe);
      size = Number.parseFloat(getComputedStyle(probe).fontSize) || 15;
      probe.remove();
      this.sizes.set(name, size);
    }
    return size;
  }

  private readonly sizes = new Map<string, number>();

  /** How wide `text` is at `size` pixels in the face `face` names, measured
   *  once for each face, size and text: a still drawing is laid out a few
   *  times over to find its best size, with every name. */
  private measured(face: string, size: number, text: string): number {
    this.measuring ??= document.createElement("canvas").getContext("2d");
    const context = this.measuring;
    if (!context) return 0;
    const family = this.face(face);
    // A face not yet loaded measures as another: its widths kept apart.
    const id = `${family}:${document.fonts.status}`;
    let widths = this.widths.get(id);
    if (!widths) {
      if (this.widths.size > 8) this.widths.clear();
      widths = new Map();
      this.widths.set(id, widths);
    }
    const key = `${size}\n${text}`;
    let width = widths.get(key);
    if (width === undefined) {
      context.font = `${size}px ${family}`;
      width = context.measureText(text).width;
      widths.set(key, width);
    }
    return width;
  }

  private readonly widths = new Map<string, Map<string, number>>();

  /** The family a face of the design names, read once a drawing. */
  private face(name: string): string {
    let family = this.faces.get(name);
    if (family === undefined) {
      family = getComputedStyle(document.documentElement).getPropertyValue(name).trim() || "serif";
      this.faces.set(name, family);
    }
    return family;
  }

  private readonly faces = new Map<string, string>();

  /** How wide the widest of the machines' names is as the paper writes
   *  them, and of the lines under them, each with a little air: the room a
   *  name takes beside its ring in a drawing that fills its frame, laid no
   *  wider than its own names need. */
  private nameRoom(scene: MachineMapScene): { name: number; under: number } | undefined {
    if (!scene.nodes.length) return undefined;
    let name = 0;
    let under = 0;
    const fine = this.textSize("--size-fine");
    for (const node of scene.nodes) {
      name = Math.max(name, this.measured("--font-reading", fine, nameInGroup(node.name, node.group?.within ? node.group.label : null)));
      if (node.sub) under = Math.max(under, this.measured("--font-reading", fine, node.sub));
    }
    // A face not yet loaded measures as another: laid again once the faces
    // are in, and a little over meanwhile.
    if (document.fonts.status !== "loaded" && !this.awaitingFaces) {
      this.awaitingFaces = true;
      void document.fonts.ready.then(() => {
        this.awaitingFaces = false;
        this.preparedFrom = null;
        this.found = null;
        if (this.scene) this.update(this.scene);
      });
    }
    const room = (width: number) => Math.ceil(width * 1.05) + NAME_AIR + 8;
    return { name: room(name), under: under ? room(under) : 0 };
  }

  /** A phone's screen, or a page as narrow: under 600 pixels across. */
  private isNarrow(): boolean {
    const width = this.element.clientWidth;
    return width > 0 && width < 600;
  }

  /** Draws the layers over and under the ink at `scale`. */
  private frameLayers(scale: number): void {
    this.layerScale = scale;
    this.element.style.setProperty("--s", String(scale));
    // Nearer, each machine says what it is under its name.
    this.element.classList.toggle("map-detailed", scale >= DETAILED);
    if (!this.layerBox) return;
    for (const layer of [this.boxes, this.marks]) frameLayer(layer, this.layerBox, scale);
  }

  private onFrame(frame: CanvasFrame): void {
    const { view } = frame;
    this.pens.view(frame);
    if (view.scale !== this.layerScale) this.frameLayers(view.scale);
    const { turned, ringed } = this.settle();
    const far = this.far;
    // Laid again when the page becomes narrow or wide, or the frame changes
    // its shape, or its size where the drawing fills it; drawn again as it
    // turns far or near, or as the view leaves the part the paper was drawn
    // for; its hit areas made again as the rings come and go.
    let laid = false;
    if (this.scene) {
      const width = this.element.clientWidth;
      const height = this.element.clientHeight;
      const narrow = !this.options.still && this.isNarrow();
      const laidFor = this.options.still || narrow ? `${width}x${height}` : width && height ? (width / height).toFixed(1) : "none";
      if (narrow !== this.narrow || laidFor !== this.laidFor) {
        this.update(this.scene);
        laid = true;
      }
    }
    // The paper's part is left as the view leaves it, or comes so much
    // nearer that most of what was drawn round it is far out of view.
    const away = !far && !!this.region && (!holds(this.region, this.inView()) || view.scale > this.regionScale * 2);
    if (!laid && (turned || away || !this.composed)) {
      if (away) this.region = null;
      this.compose();
      this.place();
      this.mark();
    } else if (!laid && ringed) this.place();
    // Seen from afar: the names that have room at this scale, the dots and
    // lines where the view has them, and the name over what is pointed at.
    this.drawBlocks();
    this.drawFar(view);
    this.showTip();
    this.options.frame?.(frame);
  }

  /** How near the drawing is seen now, read from the camera before anything
   *  is drawn for it: whether it turned far or near since it was last read,
   *  and whether its rings came or went. */
  private settle(): { turned: boolean; ringed: boolean } {
    const band = Math.min(2, this.surface.camera.view.level);
    const far = band < 2;
    const turned = far !== this.far;
    const ringed = band !== this.band;
    this.far = far;
    this.band = band;
    if (turned) this.element.classList.toggle("map-far", far);
    return { turned, ringed };
  }

  /** The part of the drawing in view now, in its units. */
  private inView(): Bounds {
    const view = this.surface.camera.view;
    return { x: -view.x / view.scale, y: -view.y / view.scale, width: view.width / view.scale, height: view.height / view.scale };
  }

  /** The hit areas. Seen from afar: every hub, each group over its
   *  machines, then the boxes; where the machines are rings and a pointer
   *  presses them, each machine first, over the ring in its row. Nearer:
   *  every hub, box title, dashed ring and machine. Their names are written
   *  on the paper or the blocks; the canvas keeps them for those who hear the
   *  drawing. */
  private place(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const { words } = this.options;
    const far = this.far;
    const hubs = scene.hubs.map((hub): CanvasItem => ({ key: `hub:${hub.id}`, kind: "hub", point: placement.hubs.get(hub.id)!, radius: KNOT + 6, label: hub.name, sub: scene.hubLabels.get(hub.id) ?? "" }));
    // A person's or a group's "⋯", over the end of its box's strip: listed
    // before the strips, since a press goes to the first that holds it.
    const mores = placement.groups.flatMap((group): CanvasItem[] => {
      const more = this.moreOf(group);
      if (!more || far) return [];
      const { x, y, width } = group.box;
      const corners = cornersOf(group);
      return [{ key: more.key, kind: "group", point: [x + width - MORE / 2, y + group.strip / 2], radius: group.strip / 2, box: { x: x + width - MORE, y, width: MORE, height: group.strip, corners: [0, corners[1], corners[2], 0] }, label: more.label }];
    });
    const label = (group: Placement["groups"][number]) => (group.kind === "adding" ? (this.options.people?.addPerson ?? "") : group.label);
    let items: CanvasItem[];
    if (far) {
      // Seen from afar a group is pressed anywhere over its machines, before
      // the box it is in; the box anywhere else on its block. The dashed box
      // that adds a person is not drawn so far away.
      const pressed = (group: GroupNode): CanvasItem => ({ key: group.key, kind: "group", point: group.point, radius: 0, box: { ...group.box }, label: group.label, sub: this.countOf(group) });
      const groups = placement.groups.filter((group) => group.within).map(pressed);
      const boxes = placement.groups.filter((group) => !group.within && group.kind !== "adding").map(pressed);
      // Under a finger the rings are too small to tell apart: they are
      // pressed only nearer, the group or box flown to meanwhile.
      const rings =
        this.band === 1 && matchMedia("(pointer: fine)").matches
          ? scene.nodes.flatMap((node): CanvasItem[] => {
              const point = placement.machines.get(node.key);
              if (!point || node.hold === "visitor") return [];
              return [{ key: node.key, kind: "machine", point, radius: RING, box: { x: point[0] - RING - 10, y: point[1] - ROW / 2, width: 2 * RING + 20, height: ROW }, label: node.name, sub: node.sub, quiet: true }];
            })
          : [];
      items = [...hubs, ...rings, ...groups, ...boxes];
    } else {
      items = [
        ...hubs,
        ...mores,
        // A box is pressed anywhere else on its title strip, a group on its
        // band, where it is drawn.
        ...placement.groups.map((group): CanvasItem => ({
          key: group.key,
          kind: "group",
          point: group.point,
          radius: group.strip / 2 + 4,
          box: { x: group.box.x, y: group.box.y, width: group.box.width, height: group.strip, corners: cornersOf(group) },
          label: label(group),
          sub: group.kind === "adding" ? "" : this.countOf(group),
        })),
        ...scene.adds.flatMap((hub): CanvasItem[] => {
          const point = this.pointOf(addKey(hub));
          return point ? [{ key: addKey(hub), kind: "machine", point, radius: RING, label: scene.waiting.has(hub) ? words.waitingJoin : words.addMachine }] : [];
        }),
        // A visitor's machine is only seen: nothing to press.
        ...scene.nodes.flatMap((node): CanvasItem[] => {
          const point = placement.machines.get(node.key);
          return point && node.hold !== "visitor" ? [{ key: node.key, kind: "machine", point, radius: RING, label: node.name, sub: node.sub }] : [];
        }),
      ];
    }
    // The whole drawing is in view until it is moved, on a phone too.
    this.surface.setScene(items, [], placement.bounds);
    const selected = scene.selected && items.some((item) => item.key === scene.selected) ? scene.selected : null;
    if (this.surface.selection !== selected) this.surface.setSelection(selected);
  }

  /** The "⋯" at the end of a box's strip: a person's, for the network's
   *  administrator, or a group's the user may rename or end. */
  private moreOf(group: Placement["groups"][number]): { key: string; label: string } | null {
    if (group.person && this.options.people) return { key: personKey(group.hub, group.person.account), label: this.options.people.more(group.person.account) };
    const name = boxParts(group.kind).group;
    if (name !== null && this.scene?.grouped?.has(group.key)) return { key: groupMoreKey(group.hub, group.kind), label: this.options.words.groups.more(name) };
    return null;
  }

  /** What a box's strip says after its name: how many machines and how
   *  many online, or that a person has none yet. */
  private countOf(group: Placement["groups"][number]): string {
    if (!group.count && group.person && this.options.people) return this.options.people.noMachinesYet;
    return this.options.words.groupSub(group.count, group.online);
  }

  /** The ink, the boxes and the names, composed again only when what they
   *  show changes. */
  private compose(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const { words } = this.options;
    // Drawn for the distance it is seen at now, which may have changed
    // since the last frame when it is laid between frames.
    if (this.settle().turned) this.region = null;
    const far = this.far;
    const view = this.surface.camera.view;
    // The paper is drawn round what is in view; all of it while it is
    // small, or the view has no size yet.
    if (!far && !this.region) {
      // Until the view has its size nothing near is drawn: the first frame
      // draws it, round what it shows.
      if (!view.width) {
        this.composed = "";
        return;
      }
      const seen = this.inView();
      this.region = { x: seen.x - seen.width * AHEAD, y: seen.y - seen.height * AHEAD, width: seen.width * (1 + 2 * AHEAD), height: seen.height * (1 + 2 * AHEAD) };
      this.regionScale = view.scale;
    }
    const region = far ? placement.bounds : this.region!;
    const key = JSON.stringify([
      far,
      far ? null : region,
      scene.hubs,
      [...scene.hubLabels],
      scene.adds,
      [...scene.waiting],
      scene.nodes.map((node) => [node.key, node.name, node.sub, node.online, node.hold, node.reaching, node.here, node.busy, node.directAt, node.through]),
      [...scene.opening],
      [...placement.machines],
      placement.groups.map((group) => [group.key, group.label, group.box, group.count, group.online, group.busy, group.person?.locked, group.person?.visitor, group.person?.promotion, this.moreOf(group)?.key]),
    ]);
    if (key === this.composed) return;
    this.composed = key;
    this.compositions += 1;
    const css = getComputedStyle(document.documentElement);
    const graphite = css.getPropertyValue("--pen-graphite").trim() || css.getPropertyValue("--text").trim();
    const clay = css.getPropertyValue("--pen-clay").trim() || css.getPropertyValue("--accent").trim();
    this.inks = {
      graphite,
      clay,
      paper: css.getPropertyValue("--bg").trim() || "transparent",
      text: css.getPropertyValue("--text").trim() || graphite,
      box: Number.parseFloat(css.getPropertyValue("--radius-m")) || 10,
      group: Number.parseFloat(css.getPropertyValue("--radius-s")) || 6,
    };
    const strokes: PenStroke[] = [];
    const add = (d: string, bounds: Bounds, pen: PenStroke["pen"], colour: string, opacity = 1) => strokes.push({ d, bounds, pen, colour, opacity });
    const around = (point: Point, reach: number): Bounds => ({ x: point[0] - reach, y: point[1] - reach, width: reach * 2, height: reach * 2 });
    const spanning = (points: Point[], pad: number): Bounds => {
      const xs = points.map((p) => p[0]);
      const ys = points.map((p) => p[1]);
      return { x: Math.min(...xs) - pad, y: Math.min(...ys) - pad, width: Math.max(...xs) - Math.min(...xs) + pad * 2, height: Math.max(...ys) - Math.min(...ys) + pad * 2 };
    };
    const near = (point: Point, reach = RING * 2) => point[0] + reach >= region.x && point[0] - reach <= region.x + region.width && point[1] + reach >= region.y && point[1] - reach <= region.y + region.height;

    // Seen from afar nothing is inked: the dots and the hub's lines are
    // drawn plain (`drawFar`).
    if (!far) {
      // The lines: the hub's down to the boxes, each column's trunk and its
      // twigs, ruled and square at the corners; a twig to a machine out of
      // reach, broken.
      for (const branch of placement.branches) {
        const [from, to] = [branch.points[0]!, branch.points[branch.points.length - 1]!];
        if (Math.hypot(to[0] - from[0], to[1] - from[1]) < 1 || !crosses(region, from, to)) continue;
        const random = seeded(`branch:${from.join(",")}:${to.join(",")}`);
        if (branch.out) for (const d of ticks(branch.points, 5, 3.5, 1.1, random)) add(d, spanning(branch.points, 3), "dry", graphite, 0.45);
        else add(ruled(from, to, 1.2, random), spanning(branch.points, 3), "marker", graphite, 0.6);
      }

      for (const hub of scene.hubs) {
        const knot = placement.hubs.get(hub.id)!;
        const random = seeded(`knot:${hub.id}`);
        add(circle(knot, KNOT, 1.8, random), around(knot, KNOT + 4), hub.online ? "marker" : "dry", graphite, hub.online ? 0.9 : 0.5);
        if (hub.online) add(blot(knot, 2.4, random), around(knot, 5), "marker", graphite, 0.9);
      }

      // The dashed box that adds a person, a small cross at its start.
      for (const group of placement.groups) {
        if (group.kind !== "adding") continue;
        const { x, y, width } = group.box;
        const random = seeded(`adding:${group.hub}`);
        for (const d of ticks(stripPoints(x, y, width, group.strip, 7), 7, 4, 1.3, random)) add(d, { x: x - 4, y: y - 4, width: width + 8, height: group.strip + 8 }, "dry", graphite, 0.6);
        const at: Point = [x + TRUNK + 14, y + group.strip / 2];
        const arm = 5;
        add(outline([[at[0] - arm, at[1]], [at[0] + arm, at[1]]], 1.5, random), around(at, arm + 3), "dry", graphite, 0.75);
        add(outline([[at[0], at[1] - arm], [at[0], at[1] + arm]], 1.5, random), around(at, arm + 3), "dry", graphite, 0.75);
      }

      // The dashed ring that adds a machine, a small cross in it.
      for (const hub of scene.adds) {
        const at = this.pointOf(addKey(hub));
        if (!at || !near(at)) continue;
        const random = seeded(`add:${hub}`);
        const waiting = scene.waiting.has(hub);
        for (const d of ticks(ringPoints(at, RING, 56), 7, 4, 1.3, random)) add(d, around(at, RING + 4), "dry", waiting ? clay : graphite, waiting ? 0.9 : 0.6);
        if (!waiting) {
          const arm = RING * 0.32;
          add(outline([[at[0] - arm, at[1]], [at[0] + arm, at[1]]], 1.6, random), around(at, arm + 3), "dry", graphite, 0.7);
          add(outline([[at[0], at[1] - arm], [at[0], at[1] + arm]], 1.6, random), around(at, arm + 3), "dry", graphite, 0.7);
        }
      }

      for (const node of scene.nodes) {
        const point = placement.machines.get(node.key);
        if (!point || !near(point)) continue;
        const [x, y] = point;
        const online = node.online === true;
        const random = seeded(`map:${node.key}`);
        // The ring: graphite for the account's own and others' machines it
        // works on, clay and broken where an agent alone assists.
        if (node.hold === "assist") {
          for (const d of ticks(ringPoints(point, RING, 64), 9, 5.5, 2, random)) add(d, around(point, RING + 4), online ? "marker" : "dry", online ? clay : graphite, online ? 1 : 0.5);
        } else if (node.hold === "visitor") {
          // A visitor's: a faint ring, only seen.
          add(circle(point, RING, 1.8, random), around(point, RING + 4), "dry", graphite, online ? 0.38 : 0.22);
        } else if (online) {
          add(circle(point, RING, 2, random), around(point, RING + 4), "marker", graphite, 0.9);
          add(circle(point, RING + 1.2, 1, random), around(point, RING + 4), "marker", graphite, 0.45);
        } else {
          add(circle(point, RING, 2.1, random), around(point, RING + 4), "dry", graphite, 0.5);
        }
        // A phone: its outline in the ring, a short stroke at its foot.
        if (node.device) {
          const faint = online ? 0.85 : 0.45;
          add(outline(phoneLoop(point, 13, 21, 3.5), 1.6, random, { pressure: 0.2, taper: 0 }), around(point, 13), "marker", graphite, faint);
          const foot: Point[] = [
            [x - 2.6, y + 7],
            [x + 2.6, y + 7],
          ];
          add(outline(foot, 1.4, random, { pressure: 0.2, taper: 0.3 }), spanning(foot, 3), "marker", graphite, faint);
        } else if (node.here) add(blot(point, 3, random), around(point, 5), "marker", graphite, 0.9);
        // Others may connect to it directly: a dashed halo round its ring,
        // in pencil while the hub checks it.
        const opening = scene.opening.has(node.key);
        if (node.directAt?.length || opening) {
          for (const d of ticks(ringPoints(point, RING + 7, 72), 5, 3.4, 1.1, random)) add(d, around(point, RING + 11), opening ? "dry" : "marker", graphite, opening ? 0.5 : 0.7);
        }
        // Another's machine whose agents may reach the account's: a short
        // clay tick across the top of its ring.
        if (node.reaching) {
          const tick: Point[] = [
            [x - RING * 0.45, y - RING * 1.08],
            [x - RING * 0.28, y - RING * 0.86],
            [x + RING * 0.1, y - RING * 1.3],
          ];
          add(outline(tick, 2.2, random, { pressure: 0.25, taper: 0.2 }), spanning(tick, 4), "marker", clay);
        }
        // The agents at work or waiting on it, round the upper right of its
        // ring, clear of its name on its right: one dot each, no names.
        for (let place = 0; place < node.busy; place++) {
          const at = -1.15 + place * 0.36;
          const spot: Point = [x + Math.cos(at) * (RING + 13), y + Math.sin(at) * (RING + 13)];
          add(blot(spot, 4, seeded(`map-agent:${node.key}:${place}`)), around(spot, 7), "marker", clay);
        }
      }
    }
    void this.pens.update(strokes).catch((error: unknown) => console.warn("the machines drawing could not be inked", error));

    // Each layer is as large as the drawing and the room round it, drawn to
    // the camera's scale by its own view box (`MapCanvas.over`).
    const { bounds } = placement;
    this.layerBox = { x: bounds.x - LAYER_ROOM, y: bounds.y - LAYER_ROOM, width: bounds.width + 2 * LAYER_ROOM, height: bounds.height + 2 * LAYER_ROOM };
    this.frameLayers(view.scale || 1);

    // The boxes: a wash for the body and a deeper one for the title strip,
    // no line round either. Seen from afar they are the blocks instead.
    const inRegion = (box: Bounds) => box.x + box.width >= region.x && box.x <= region.x + region.width && box.y + box.height >= region.y && box.y <= region.y + region.height;
    this.boxes.innerHTML = far
      ? ""
      : placement.groups
          .filter((group) => inRegion(group.box))
          .map((group) => {
            const { x, y, width, height } = group.box;
            // The dashed box that adds a person is its pen strokes alone.
            if (group.kind === "adding") return "";
            const tone = toneOf(group.kind);
            const locked = group.person?.locked ? " locked" : "";
            const visitor = group.person?.visitor ? " visitor" : "";
            // A group's band lies in its owner's box over the group's machines.
            if (group.within) return `<g class="box ${tone}"><rect class="box-band" x="${x}" y="${y}" width="${width}" height="${group.strip}" rx="6"/></g>`;
            const title = group.strip;
            const strip =
              height <= title
                ? `<rect class="box-title" x="${x}" y="${y}" width="${width}" height="${title}" rx="7"/>`
                : `<rect class="box-body" x="${x}" y="${y}" width="${width}" height="${height}" rx="7"/><path class="box-title" d="M${x} ${y + title}V${y + 7}Q${x} ${y} ${x + 7} ${y}H${x + width - 7}Q${x + width} ${y} ${x + width} ${y + 7}V${y + title}Z"/>`;
            return `<g class="box ${tone}${locked}${visitor}">${strip}</g>`;
          })
          .join("");

    // The names: the hub's above it; near, each box's on its strip, each
    // machine's right of its ring with what it is under it. Each is placed
    // by its point in the drawing's units, `--x` and `--y` (and a strip by
    // its size, `--w` and `--h`), the scale applied by the styles.
    const text: string[] = [];
    const spot = (x: number, y: number) => `--x:${x.toFixed(1)};--y:${y.toFixed(1)}`;
    for (const hub of scene.hubs) {
      const [x, y] = placement.hubs.get(hub.id)!;
      const said = scene.hubLabels.get(hub.id) ?? "";
      text.push(`<div class="mw-hub" data-name="hub:${escapeText(hub.id)}" style="${spot(x, y)}"><span class="mw-hub-name">${escapeText(hub.name)}</span>${said ? `<span class="mw-hub-sub">${escapeText(said)}</span>` : ""}</div>`);
    }
    if (!far) {
      const strip = (x: number, y: number, width: number, height: number) => `${spot(x, y)};--w:${width.toFixed(1)};--h:${height.toFixed(1)}`;
      for (const group of placement.groups) {
        if (!inRegion(group.box)) continue;
        const { x, y, width } = group.box;
        if (group.kind === "adding") {
          text.push(`<div class="mw-strip adding" style="${strip(x + TRUNK + 26, y, width - TRUNK - 26, group.strip)}"><span class="mw-say"><span class="mw-title">${escapeText(this.options.people?.addPerson ?? "")}</span></span></div>`);
          continue;
        }
        const locked = group.person?.locked ? " locked" : "";
        // A visitor's box says so, and so does one asked to become a member.
        const people = this.options.people;
        const role = !people ? "" : group.person?.promotion ? people.waitingTag : group.person?.visitor ? people.visitorTag : "";
        // A small lock while the person is locked, then the "⋯".
        const end = this.moreOf(group) ? `<span class="mw-end">${group.person?.locked ? glyph("lock", 14) : ""}${glyph("more", 18)}</span>` : "";
        const title = group.label ? `<span class="mw-title">${escapeText(group.label)}</span>` : "";
        text.push(
          `<div class="mw-strip${group.within ? " band" : ""}${locked}" style="${strip(x + TRUNK + 14, y, width - TRUNK - 14, group.strip)}"><span class="mw-say">${title}${role ? `<span class="mw-role">${escapeText(role)}</span>` : ""}<span class="mw-count">${escapeText(this.countOf(group))}</span></span>${end}</div>`,
        );
      }
      const nameAt = (key: string, point: Point, name: string, sub: string, quiet = false) =>
        text.push(`<div class="mw-machine${quiet ? " quiet" : ""}" data-name="${escapeText(key)}" style="${spot(point[0] + RING + 15, point[1])}"><span class="mw-name">${escapeText(name)}</span>${sub ? `<span class="mw-sub">${escapeText(sub)}</span>` : ""}</div>`);
      for (const hub of scene.adds) {
        const at = this.pointOf(addKey(hub));
        if (at && near(at)) nameAt(addKey(hub), at, scene.waiting.has(hub) ? words.waitingJoin : words.addMachine, "", true);
      }
      for (const node of scene.nodes) {
        const point = placement.machines.get(node.key);
        if (!point || !near(point)) continue;
        const through = node.through ? words.viaShort(node.through) : "";
        nameAt(node.key, point, nameInGroup(node.name, node.group?.within ? node.group.label : null), [node.sub, through].filter(Boolean).join(" "), node.online === false || node.hold === "visitor");
      }
    }
    // A name runs from its ring to a little short of the next column's line.
    this.words.style.setProperty("--room", String(placement.room - 5 - NAME_AIR));
    this.words.innerHTML = text.join("");
    this.blocksDrawn = "";
    this.drawBlocks();
    this.drawFar(view);
  }

  /** Seen from afar, on each box's block (`drawFar`), the names of the box
   *  and of each of its groups, with how many machines where there is room;
   *  faint where nothing looked for is in it. The styles keep them in their
   *  places as the drawing zooms; they are written only for the boxes round
   *  what is in view, again as the view leaves them, as the distance turns,
   *  as the scale moves by more than a quarter of a doubling, for what has
   *  room then, and as the pointer comes to rest on another. */
  private drawBlocks(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const view = this.surface.camera.view;
    const scale = view.scale || 1;
    if (!this.far || !view.width) {
      if (this.blocks.firstChild) this.blocks.innerHTML = "";
      this.blocksDrawn = "";
      this.blocksRegion = null;
      return;
    }
    const seen = this.inView();
    const away = !this.blocksRegion || !holds(this.blocksRegion, seen);
    const key = `${this.markings}:${this.compositions}:${this.band}:${this.pointed}`;
    const rescaled = !this.blocksScale || Math.abs(Math.log2(scale / this.blocksScale)) > 0.25;
    if (key === this.blocksDrawn && !away && !rescaled) return;
    if (away || rescaled) this.blocksRegion = { x: seen.x - seen.width * HEADS_AHEAD, y: seen.y - seen.height * HEADS_AHEAD, width: seen.width * (1 + 2 * HEADS_AHEAD), height: seen.height * (1 + 2 * HEADS_AHEAD) };
    this.blocksDrawn = key;
    this.blocksScale = scale;
    const region = this.blocksRegion!;
    const { words } = this.options;
    const people = this.options.people;
    const found = scene.found ?? null;
    // The boxes something looked for stands in.
    const lit = found ? new Set([...found].map((machine) => placement.boxOf.get(machine))) : null;
    const text = this.textSize("--size-text");
    const fine = this.textSize("--size-fine");
    const lineOf = (size: number) => Math.max(size, fine) * 1.3 + 2;
    // Far, the names lie at the top of the blocks, over the drops; nearer,
    // where rings would be under them, each sits on its own title strip or
    // band and rises above it, into the room the drawing leaves there.
    const ringed = this.band === 1;
    const inside = new Map<string, GroupNode[]>();
    for (const group of placement.groups) {
      if (!group.within) continue;
      const known = inside.get(group.within);
      if (known) known.push(group);
      else inside.set(group.within, [group]);
    }
    // Each from a point in the drawing's units, so many pixels below it:
    // far, the box's top; nearer, the bottom of its strip or band.
    const spot = (x: number, y: number, width: number, top: number) => `--x:${x.toFixed(1)};--y:${y.toFixed(1)};--w:${width.toFixed(1)};--top:${top.toFixed(1)}px`;
    const html: string[] = [];
    for (const box of placement.groups) {
      if (box.within || box.kind === "adding") continue;
      const { x, y, width, height } = box.box;
      if (x > region.x + region.width || x + width < region.x || y > region.y + region.height || y + height < region.y) continue;
      if (!ringed && !box.count && height * scale < SLIVER) continue;
      const role = !people ? "" : box.person?.promotion ? people.waitingTag : box.person?.visitor ? people.visitorTag : "";
      const count = box.count || !box.person ? words.groupCount(box.count) : this.countOf(box);
      const boxLine = lineOf(text);
      const boxTop = ringed ? TITLE * scale - boxLine : HEAD_TOP;
      const tall = ringed || height * scale >= HEAD_TOP + boxLine + 4;
      const head = tall ? this.headOf(box.label, "--font-emphasis", text, role, count, width * scale) : null;
      const headRight = head ? HEAD_LEFT + head.width : -Infinity;
      // Each name on the ground of its block, deeper while the pointer rests
      // there; faint with its block.
      const state = `${toneOf(box.kind)}${box.person?.locked ? " locked" : ""}${lit && !lit.has(box.key) ? " unlit" : ""}${this.pointed === box.key ? " pointed" : ""}`;
      if (head) html.push(`<div class="mb-head ${state}" style="${ringed ? spot(x, y + TITLE, width, -boxLine) : spot(x, y, width, HEAD_TOP)}">${head.html}</div>`);
      for (const group of inside.get(box.key) ?? []) {
        const below = group.box.y - y;
        const groupLine = lineOf(fine);
        // Its name's top, from the group's own top, and from the box's.
        const top = ringed ? BAND * scale - groupLine : HEAD_TOP - below * scale;
        const atTop = below * scale + top;
        const left = (group.box.x - x) * scale + HEAD_LEFT;
        const clash = !!head && atTop < boxTop + boxLine && atTop + groupLine > boxTop && left < headRight + HEAD_CLEAR;
        const said = tall && !clash ? this.headOf(group.label, "--font-reading", fine, "", words.groupCount(group.count), group.box.width * scale) : null;
        if (said) html.push(`<div class="mb-head group ${state}${this.pointed === group.key ? " within" : ""}" style="${ringed ? spot(group.box.x, group.box.y + BAND, group.box.width, -groupLine) : spot(group.box.x, y, group.box.width, HEAD_TOP)}">${said.html}</div>`);
      }
    }
    this.blocks.innerHTML = html.join("");
  }

  /** The name on a block seen from afar, `room` pixels across, and what
   *  follows it where there is room: a tag, then how many machines; with
   *  how wide its ground is. Nothing where the block has room for no more
   *  than a few letters. */
  private headOf(name: string, face: string, size: number, role: string, count: string, room: number): { html: string; width: number } | null {
    const fine = this.textSize("--size-fine");
    const across = room - 2 * (HEAD_LEFT + HEAD_PAD);
    if (across < HEAD_LEAST) return null;
    const gap = 8;
    const parts = name ? [`<span class="mb-name">${escapeText(name)}</span>`] : [];
    let used = name ? this.measured(face, size, name) : -gap;
    if (role) {
      const width = this.measured("--font-emphasis", fine, role);
      if (used + gap + width <= across) {
        parts.push(`<span class="mb-role">${escapeText(role)}</span>`);
        used += gap + width;
      }
    }
    const counted = this.measured("--font-reading", fine, count);
    if (used + gap + counted <= across) {
      parts.push(`<span class="mb-count">${escapeText(count)}</span>`);
      used += gap + counted;
    }
    if (!parts.length) return null;
    return { html: parts.join(""), width: Math.min(used, across) + 2 * HEAD_PAD };
  }

  /** What the pointer rests on: seen from afar its block deepens, names and
   *  all, on the next frame, and it is named over it. */
  private point(key: string | null): void {
    if (key === this.pointed) return;
    this.pointed = key;
    if (!this.far) return;
    this.drawBlocks();
    this.showTip();
  }

  /** Seen from afar, what the pointer rests on, named in a small card over
   *  it: a box or a group with how many of its machines are online, a ring
   *  with what the machine is. */
  private showTip(): void {
    const { placement, scene } = this;
    // The one chosen is named on its bar already.
    const pointed = this.far && placement && scene && this.pointed !== scene.selected ? this.pointed : null;
    const group = pointed ? placement!.groups.find((one) => one.key === pointed && one.kind !== "adding") : undefined;
    const node = pointed && !group ? scene!.nodes.find((one) => one.key === pointed) : undefined;
    const point = node ? placement!.machines.get(node.key) : undefined;
    if (!group && !(node && point)) {
      if (!this.tip.hidden) this.tip.hidden = true;
      return;
    }
    const key = group?.key ?? node!.key;
    if (this.tip.dataset.for !== key) {
      this.tip.dataset.for = key;
      const said = group ? this.countOf(group) : node!.sub;
      this.tip.innerHTML = `<span class="map-tip-name">${escapeText(group?.label ?? node!.name)}</span>${said ? `<span class="map-tip-count">${escapeText(said)}</span>` : ""}`;
    }
    this.tip.hidden = false;
    const view = this.surface.camera.view;
    // Over the block's corner, or over the ring.
    const [x, y] = group ? this.surface.camera.toScreen([group.box.x, group.box.y]) : this.surface.camera.toScreen([point![0] - RING, point![1] - RING]);
    const width = this.tip.offsetWidth;
    const height = this.tip.offsetHeight;
    const left = Math.max(8, Math.min(view.width - width - 8, x));
    // Over it, or inside its top where there is no room above it.
    const top = y - height - 6 >= 8 ? y - height - 6 : Math.max(8, Math.min(view.height - height - 8, y + 6));
    this.tip.style.transform = `translate(${left.toFixed(1)}px,${top.toFixed(1)}px)`;
  }

  /** Seen from afar, drawn on the canvas under the blocks, each frame and
   *  only what the view holds: each hub's knot and its lines; further, its
   *  trunk and the line over each row of its boxes, and a drop of ink for
   *  each machine; nearer, every line down to the machines and each one's
   *  ring, along the line the pen draws it on the paper, with the marks the
   *  paper puts round it. The one chosen and those framed are circled in
   *  clay. */
  private drawFar(view: View): void {
    const canvas = this.dots;
    const ratio = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.round(view.width * ratio));
    const height = Math.max(1, Math.round(view.height * ratio));
    const sized = canvas.width === width && canvas.height === height;
    if (!this.far && sized && this.dotsBare) return;
    const context = canvas.getContext("2d");
    if (!context) return;
    if (!sized) {
      canvas.width = width;
      canvas.height = height;
    }
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, width, height);
    const { scene, placement } = this;
    this.dotsBare = true;
    if (!this.far || !scene || !placement) return;
    this.dotsBare = false;
    const version = `${this.compositions}:${this.markings}`;
    if (this.dotsFor !== version) {
      this.dotsFor = version;
      [this.farDots, this.farRings] = this.inkOf(scene, placement);
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    const { scale } = view;
    const ringed = this.band === 1;
    const at = (point: Point): Point => [view.x + point[0] * scale, view.y + point[1] * scale];
    const seen = (x: number, y: number, reach: number) => x >= -reach && y >= -reach && x <= view.width + reach && y <= view.height + reach;
    const { graphite, clay, paper } = this.inks;
    this.drawWashes(context, view, scene, placement);
    context.lineCap = "round";
    context.lineJoin = "round";
    // The lines: the hub's further; every one nearer, a twig to a machine
    // out of reach broken.
    for (const out of ringed ? [false, true] : [false]) {
      context.lineWidth = ringed ? 1 : 1.25;
      context.strokeStyle = graphite;
      context.globalAlpha = out ? 0.3 : 0.45;
      context.setLineDash(out ? [3, 3] : []);
      context.beginPath();
      for (const branch of placement.branches) {
        if (!!branch.out !== out || (!ringed && !branch.trunk)) continue;
        const [fromX, fromY] = at(branch.points[0]!);
        const [toX, toY] = at(branch.points[branch.points.length - 1]!);
        if (Math.max(fromX, toX) < 0 || Math.min(fromX, toX) > view.width || Math.max(fromY, toY) < 0 || Math.min(fromY, toY) > view.height) continue;
        context.moveTo(fromX, fromY);
        context.lineTo(toX, toY);
      }
      context.stroke();
    }
    context.setLineDash([]);
    // Each knot over its trunk: a ring, a dot in it while the hub is online.
    context.lineWidth = 1.5;
    context.strokeStyle = graphite;
    for (const hub of scene.hubs) {
      const [x, y] = at(placement.hubs.get(hub.id)!);
      context.beginPath();
      context.arc(x, y, 6, 0, Math.PI * 2);
      context.globalAlpha = 1;
      context.fillStyle = paper;
      context.fill();
      context.globalAlpha = hub.online ? 0.9 : 0.5;
      context.stroke();
      if (!hub.online) continue;
      context.beginPath();
      context.arc(x, y, 2, 0, Math.PI * 2);
      context.fillStyle = graphite;
      context.fill();
    }
    const radius = ringed ? RING * scale : Math.max(DOT_LEAST, Math.min(DOT_MOST, RING * 2 * scale * 0.36)) / 2;
    if (ringed) this.drawRings(context, view, at, seen);
    else {
      // The machines, each a drop.
      for (const set of this.farDots) {
        const r = set.large ? radius + 1.5 : radius;
        context.globalAlpha = set.alpha;
        context.fillStyle = set.colour;
        context.beginPath();
        for (const point of set.points) {
          const [x, y] = at(point);
          if (!seen(x, y, r)) continue;
          context.moveTo(x + r, y);
          context.arc(x, y, r, 0, Math.PI * 2);
        }
        context.fill();
      }
    }
    // The one chosen, and those framed, circled.
    const round = ringed ? (RING + 8) * scale : radius + 4;
    context.globalAlpha = 1;
    context.strokeStyle = clay;
    context.lineWidth = ringed ? 2 : 1.6;
    context.beginPath();
    for (const key of [scene.selected, ...scene.framed]) {
      const point = key ? placement.machines.get(key) : undefined;
      if (!point) continue;
      const [x, y] = at(point);
      if (!seen(x, y, round + 2)) continue;
      context.moveTo(x + round, y);
      context.arc(x, y, round, 0, Math.PI * 2);
    }
    context.stroke();
    context.globalAlpha = 1;
  }

  /** Each box seen from afar, a block of its wash: clay for the account's
   *  own, paler for an assisting agent's, the words' ink for another's;
   *  deeper under the pointer, and a group's place in it deeper still while
   *  the pointer rests there; faint for a locked person, and where nothing
   *  looked for stands in it. Only what the view holds. */
  private drawWashes(context: CanvasRenderingContext2D, view: View, scene: MachineMapScene, placement: Placement): void {
    const { scale } = view;
    const { clay, text, box: round, group: groupRound } = this.inks;
    const found = scene.found ?? null;
    const lit = found ? new Set([...found].map((machine) => placement.boxOf.get(machine))) : null;
    const ringed = this.band === 1;
    const fill = (bounds: Bounds, radius: number, colour: string, alpha: number) => {
      const x = view.x + bounds.x * scale;
      const y = view.y + bounds.y * scale;
      const width = bounds.width * scale;
      const height = bounds.height * scale;
      if (x > view.width || y > view.height || x + width < 0 || y + height < 0) return;
      context.globalAlpha = alpha;
      context.fillStyle = colour;
      context.beginPath();
      context.roundRect(x, y, width, height, Math.min(radius, width / 2, height / 2));
      context.fill();
    };
    const boxes = new Map(placement.groups.map((group) => [group.key, group]));
    for (const group of placement.groups) {
      if (group.kind === "adding") continue;
      const owner = group.within ? boxes.get(group.within) : group;
      if (!owner) continue;
      const tone = toneOf(owner.kind);
      const faint = (owner.person?.locked ? 0.45 : 1) * (lit && !lit.has(owner.key) ? 0.4 : 1);
      const colour = tone === "others" ? text : clay;
      if (group.within) {
        // A group has no wash of its own until the pointer rests on it.
        if (this.pointed === group.key) fill(group.box, groupRound, colour, (tone === "mine" ? 0.09 : 0.07) * faint);
        continue;
      }
      if (!ringed && !group.count && group.box.height * scale < SLIVER) continue;
      const pointed = this.pointed === group.key;
      const alpha = tone === "mine" ? (pointed ? 0.19 : 0.11) : tone === "assist" ? (pointed ? 0.12 : 0.06) : pointed ? 0.11 : 0.06;
      fill(group.box, round, colour, alpha * faint);
    }
    context.globalAlpha = 1;
  }

  /** The machines' rings drawn small, between the drops and the paper:
   *  each along the centre line the pen draws its ring on, in ink while it
   *  is online, faint while away, broken clay where another's agent alone
   *  assists; the marks round it as the paper has them, a dot at its middle
   *  for this machine, a phone in it, a dashed halo where others connect
   *  to it, a clay dot for each agent at work; those looked for ringed in
   *  clay. */
  private drawRings(context: CanvasRenderingContext2D, view: View, at: (point: Point) => Point, seen: (x: number, y: number, reach: number) => boolean): void {
    const { scale } = view;
    const { graphite, clay } = this.inks;
    const r = RING * scale;
    const reach = (RING + 20) * scale;
    const shown = this.farRings.filter((ring) => {
      const [x, y] = at(ring.point);
      return seen(x, y, reach);
    });
    // The rings, those inked alike in one stroke.
    const strokes = new Map<string, FarRing[]>();
    for (const ring of shown) {
      const id = `${ring.colour}:${ring.alpha}:${ring.broken}`;
      const same = strokes.get(id);
      if (same) same.push(ring);
      else strokes.set(id, [ring]);
    }
    for (const rings of strokes.values()) {
      const { colour, alpha, broken } = rings[0]!;
      context.strokeStyle = colour;
      context.globalAlpha = alpha;
      context.lineWidth = Math.max(1.1, 2 * scale);
      context.setLineDash(broken ? [Math.max(2, 9 * scale), Math.max(1.5, 5.5 * scale)] : []);
      context.beginPath();
      for (const ring of rings) {
        const [x, y] = at(ring.point);
        if (broken) {
          context.moveTo(x + r, y);
          context.arc(x, y, r, 0, Math.PI * 2);
          continue;
        }
        const line = this.ringLine(ring.key);
        context.moveTo(x + line[0]! * r, y + line[1]! * r);
        for (let index = 2; index < line.length; index += 2) context.lineTo(x + line[index]! * r, y + line[index + 1]! * r);
      }
      context.stroke();
    }
    context.setLineDash([]);
    // Others may connect to it directly: a dashed halo round its ring.
    context.strokeStyle = graphite;
    context.lineWidth = 1;
    context.globalAlpha = 0.5;
    context.setLineDash([Math.max(1.5, 5 * scale), Math.max(1.5, 3.4 * scale)]);
    context.beginPath();
    for (const ring of shown) {
      if (!ring.halo) continue;
      const [x, y] = at(ring.point);
      context.moveTo(x + (RING + 7) * scale, y);
      context.arc(x, y, (RING + 7) * scale, 0, Math.PI * 2);
    }
    context.stroke();
    context.setLineDash([]);
    // This machine's dot, and a phone's outline in its ring.
    context.globalAlpha = 0.85;
    context.fillStyle = graphite;
    context.beginPath();
    for (const ring of shown) {
      if (!ring.here) continue;
      const [x, y] = at(ring.point);
      const dot = Math.max(1.2, 3 * scale);
      context.moveTo(x + dot, y);
      context.arc(x, y, dot, 0, Math.PI * 2);
    }
    context.fill();
    context.lineWidth = 1;
    context.globalAlpha = 0.6;
    context.beginPath();
    for (const ring of shown) {
      if (!ring.device) continue;
      const [x, y] = at(ring.point);
      context.roundRect(x - 6.5 * scale, y - 10.5 * scale, 13 * scale, 21 * scale, 3.5 * scale);
    }
    context.stroke();
    // The agents at work on it, round the upper right of its ring.
    context.globalAlpha = 1;
    context.fillStyle = clay;
    context.beginPath();
    const spot = Math.max(1.6, 4 * scale);
    for (const ring of shown) {
      const [x, y] = at(ring.point);
      for (let place = 0; place < ring.busy; place += 1) {
        const angle = -1.15 + place * 0.36;
        const dotX = x + Math.cos(angle) * (RING + 13) * scale;
        const dotY = y + Math.sin(angle) * (RING + 13) * scale;
        context.moveTo(dotX + spot, dotY);
        context.arc(dotX, dotY, spot, 0, Math.PI * 2);
      }
    }
    context.fill();
    // What is looked for and found, ringed in clay.
    context.strokeStyle = clay;
    context.globalAlpha = 0.55;
    context.lineWidth = 2;
    context.beginPath();
    for (const ring of shown) {
      if (!ring.found) continue;
      const [x, y] = at(ring.point);
      context.moveTo(x + (RING + 8) * scale, y);
      context.arc(x, y, (RING + 8) * scale, 0, Math.PI * 2);
    }
    context.stroke();
  }

  /** The centre line the pen draws `key`'s ring on, for a ring of radius 1
   *  round the origin: the same line, so the ring drawn small is the very
   *  ring the paper draws nearer. */
  private ringLine(key: string): number[] {
    let line = this.ringLines.get(key);
    if (!line) {
      if (this.ringLines.size > 20000) this.ringLines.clear();
      const points = circlePoints([0, 0], RING, seeded(`map:${key}`));
      // Every other point is enough at this size.
      line = points.filter((_, index) => index % 2 === 0 || index === points.length - 1).flatMap(([x, y]) => [x / RING, y / RING]);
      this.ringLines.set(key, line);
    }
    return line;
  }

  private readonly ringLines = new Map<string, number[]>();

  /** The machines seen from afar: their drops gathered by how each is
   *  inked, in ink while online, faint while away or only seen, in clay
   *  while something works on it or another's agent alone assists there,
   *  faint in a locked person's box, and, while machines are looked for,
   *  those found larger and in clay, the rest fainter; and their rings as
   *  they are drawn nearer. */
  private inkOf(scene: MachineMapScene, placement: Placement): [FarDots[], FarRing[]] {
    const { graphite, clay } = this.inks;
    const found = scene.found ?? null;
    const locked = new Set(placement.groups.filter((group) => group.person?.locked).map((group) => group.key));
    const sets = new Map<string, FarDots>();
    const put = (colour: string, alpha: number, large: boolean, point: Point) => {
      const id = `${colour}:${alpha}:${large}`;
      let set = sets.get(id);
      if (!set) {
        set = { colour, alpha, large, points: [] };
        sets.set(id, set);
      }
      set.points.push(point);
    };
    const rings: FarRing[] = [];
    for (const node of scene.nodes) {
      const point = placement.machines.get(node.key);
      if (!point) continue;
      const online = node.online === true;
      const working = online && node.busy > 0;
      const assist = node.hold === "assist";
      const dim = (locked.has(placement.boxOf.get(node.key) ?? "") ? 0.45 : 1) * (found && !found.has(node.key) ? 0.3 : 1);
      const ink = (alpha: number) => Math.round(alpha * dim * 100) / 100;
      // The ring as the paper inks it.
      const ringAlpha = node.hold === "visitor" ? (online ? 0.38 : 0.22) : assist ? (online ? 1 : 0.5) : online ? 0.9 : 0.42;
      rings.push({ key: node.key, point, colour: assist && online ? clay : graphite, alpha: ink(ringAlpha), broken: assist, here: node.here && !node.device, device: !!node.device, halo: !!node.directAt?.length || scene.opening.has(node.key), busy: online ? node.busy : 0, found: !!found?.has(node.key) });
      if (found?.has(node.key)) {
        put(clay, 1, true, point);
        continue;
      }
      const alpha = node.hold === "visitor" ? 0.2 : working ? 1 : online ? (assist ? 0.6 : 0.78) : 0.26;
      put(working || assist ? clay : graphite, ink(alpha), false, point);
    }
    // The found over the rest.
    return [[...sets.values()].sort((a, b) => Number(a.large) - Number(b.large)), rings];
  }

  /** Drawn live over the ink: the clay circle round the one chosen and the
   *  ones framed, the circles round machines just come, and strokes moving
   *  along the twig of a machine something works on. */
  private mark(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const far = this.far;
    const lit = scene.lit ?? null;
    const found = scene.found ?? null;
    const key = JSON.stringify([scene.selected, scene.framed, [...scene.fresh], far, scene.nodes.map((node) => [node.key, node.online, node.busy > 0]), [...placement.machines], lit && [...lit], found && [...found], this.compositions]);
    if (key === this.marked) return;
    this.marked = key;
    this.markings += 1;
    this.drawBlocks();
    this.drawFar(this.surface.camera.view);
    const circles: Ring[] = [];
    const moons: string[] = [];
    const flows: string[] = [];
    if (!far) {
      const chosen = scene.selected && !scene.selected.startsWith("group:") ? this.pointOf(scene.selected) : null;
      if (chosen) circles.push({ id: `chosen:${scene.selected}`, kind: "chosen", d: circle(chosen, scene.selected!.startsWith("hub:") ? KNOT + 9 : RING + 8, 3, seeded(`chosen:${scene.selected}`)) });
      for (const framed of scene.framed) {
        const point = this.pointOf(framed);
        if (point && framed !== scene.selected) circles.push({ id: `chosen:${framed}`, kind: "chosen", d: circle(point, RING + 8, 3, seeded(`chosen:${framed}`)) });
      }
      for (const fresh of scene.fresh) {
        const point = this.pointOf(fresh);
        if (point && fresh !== scene.selected) circles.push({ id: `arrived:${fresh}`, kind: "arrived", d: circle(point, RING + 8, 3, seeded(`arrived:${fresh}`)) });
      }
      // The machines that can take the server glow with a broken clay ring,
      // a small moon beside one that goes to sleep.
      for (const [key, candidate] of lit ?? []) {
        const point = this.pointOf(key);
        if (!point) continue;
        circles.push({ id: `lit:${key}`, kind: "lit", d: circle(point, RING + 8, 3, seeded(`lit:${key}`)) });
        if (candidate.sleeps) moons.push(`<path class="moon" d="${crescent([point[0] - (RING + 18) * 0.72, point[1] - (RING + 18) * 0.72], 7)}"/>`);
      }
      // What is looked for and found is ringed and stays in ink, as still as
      // the one chosen is.
      for (const key of found ?? []) {
        const point = this.pointOf(key);
        if (point && key !== scene.selected) circles.push({ id: `found:${key}`, kind: "found", d: circle(point, RING + 8, 3, seeded(`found:${key}`)) });
      }
      for (const name of this.words.querySelectorAll<HTMLElement>(".mw-machine")) {
        const named = name.dataset.name ?? "";
        name.classList.toggle("unlit", (!!lit && !lit.has(named)) || (!!found && !found.has(named)));
      }
      const busy = new Set(scene.nodes.filter((node) => node.online === true && node.busy > 0).map((node) => node.key));
      for (const branch of placement.branches) {
        if (!branch.machine || !busy.has(branch.machine)) continue;
        const [from, to] = [branch.points[0]!, branch.points[branch.points.length - 1]!];
        flows.push(`<path class="flow" d="M${from[0].toFixed(1)} ${from[1].toFixed(1)}L${to[0].toFixed(1)} ${to[1].toFixed(1)}"/>`);
      }
    }
    if (!this.marks.firstElementChild) this.marks.innerHTML = `<g class="clay" filter="url(#pen-marker)"></g><g class="moons"></g><g class="flows"></g>`;
    const [clayLayer, moon, flow] = [...this.marks.children] as SVGGElement[];
    keepRings(clayLayer!, circles);
    for (const [layer, html] of [
      [moon!, moons.join("")],
      [flow!, flows.join("")],
    ] as const) {
      if (layer.dataset.drawn === html) continue;
      layer.dataset.drawn = html;
      layer.innerHTML = html;
    }
  }
}

/** Machines' dots seen from afar inked alike: their colour, how strongly,
 *  whether larger, and where they stand. */
interface FarDots {
  colour: string;
  alpha: number;
  large: boolean;
  points: Point[];
}

/** A machine's ring drawn small: where, its ink, whether broken, and the
 *  marks round it. */
interface FarRing {
  key: string;
  point: Point;
  colour: string;
  alpha: number;
  broken: boolean;
  here: boolean;
  device: boolean;
  halo: boolean;
  busy: number;
  found: boolean;
}

/** A box's wash by whose it is: the account's own, an assisting agent's,
 *  or another person's. */
function toneOf(kind: string): string {
  const owner = boxParts(kind).owner;
  return owner === "mine" ? "mine" : owner.startsWith("assist:") ? "assist" : "others";
}

/** Whether `outer` holds all of `inner`. */
function holds(outer: Bounds, inner: Bounds): boolean {
  return inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.width <= outer.x + outer.width && inner.y + inner.height <= outer.y + outer.height;
}

/** Whether the line from `from` to `to` passes through `region`. */
function crosses(region: Bounds, from: Point, to: Point): boolean {
  const [left, right] = [Math.min(from[0], to[0]), Math.max(from[0], to[0])];
  const [top, bottom] = [Math.min(from[1], to[1]), Math.max(from[1], to[1])];
  return right >= region.x && left <= region.x + region.width && bottom >= region.y && top <= region.y + region.height;
}

/** A clay ring drawn live, by what it rings and why. */
interface Ring {
  id: string;
  kind: "chosen" | "arrived" | "lit" | "found";
  d: string;
}

/** The rings of `layer` made to be `rings`: one already there is only moved
 *  where it must be, so the pen does not draw it in again when something
 *  else on the map changes; one new is drawn in; one gone goes. */
function keepRings(layer: SVGGElement, rings: Ring[]): void {
  const shown = new Map([...layer.children].map((one) => [(one as SVGPathElement).dataset.ring!, one as SVGPathElement]));
  const wanted = new Set(rings.map((ring) => ring.id));
  for (const [id, element] of shown) if (!wanted.has(id)) element.remove();
  for (const ring of rings) {
    let element = shown.get(ring.id);
    if (!element) {
      element = document.createElementNS(SVG, "path");
      element.dataset.ring = ring.id;
      element.setAttribute("class", ring.kind);
      layer.append(element);
    }
    if (element.getAttribute("d") !== ring.d) element.setAttribute("d", ring.d);
  }
}

/** Lays `layer` over `frame`, a box of the drawing, at `scale` screen
 *  pixels to the drawing's unit: its user space is the drawing's. */
function frameLayer(layer: SVGSVGElement, frame: Bounds, scale: number): void {
  layer.setAttribute("viewBox", `${frame.x} ${frame.y} ${frame.width} ${frame.height}`);
  layer.setAttribute("width", String(frame.width * scale));
  layer.setAttribute("height", String(frame.height * scale));
  layer.style.left = `${frame.x * scale}px`;
  layer.style.top = `${frame.y * scale}px`;
}

/** A crescent moon of radius `r` round `centre`, opening to the right. */
function crescent(centre: Point, r: number): string {
  const [x, y] = centre;
  const tip = (sign: number) => `${(x + r * 0.35).toFixed(1)} ${(y + sign * r * 0.94).toFixed(1)}`;
  return `M${tip(-1)}A${r} ${r} 0 1 0 ${tip(1)}A${(r * 0.78).toFixed(1)} ${(r * 0.78).toFixed(1)} 0 1 1 ${tip(-1)}Z`;
}

/** The corners a box's strip is rounded at: all four for a strip alone or
 *  a group's band, the top two over a box's body. */
function cornersOf(group: Placement["groups"][number]): [number, number, number, number] {
  if (group.within) return [6, 6, 6, 6];
  return group.box.height <= group.strip ? [7, 7, 7, 7] : [7, 7, 0, 0];
}

/** Points round a rounded rectangle, for a box drawn as broken strokes. */
function stripPoints(x: number, y: number, width: number, height: number, r: number): Point[] {
  const points: Point[] = [];
  const corner = (cx: number, cy: number, from: number) => {
    for (let step = 0; step <= 4; step += 1) {
      const angle = from + (step / 4) * (Math.PI / 2);
      points.push([cx + Math.cos(angle) * r, cy + Math.sin(angle) * r]);
    }
  };
  corner(x + width - r, y + r, -Math.PI / 2);
  corner(x + width - r, y + height - r, 0);
  corner(x + r, y + height - r, Math.PI / 2);
  corner(x + r, y + r, Math.PI);
  points.push(points[0]!);
  return points;
}

/** The edge of a phone standing at `centre`, `width` by `height`, its
 *  corners rounded by `corner`: one loop from the middle of its top. */
function phoneLoop(centre: Point, width: number, height: number, corner: number): Point[] {
  const [x, y] = centre;
  const left = x - width / 2;
  const right = x + width / 2;
  const top = y - height / 2;
  const bottom = y + height / 2;
  const points: Point[] = [[x, top]];
  const arc = (cx: number, cy: number, from: number) => {
    for (let step = 0; step <= 4; step++) {
      const at = from + (step / 4) * (Math.PI / 2);
      points.push([cx + Math.cos(at) * corner, cy + Math.sin(at) * corner]);
    }
  };
  arc(right - corner, top + corner, -Math.PI / 2);
  arc(right - corner, bottom - corner, 0);
  arc(left + corner, bottom - corner, Math.PI / 2);
  arc(left + corner, top + corner, Math.PI);
  points.push([x + 1, top]);
  return points;
}

/** Points round a circle, for a ring drawn as broken strokes. */
function ringPoints(centre: Point, radius: number, count: number): Point[] {
  const points: Point[] = [];
  for (let index = 0; index <= count; index += 1) {
    const angle = (index / count) * Math.PI * 2;
    points.push([centre[0] + Math.cos(angle) * radius, centre[1] + Math.sin(angle) * radius]);
  }
  return points;
}

/** Text for SVG and HTML, its markup characters written as entities. */
function escapeText(text: string): string {
  return text.replace(/[&<>"]/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[character]!);
}
