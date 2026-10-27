// The machines page's drawing as a canvas (`world-gui.md`, 账号、机器、共享与
// 协助): the account's World drawn by hand on graph paper (`map-layout.ts`),
// moved and zoomed (`../map-canvas`). The hub at the top, square lines down
// to a box for each group of machines by whose they are, its name on a title
// strip, and the machines standing in a grid inside, names to the right.
// The names are written on the paper and grow and shrink with it. Nearer,
// the drawing shows what each machine is and the agents on it. The account's
// own end with a dashed ring that adds a machine. For the network's
// administrator every person has a box, a strip alone until a machine of
// theirs comes, with a "⋯" at its end and a small lock while they are
// locked, and a dashed box after them adds a person. A machine of another
// whose agent alone assists is a broken ring of clay; one out of reach is
// drawn with a dry pen. A machine others may connect to directly wears a
// dashed halo, in pencil until the hub has checked it: every machine that
// reaches it links with it, so the links are not drawn one by one; which
// are up is said in its details. What floats beside a ring chosen is the
// page's.

import { reducedMotion } from "../motion";
import { MapCanvas, PenLayer, type Bounds, type CanvasEvent, type CanvasFrame, type CanvasItem, type PenStroke, type Point } from "../map-canvas";
import { blot, circle, outline, ruled, seeded, ticks } from "../pen/shapes";
import type { MachinesWords } from "./account-words";
import { boxParts, groupMoreKey } from "./machine-groups";
import { KNOT, MapLayout, RING, ROWS, TRUNK, type Placement } from "./map-layout";
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
   *  columns as tall as fill the frame best, its names kept readable however
   *  small it is drawn. */
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
  /** The boxes folded to their title strip, by key. */
  folded: ReadonlySet<string>;
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
   *  in clay, the other names faint. */
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

/** Past this many machines, others' boxes start folded: the drawing fitted
 *  to its page is too small to read them all. */
const FOLD_PAST = 24;

/** Past this many machines in it, a group's band starts folded where the
 *  machines are many, as the list opens a group only this small. */
const OPEN_BAND = 12;

/** The boxes folded: others', when the machines are many, and then a group
 *  of more than a dozen inside its owner's box, unless the user opened or
 *  folded one, as `chosen` holds. */
export function foldedOf(nodes: readonly MapNode[], chosen: ReadonlyMap<string, boolean> = new Map()): Set<string> {
  const many = nodes.length > FOLD_PAST;
  // How many machines each group's band holds, on each hub.
  const held = new Map<string, number>();
  if (many) {
    for (const node of nodes) {
      const group = node.group?.key;
      if (!group || boxParts(group).group === null) continue;
      for (const hub of node.hubs) held.set(`group:${hub}:${group}`, (held.get(`group:${hub}:${group}`) ?? 0) + 1);
    }
  }
  const folded = new Set<string>();
  for (const node of nodes) {
    const group = node.group?.key ?? "mine";
    const owner = boxParts(group).owner;
    for (const hub of node.hubs) {
      const key = `group:${hub}:${owner}`;
      if (chosen.get(key) ?? (many && owner !== "mine")) folded.add(key);
      const band = `group:${hub}:${group}`;
      if (group !== owner && (chosen.get(band) ?? (held.get(band) ?? 0) > OPEN_BAND)) folded.add(band);
    }
  }
  return folded;
}

/** A still drawing's names are never drawn smaller than this share of their
 *  size, nor larger than twice it to get there, each box's title strip
 *  growing to hold its name; on a phone's narrow frame, where the drawing
 *  is smallest, they keep their whole size and grow as far as three times. */
const LEGIBLE = 0.85;
const MOST_GROWN = 2;
const LEGIBLE_NARROW = 1;
const MOST_GROWN_NARROW = 3;
const NARROW_FRAME = 480;

/** Names that would be drawn smaller than this share of their size, grown as
 *  far as they may, are not drawn at all: under ten pixels high. */
const BARE = 0.75;

/** The scale the drawing fits itself no smaller than: its machines' names
 *  about twelve pixels high. */
const READABLE = 0.8;

/** The side gutter a phone's drawing is fitted within, as the page's. */
const NARROW_PAD = 16;

/** Room round a hub's point for its name over it, in the drawing's units. */
const HUB_ROOM = 64;

/** The smallest box holding all of `boxes`. */
function enclosing(boxes: readonly Bounds[]): Bounds {
  const x = Math.min(...boxes.map((box) => box.x));
  const y = Math.min(...boxes.map((box) => box.y));
  const right = Math.max(...boxes.map((box) => box.x + box.width));
  const bottom = Math.max(...boxes.map((box) => box.y + box.height));
  return { x, y, width: right - x, height: bottom - y };
}

/** Zoom levels: below the first the drawing is far, above the second near. */
const LEVELS: [number, number] = [0.55, 1.2];
const SVG = "http://www.w3.org/2000/svg";

/** How long the drawing takes to glide to where it is brought. */
const GLIDE_MS = 240;

/** Room round the drawing in its layers, for what they draw beyond it: the
 *  names above a hub and those right of the rings. */
const LAYER_ROOM = 160;

export class MachineMap {
  readonly element: HTMLElement;
  private readonly surface: MapCanvas;
  private readonly pens: PenLayer;
  private readonly layout: MapLayout;
  /** Under the ink: the groups' boxes. Over it: the names, then what is
   *  marked live. All in the drawing's units, so they zoom with it. */
  private readonly boxes: SVGSVGElement;
  private readonly names: SVGSVGElement;
  private readonly marks: SVGSVGElement;
  private readonly themes: MutationObserver;
  private scene: MachineMapScene | null = null;
  private placement: Placement | null = null;
  /** What the placement was laid out from, to lay it again only when that changed. */
  private laidFrom: { from: readonly unknown[]; shape: string } | null = null;
  private level = -1;
  /** Whether the drawing is laid for a narrow screen, as of the last
   *  update. */
  private narrow = false;
  private composed = "";
  private marked = "";
  /** A still drawing's frame, as of its last layout. */
  private framed = "";
  /** How many times their size a still drawing's names are drawn, as it
   *  was laid for. */
  private grow = 1;
  /** Whether a still drawing is laid too small for its machines' names to
   *  be read however they grow, and leaves them out. */
  private bare = false;
  /** Measures a still drawing's names for the room they take. */
  private measuring: CanvasRenderingContext2D | null = null;
  /** The faces are loading, and the drawing is laid again when they are in. */
  private awaitingFaces = false;
  /** The boxes folded as of the last placing: a fold the person made keeps
   *  the drawing where it is. */
  private placedFolds: string | null = null;
  /** The box of the drawing the layers over and under the ink are laid
   *  over, and the scale they are drawn at. */
  private layerBox: Bounds | null = null;
  private layerScale = NaN;

  constructor(private readonly options: MachineMapOptions) {
    const { words } = options;
    this.layout = new MapLayout(options.storageKey);
    this.surface = new MapCanvas({
      label: words.map,
      controls: { minus: words.zoomOut, plus: words.zoomIn, fit: words.fit, scale: words.scale },
      hints: options.still ? undefined : { zoom: words.hintZoom, pan: words.hintPan, ...(options.afterHints ? { after: options.afterHints } : {}) },
      still: options.still ?? false,
      minScale: 1 / 16,
      maxScale: 4,
      fitFloor: READABLE,
      levels: LEVELS,
      validTarget: () => false,
      event: (event) => options.event(event),
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
    // washes lie under them, the names and the live marks over them.
    this.boxes = layer("map-boxes");
    this.surface.under.append(this.boxes);
    this.pens = new PenLayer(this.surface.ink, "texture");
    this.names = layer("map-names");
    this.marks = layer("map-live");
    this.surface.over.append(this.names, this.marks);
    // The ink's colours are resolved for the textures: drawn again when the
    // theme changes.
    this.themes = new MutationObserver(() => {
      this.composed = "";
      this.compose();
    });
    this.themes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class"] });
  }

  /** The machine, box or hub's place on the screen, in the canvas's
   *  pixels. */
  screenOf(key: string): Point | null {
    const point = this.pointOf(key);
    return point ? this.surface.camera.toScreen(point) : null;
  }

  /** How wide a machine's name is on the screen now. */
  nameWidth(key: string): number {
    const text = [...this.names.querySelectorAll<SVGTextElement>("text[data-name]")].find((one) => one.dataset.name === key);
    return text ? text.getBBox().width * this.surface.camera.view.scale : 0;
  }

  /** The screen box of a ring, for keeping names clear of what floats. */
  reserve(boxes: readonly Bounds[]): void {
    this.surface.reserveLabels(boxes);
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

  dispose(): void {
    this.themes.disconnect();
    this.pens.dispose();
    this.surface.dispose();
  }

  update(scene: MachineMapScene): void {
    this.scene = scene;
    // The faces as the theme names them now, read once this drawing.
    this.faces.clear();
    if (this.options.still) this.placement = this.fill(scene);
    else {
      this.narrow = this.isNarrow();
      // Laid out again only when what it is laid from changed: the page
      // draws itself every couple of seconds with the same machines.
      const from = [scene.hubs, scene.nodes, scene.links, scene.people ?? null] as const;
      const shape = JSON.stringify([[...scene.folded ?? []].sort(), [...scene.adding ?? []].sort(), this.narrow]);
      const laid = this.laidFrom;
      if (!this.placement || !laid || laid.shape !== shape || laid.from.some((one, at) => one !== from[at])) {
        this.placement = this.layout.update(scene.hubs, scene.nodes, { links: scene.links, folded: scene.folded, narrow: this.narrow, people: scene.people ?? [], adding: new Set(scene.adding ?? []), label: (text) => this.labelWidth(text) });
        this.laidFrom = { from, shape };
      }
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
    if (key.startsWith("add:")) return this.layout.vacant(key.slice(4));
    return placement.machines.get(key) ?? null;
  }

  /** A still drawing laid with its columns as tall as let it be drawn
   *  largest in its frame; ten to a column until the frame has a size. Its
   *  names grow back to stay readable as it is drawn small, and take the
   *  room they need: each way of laying it is laid again for the size its
   *  names will be, a few times over, as the two settle. Too small for its
   *  machines' names to be read even grown, as a phone's frame is for all
   *  but the smallest network, it leaves them out and its machines stand
   *  closer: the rings, the marks and the boxes' names alone. */
  private fill(scene: MachineMapScene): Placement {
    const width = this.element.clientWidth;
    const height = this.element.clientHeight;
    this.framed = `${width}x${height}`;
    const named = this.nameRoom(scene);
    const label = (text: string) => this.labelWidth(text);
    const lay = (rows: number, grow: number, room: number | undefined) =>
      this.layout.update(scene.hubs, scene.nodes, { links: scene.links, folded: scene.folded, rows, vacant: scene.adds.length > 0, grow, label, ...(room === undefined ? {} : { room }) });
    if (!width || !height) {
      this.grow = 1;
      this.bare = false;
      return lay(ROWS, 1, named);
    }
    // What it is laid out from, but who is online: a machine coming or
    // going is laid out as the best size was last found for these, once.
    const shape = JSON.stringify([width, height, named, document.fonts.status, scene.hubs.map((hub) => hub.id), scene.nodes.map((node) => [node.key, node.name, node.sub, node.group?.key ?? null, node.hubs]), scene.links.map((link) => [link.a, link.b]), [...scene.folded], scene.adds]);
    const found = this.found;
    if (found?.shape === shape) {
      this.grow = found.grow;
      this.bare = found.bare;
      return lay(found.rows, found.grow, found.room);
    }
    // The canvas leaves a tenth of the shorter side round what it fits.
    const pad = Math.max(24, Math.min(76, Math.min(width, height) * 0.1));
    const scaleOf = ({ bounds }: Placement) => Math.min((width - 2 * pad) / bounds.width, (height - 2 * pad) / bounds.height);
    const largest = (room: number | undefined) => {
      let best: { placement: Placement; scale: number; grow: number; rows: number; room: number | undefined } | null = null;
      for (let rows = 1; rows <= ROWS; rows += 1) {
        let grow = 1;
        let placement = lay(rows, grow, room);
        let scale = scaleOf(placement);
        for (let round = 0; round < 4; round += 1) {
          const next = Math.min(this.mostGrown(), Math.max(1, this.legible() / scale));
          if (Math.abs(next - grow) < 0.02) break;
          grow = next;
          placement = lay(rows, grow, room);
          scale = scaleOf(placement);
        }
        if (!best || scale > best.scale + 1e-6) best = { placement, scale, grow, rows, room };
      }
      return best!;
    };
    let best = largest(named);
    this.bare = best.scale * best.grow < BARE;
    if (this.bare) {
      // Its machines stand as close as they may, or further apart as far as
      // its frame has the room without drawing it smaller: the room its
      // boxes' names then have.
      const close = largest(0);
      best = close;
      for (const share of [0.5, 0.25]) {
        const wider = largest((named ?? 0) * share);
        if (wider.scale < close.scale * 0.98) continue;
        best = wider;
        break;
      }
    }
    this.grow = best.grow;
    this.found = { shape, rows: best.rows, grow: best.grow, room: best.room, bare: this.bare };
    return best.placement;
  }

  /** The best size last found for a still drawing, and what for. */
  private found: { shape: string; rows: number; grow: number; room: number | undefined; bare: boolean } | null = null;

  /** The least share of their size a still drawing's names are drawn at,
   *  and the most they grow to get there. */
  private legible(): number {
    return this.element.clientWidth < NARROW_FRAME ? LEGIBLE_NARROW : LEGIBLE;
  }

  private mostGrown(): number {
    return this.element.clientWidth < NARROW_FRAME ? MOST_GROWN_NARROW : MOST_GROWN;
  }

  /** What a box's strip says of whose it is: its label, or drawn bare,
   *  as much of it as its strip has the room for. */
  private labelOf(group: Placement["groups"][number]): string {
    if (!this.bare) return group.label;
    const { x, width } = group.box;
    return this.fitted(group.label, x + width - (this.moreOf(group) ? MORE : 10) - (x + TRUNK + 14));
  }

  /** A box's `label` if it fits `room` of the drawing's units at the size
   *  its name is laid for, or as much as does cut short with an ellipsis
   *  while three letters are left; otherwise nothing. */
  private fitted(label: string, room: number): string {
    if (this.measured("--font-emphasis", 14 * this.grow, label) <= room) return label;
    const context = this.measuring;
    if (!context) return label;
    context.font = `${(14 * this.grow).toFixed(1)}px ${this.face("--font-emphasis")}`;
    const letters = [...label];
    while (letters.length && context.measureText(`${letters.join("").trimEnd()}…`).width > room) letters.pop();
    const kept = letters.join("").trimEnd();
    return [...kept].length >= 3 ? `${kept}…` : "";
  }

  /** How wide the widest of the machines' names is, or the line under
   *  one, at their size and with a little air: the room a name takes beside
   *  its ring in a still drawing, which lays it no wider than its own names
   *  need. */
  /** How wide a box's name is written on its strip, at its own size. */
  private labelWidth(text: string): number {
    return this.measured("--font-emphasis", 14, text);
  }

  /** How wide `text` is at `size` pixels in the face `face` names, measured
   *  once for each face, size and text: the drawing lays itself out a few
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

  private nameRoom(scene: MachineMapScene): number | undefined {
    if (!scene.nodes.length) return undefined;
    let widest = 0;
    for (const node of scene.nodes) {
      widest = Math.max(widest, this.measured("--font-reading", 13, node.name));
      if (node.sub) widest = Math.max(widest, this.measured("--font-reading", 10.5, node.sub));
    }
    // A face not yet loaded measures as another: laid again once the faces
    // are in, and a little over meanwhile.
    if (document.fonts.status !== "loaded" && !this.awaitingFaces) {
      this.awaitingFaces = true;
      void document.fonts.ready.then(() => {
        this.awaitingFaces = false;
        // The names measure anew: laid out again whatever else stayed.
        this.laidFrom = null;
        if (this.scene) this.update(this.scene);
      });
    }
    return Math.ceil(widest * 1.15) + 16;
  }

  /** A phone's screen, or a page as narrow: under 600 pixels across. */
  private isNarrow(): boolean {
    const width = this.element.clientWidth;
    return width > 0 && width < 600;
  }

  /** Draws the layers over and under the ink at `scale`. */
  private frameLayers(scale: number): void {
    this.layerScale = scale;
    if (!this.layerBox) return;
    for (const layer of [this.boxes, this.names, this.marks]) frameLayer(layer, this.layerBox, scale);
  }

  private onFrame(frame: CanvasFrame): void {
    this.pens.view(frame);
    if (frame.view.scale !== this.layerScale) this.frameLayers(frame.view.scale);
    // Laid again when the page becomes narrow or wide, or a still drawing's
    // frame changes shape.
    if (this.options.still) {
      if (this.scene && `${this.element.clientWidth}x${this.element.clientHeight}` !== this.framed) this.update(this.scene);
      // Drawn small, its names grow back to stay readable, as large as it
      // was laid for.
      this.element.style.setProperty("--name-grow", Math.min(this.grow, Math.max(1, this.legible() / frame.view.scale)).toFixed(3));
    } else if (this.scene && this.isNarrow() !== this.narrow) this.update(this.scene);
    if (frame.view.level !== this.level) {
      this.level = frame.view.level;
      this.compose();
      this.place();
      this.mark();
    }
    this.options.frame?.(frame);
  }

  /** The hit areas: every hub, box title, dashed ring and machine. Their
   *  names are written on the paper; the canvas keeps them for those who
   *  hear the drawing. */
  private place(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const { words } = this.options;
    const items: CanvasItem[] = [
      ...scene.hubs.map((hub): CanvasItem => ({ key: `hub:${hub.id}`, kind: "hub", point: placement.hubs.get(hub.id)!, radius: KNOT + 6, label: hub.name, sub: scene.hubLabels.get(hub.id) ?? "" })),
      // A person's or a group's "⋯", over the end of its box's strip:
      // listed before the strips, since a press goes to the first that
      // holds it.
      ...placement.groups.flatMap((group): CanvasItem[] => {
        const more = this.moreOf(group);
        if (!more) return [];
        const { x, y, width } = group.box;
        const corners = cornersOf(group);
        return [
          {
            key: more.key,
            kind: "group",
            point: [x + width - MORE / 2, y + group.strip / 2],
            radius: group.strip / 2,
            box: { x: x + width - MORE, y, width: MORE, height: group.strip, corners: [0, corners[1], corners[2], 0] },
            label: more.label,
          },
        ];
      }),
      // A box is pressed anywhere else on its title strip, a group on its
      // band, where it is drawn.
      ...placement.groups.filter((group) => !(this.bare && group.within && !this.labelOf(group))).map((group): CanvasItem => ({
        key: group.key,
        kind: "group",
        point: group.point,
        radius: group.strip / 2 + 4,
        box: { x: group.box.x, y: group.box.y, width: group.box.width, height: group.strip, corners: cornersOf(group) },
        label: group.kind === "adding" ? (this.options.people?.addPerson ?? "") : group.label,
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
    // Narrow, the drawing is fitted to the width and read from the top,
    // moving down for the rest; never drawn larger than its own size, its
    // names as large as a phone writes them.
    const view = this.surface.camera.view;
    // The page's head lies over the top of a phone's drawing: room above.
    const top = { ...placement.bounds, y: placement.bounds.y - 40, height: placement.bounds.height + 40 };
    const wide = { ...top, width: Math.max(top.width, view.width - 2 * NARROW_PAD) };
    const fitted = this.narrow && view.width > 0 ? { ...wide, height: Math.min(wide.height, (wide.width * view.height) / view.width) } : placement.bounds;
    // Folding or opening a box moves nothing else on the screen.
    const folds = [...scene.folded].sort().join("|");
    const folding = this.placedFolds !== null && folds !== this.placedFolds;
    this.placedFolds = folds;
    // Too large to read whole, the drawing shows its hubs and the account's
    // own machines first; the rest is a drag away.
    const firsts = [
      ...[...placement.hubs.values()].map(([x, y]): Bounds => ({ x: x - HUB_ROOM, y: y - HUB_ROOM, width: HUB_ROOM * 2, height: HUB_ROOM * 2 })),
      ...placement.groups.filter((group) => group.kind === "mine").map((group) => group.box),
    ];
    const focus = this.narrow || !firsts.length ? null : enclosing(firsts);
    this.surface.setScene(items, [], fitted, folding, focus);
    const selected = scene.selected && items.some((item) => item.key === scene.selected) ? scene.selected : null;
    if (this.surface.selection !== selected) this.surface.setSelection(selected);
  }

  /** What a box's strip says after its name: how many machines and how
   *  many online, or that a person has none yet. */
  /** The "⋯" at the end of a box's strip: a person's, for the network's
   *  administrator, or a group's the user may rename or end. */
  private moreOf(group: Placement["groups"][number]): { key: string; label: string } | null {
    if (group.person && this.options.people) return { key: personKey(group.hub, group.person.account), label: this.options.people.more(group.person.account) };
    const name = boxParts(group.kind).group;
    if (name !== null && this.scene?.grouped?.has(group.key)) return { key: groupMoreKey(group.hub, group.kind), label: this.options.words.groups.more(name) };
    return null;
  }

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
    // A still drawing is a picture of what is afoot: its marks are drawn
    // however small it is, as large as its names.
    const near = this.level >= 1 || !!this.options.still;
    const marks = this.options.still ? Math.min(1.6, this.grow) : 1;
    const key = JSON.stringify([
      near,
      marks,
      this.bare,
      this.grow,
      scene.hubs,
      [...scene.hubLabels],
      scene.adds,
      [...scene.waiting],
      scene.nodes.map((node) => [node.key, node.name, node.sub, node.online, node.hold, node.reaching, node.here, near ? node.busy : null, node.directAt, node.through]),
      [...scene.opening],
      [...placement.machines],
      placement.groups.map((group) => [group.key, group.label, group.folded, group.box, group.count, group.online, group.person?.locked, group.person?.visitor, group.person?.promotion, this.moreOf(group)?.key]),
    ]);
    if (key === this.composed) return;
    this.composed = key;
    const css = getComputedStyle(document.documentElement);
    const graphite = css.getPropertyValue("--pen-graphite").trim() || css.getPropertyValue("--text").trim();
    const clay = css.getPropertyValue("--pen-clay").trim() || css.getPropertyValue("--accent").trim();
    const strokes: PenStroke[] = [];
    const add = (d: string, bounds: Bounds, pen: PenStroke["pen"], colour: string, opacity = 1) => strokes.push({ d, bounds, pen, colour, opacity });
    const around = (point: Point, reach: number): Bounds => ({ x: point[0] - reach, y: point[1] - reach, width: reach * 2, height: reach * 2 });
    const spanning = (points: Point[], pad: number): Bounds => {
      const xs = points.map((p) => p[0]);
      const ys = points.map((p) => p[1]);
      return { x: Math.min(...xs) - pad, y: Math.min(...ys) - pad, width: Math.max(...xs) - Math.min(...xs) + pad * 2, height: Math.max(...ys) - Math.min(...ys) + pad * 2 };
    };

    // The lines: the hub's down to the boxes, each column's trunk and its
    // twigs, ruled and square at the corners; a twig to a machine out of
    // reach, broken.
    for (const branch of placement.branches) {
      const [from, to] = [branch.points[0]!, branch.points[branch.points.length - 1]!];
      if (Math.hypot(to[0] - from[0], to[1] - from[1]) < 1) continue;
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

    // A folded box: its line ends in a blot on the title strip, for the
    // machines inside.
    for (const group of placement.groups) {
      if (!group.folded) continue;
      const at: Point = [group.box.x + TRUNK, group.box.y + group.strip / 2];
      add(blot(at, 4.2, seeded(`folded:${group.key}`)), around(at, 6), "marker", graphite, 0.85);
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
      if (!at) continue;
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
      if (!point) continue;
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
      if (!near) continue;
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
        const spot: Point = [x + Math.cos(at) * (RING + 9 + 4 * marks), y + Math.sin(at) * (RING + 9 + 4 * marks)];
        add(blot(spot, 4 * marks, seeded(`map-agent:${node.key}:${place}`)), around(spot, 3 + 4 * marks), "marker", clay);
      }
    }
    void this.pens.update(strokes).catch((error: unknown) => console.warn("the machines drawing could not be inked", error));

    // Each layer is as large as the drawing and the room round it, drawn to
    // the camera's scale by its own view box (`MapCanvas.over`).
    const { bounds } = placement;
    this.layerBox = { x: bounds.x - LAYER_ROOM, y: bounds.y - LAYER_ROOM, width: bounds.width + 2 * LAYER_ROOM, height: bounds.height + 2 * LAYER_ROOM };
    this.frameLayers(this.surface.camera.view.scale);

    // The boxes: a wash for the body and a deeper one for the title strip,
    // no line round either; folded, the strip alone.
    this.boxes.innerHTML = placement.groups
      .map((group) => {
        const { x, y, width, height } = group.box;
        // The dashed box that adds a person is its pen strokes alone.
        if (group.kind === "adding") return "";
        const owner = boxParts(group.kind).owner;
        const tone = owner === "mine" ? "mine" : owner.startsWith("assist:") ? "assist" : "others";
        const locked = group.person?.locked ? " locked" : "";
        const visitor = group.person?.visitor ? " visitor" : "";
        // A group's band lies in its owner's box over the group's machines;
        // drawn bare with no room for its name, not at all.
        if (group.within) return this.bare && !this.labelOf(group) ? "" : `<g class="box ${tone}"><rect class="box-band" x="${x}" y="${y}" width="${width}" height="${group.strip}" rx="6"/></g>`;
        const title = group.strip;
        const strip = height <= title
          ? `<rect class="box-title" x="${x}" y="${y}" width="${width}" height="${title}" rx="7"/>`
          : `<rect class="box-body" x="${x}" y="${y}" width="${width}" height="${height}" rx="7"/><path class="box-title" d="M${x} ${y + title}V${y + 7}Q${x} ${y} ${x + 7} ${y}H${x + width - 7}Q${x + width} ${y} ${x + width} ${y + 7}V${y + title}Z"/>`;
        return `<g class="box ${tone}${locked}${visitor}">${strip}</g>`;
      })
      .join("");

    // The names, on the paper: the hub's above it, each box's on its strip,
    // each machine's to the right of its ring with what it is under it.
    const text: string[] = [];
    for (const hub of scene.hubs) {
      const [x, y] = placement.hubs.get(hub.id)!;
      const said = scene.hubLabels.get(hub.id) ?? "";
      text.push(`<text class="hub-name" x="${x}" y="${y - 22}"${said ? ` dy="-1.15em"` : ""} text-anchor="middle">${escapeText(hub.name)}</text>`);
      if (said) text.push(`<text class="hub-sub" x="${x}" y="${y - 22}" text-anchor="middle">${escapeText(said)}</text>`);
    }
    // A box's strip: an arrow that says it opens or folds (a still picture
    // only leads to the machines page), then its name and how many.
    const still = this.options.still ?? false;
    for (const group of placement.groups) {
      const { x, y, width } = group.box;
      const middle = y + group.strip / 2;
      const arrowAt = x + TRUNK + 20;
      const band = group.within ? " band" : "";
      if (group.kind === "adding") {
        text.push(`<text class="box-name adding" x="${x + TRUNK + 28}" y="${middle + 5}">${escapeText(this.options.people?.addPerson ?? "")}</text>`);
        continue;
      }
      // Only a box with machines in it folds.
      const folds = !still && group.count > 0;
      if (folds) {
        const d = group.folded ? `M${arrowAt - 2} ${middle - 4}L${arrowAt + 2} ${middle}L${arrowAt - 2} ${middle + 4}` : `M${arrowAt - 4} ${middle - 2}L${arrowAt} ${middle + 2}L${arrowAt + 4} ${middle - 2}`;
        text.push(`<path class="box-arrow" d="${d}"/>`);
      }
      const locked = group.person?.locked ? " locked" : "";
      // A visitor's box says so, and so does one asked to become a member.
      const people = this.options.people;
      const role = !people ? "" : group.person?.promotion ? people.waitingTag : group.person?.visitor ? people.visitorTag : "";
      const nameX = folds ? arrowAt + 12 : x + TRUNK + 14;
      // Drawn bare, a box says whose it is and no more, in the room it has;
      // its rings say how many and which are online.
      const label = this.labelOf(group);
      const after = this.bare ? "" : `${role ? `<tspan class="box-role" dx="8">${escapeText(role)}</tspan>` : ""}<tspan class="box-count" dx="10">${escapeText(this.countOf(group))}</tspan>`;
      if (label) text.push(`<text class="box-name${locked}${band}" x="${nameX}" y="${middle + 5}"><tspan>${escapeText(label)}</tspan>${after}</text>`);
      if (this.moreOf(group)) {
        // A small lock while the person is locked, then the "⋯".
        const end = x + width - MORE / 2;
        if (group.person?.locked) text.push(`<path class="box-lock" d="${lockPath(end - 26, middle)}"/>`);
        text.push(`<g class="box-more">${[-6, 0, 6].map((dx) => `<circle cx="${end + dx}" cy="${middle}" r="1.6"/>`).join("")}</g>`);
      }
    }
    const nameAt = (key: string, point: Point, name: string, sub: string, quiet = false) => {
      const x = point[0] + RING + 15;
      text.push(
        sub
          ? `<text class="machine-name${quiet ? " quiet" : ""}" data-name="${escapeText(key)}" x="${x}" y="${point[1] - 2}">${escapeText(name)}<tspan class="machine-sub" x="${x}" dy="1.43em">${escapeText(sub)}</tspan></text>`
          : `<text class="machine-name${quiet ? " quiet" : ""}" data-name="${escapeText(key)}" x="${x}" y="${point[1] + 5}">${escapeText(name)}</text>`,
      );
    };
    for (const hub of this.bare ? [] : scene.adds) {
      const at = this.pointOf(addKey(hub));
      if (at) nameAt(addKey(hub), at, scene.waiting.has(hub) ? words.waitingJoin : words.addMachine, "", true);
    }
    for (const node of this.bare ? [] : scene.nodes) {
      const point = placement.machines.get(node.key);
      if (!point) continue;
      const through = node.through ? words.viaShort(node.through) : "";
      nameAt(node.key, point, node.name, [node.sub, through].filter(Boolean).join(" "), node.online === false || node.hold === "visitor");
    }
    this.names.innerHTML = text.join("");
  }

  /** Drawn live over the ink: the clay circle round the one chosen and the
   *  ones framed, the circles round machines just come, and strokes moving
   *  along the twig of a machine something works on. */
  private mark(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const near = this.level >= 1 || !!this.options.still;
    const lit = scene.lit ?? null;
    const found = scene.found ?? null;
    const key = JSON.stringify([scene.selected, scene.framed, [...scene.fresh], near, scene.nodes.map((node) => [node.key, node.online, node.busy > 0]), [...placement.machines], lit && [...lit], found && [...found]]);
    if (key === this.marked) return;
    this.marked = key;
    const circles: Ring[] = [];
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
    const moons: string[] = [];
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
    for (const name of this.names.querySelectorAll<SVGTextElement>(".machine-name")) {
      const named = name.dataset.name ?? "";
      name.classList.toggle("unlit", (!!lit && !lit.has(named)) || (!!found && !found.has(named)));
    }
    const flows: string[] = [];
    if (near) {
      for (const branch of placement.branches) {
        const node = branch.machine ? scene.nodes.find((one) => one.key === branch.machine) : undefined;
        if (!node || node.online !== true || node.busy === 0) continue;
        const [from, to] = [branch.points[0]!, branch.points[branch.points.length - 1]!];
        flows.push(`<path class="flow" d="M${from[0].toFixed(1)} ${from[1].toFixed(1)}L${to[0].toFixed(1)} ${to[1].toFixed(1)}"/>`);
      }
    }
    if (!this.marks.firstElementChild) this.marks.innerHTML = `<g class="clay" filter="url(#pen-marker)"></g><g class="moons"></g><g class="flows"></g>`;
    const [clay, moon, flow] = [...this.marks.children] as SVGGElement[];
    keepRings(clay!, circles);
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

/** A small padlock centred on `x`, `y`. */
function lockPath(x: number, y: number): string {
  return `M${x - 4.5} ${y - 0.5}h9v6.5h-9zM${x - 2.6} ${y - 0.5}v-2.4a2.6 2.6 0 0 1 5.2 0v2.4`;
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

/** Points round a circle, for a ring drawn as broken strokes. */
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

function ringPoints(centre: Point, radius: number, count: number): Point[] {
  const points: Point[] = [];
  for (let index = 0; index <= count; index += 1) {
    const angle = (index / count) * Math.PI * 2;
    points.push([centre[0] + Math.cos(angle) * radius, centre[1] + Math.sin(angle) * radius]);
  }
  return points;
}

/** Text for SVG, its markup characters written as entities. */
function escapeText(text: string): string {
  return text.replace(/[&<>"]/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[character]!);
}
