// The machines page's drawing as a canvas (`world-gui.md`, 账号、机器、共享与
// 借出): the account's World drawn by hand on graph paper (`map-layout.ts`),
// moved and zoomed (`../map-canvas`). The hub at the top, square lines down
// to a box for each group of machines by whose they are, its name on a title
// strip, and the machines standing in a grid inside, names to the right.
// The names are written on the paper and grow and shrink with it. Nearer,
// the drawing shows what each machine is and the agents on it. The account's
// own end with a dashed ring that adds a machine. A machine of another
// whose agent alone is lent is a broken ring of clay; one out of reach is
// drawn with a dry pen. A machine others may connect to directly wears a
// dashed halo, in pencil until the hub has checked it: every machine that
// reaches it links with it, so the links are not drawn one by one; which
// are up is said in its details. What floats beside a ring chosen is the
// page's.

import { reducedMotion } from "../motion";
import { MapCanvas, PenLayer, type Bounds, type CanvasEvent, type CanvasFrame, type CanvasItem, type PenStroke, type Point } from "../map-canvas";
import { blot, circle, outline, ruled, seeded, ticks } from "../pen/shapes";
import type { MachinesWords } from "./account-words";
import { KNOT, MapLayout, RING, ROWS, TITLE, TRUNK, type Placement } from "./map-layout";
import type { MapHub, MapLink, MapNode } from "./map-nodes";

export interface MachineMapOptions {
  words: MachinesWords;
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
}

/** Where this device remembers the order of the machines on the drawing,
 *  the same on every page that draws it. */
export const MAP_ORDER = "machines-map";

/** The key of a hub's dashed ring that adds a machine. */
export const addKey = (hub: string) => `add:${hub}`;

/** Past this many machines, others' boxes start folded: the drawing fitted
 *  to its page is too small to read them all. */
const FOLD_PAST = 24;

/** The boxes folded: others', when the machines are many, unless the user
 *  opened or folded one, as `chosen` holds. */
export function foldedOf(nodes: readonly MapNode[], chosen: ReadonlyMap<string, boolean> = new Map()): Set<string> {
  const many = nodes.length > FOLD_PAST;
  const folded = new Set<string>();
  for (const node of nodes) {
    const group = node.group?.key ?? "mine";
    for (const hub of node.hubs) {
      const key = `group:${hub}:${group}`;
      if (chosen.get(key) ?? (many && group !== "mine")) folded.add(key);
    }
  }
  return folded;
}

/** A still drawing's names are never drawn smaller than this share of their
 *  size. */
const LEGIBLE = 0.85;

/** Zoom levels: below the first the drawing is far, above the second near. */
const LEVELS: [number, number] = [0.55, 1.2];
const SVG = "http://www.w3.org/2000/svg";

/** How long the drawing takes to glide to where it is brought. */
const GLIDE_MS = 240;

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
  private level = -1;
  /** Whether the drawing is laid for a narrow screen, as of the last
   *  update. */
  private narrow = false;
  private composed = "";
  private marked = "";
  /** A still drawing's frame, as of its last layout. */
  private framed = "";
  /** The boxes folded as of the last placing: a fold the person made keeps
   *  the drawing where it is. */
  private placedFolds: string | null = null;

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
    this.boxes = layer("map-boxes");
    this.surface.ink.append(this.boxes);
    this.pens = new PenLayer(this.surface.ink, "texture");
    this.names = layer("map-names");
    this.marks = layer("map-live");
    this.surface.ink.append(this.names, this.marks);
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
    if (this.options.still) this.placement = this.fill(scene);
    else {
      this.narrow = this.isNarrow();
      this.placement = this.layout.update(scene.hubs, scene.nodes, { links: scene.links, folded: scene.folded, narrow: this.narrow });
    }
    this.compose();
    this.place();
    this.mark();
  }

  private pointOf(key: string): Point | null {
    const placement = this.placement;
    if (!placement) return null;
    if (key.startsWith("hub:")) return placement.hubs.get(key.slice(4)) ?? null;
    if (key.startsWith("group:")) return placement.groups.find((group) => group.key === key)?.point ?? null;
    // A hub's dashed ring waits where its next machine will stand.
    if (key.startsWith("add:")) return this.layout.vacant(key.slice(4));
    return placement.machines.get(key) ?? null;
  }

  /** A still drawing laid with its columns as tall as let it be drawn
   *  largest in its frame; ten to a column until the frame has a size. */
  private fill(scene: MachineMapScene): Placement {
    const width = this.element.clientWidth;
    const height = this.element.clientHeight;
    this.framed = `${width}x${height}`;
    const lay = (rows: number) => this.layout.update(scene.hubs, scene.nodes, { links: scene.links, folded: scene.folded, rows, vacant: scene.adds.length > 0 });
    if (!width || !height) return lay(ROWS);
    // The canvas leaves a tenth of the shorter side round what it fits.
    const pad = Math.max(24, Math.min(76, Math.min(width, height) * 0.1));
    let best: { placement: Placement; scale: number } | null = null;
    for (let rows = 1; rows <= ROWS; rows += 1) {
      const placement = lay(rows);
      const { bounds } = placement;
      const scale = Math.min((width - 2 * pad) / bounds.width, (height - 2 * pad) / bounds.height);
      if (!best || scale > best.scale + 1e-6) best = { placement, scale };
    }
    return best!.placement;
  }

  /** A phone's screen, or a page as narrow: under 600 pixels across. */
  private isNarrow(): boolean {
    const width = this.element.clientWidth;
    return width > 0 && width < 600;
  }

  private onFrame(frame: CanvasFrame): void {
    this.pens.view(frame);
    // Laid again when the page becomes narrow or wide, or a still drawing's
    // frame changes shape.
    if (this.options.still) {
      if (this.scene && `${this.element.clientWidth}x${this.element.clientHeight}` !== this.framed) this.update(this.scene);
      // Drawn small, its names grow back to stay readable.
      this.element.style.setProperty("--name-grow", Math.max(1, LEGIBLE / frame.view.scale).toFixed(3));
    } else if (this.scene && this.isNarrow() !== this.narrow) this.update(this.scene);
    // The graph paper moves with the drawing, a line every 32 of its units.
    this.element.style.setProperty("--paper-step", `${(32 * frame.view.scale).toFixed(2)}px`);
    this.element.style.setProperty("--paper-x", `${frame.view.x.toFixed(1)}px`);
    this.element.style.setProperty("--paper-y", `${frame.view.y.toFixed(1)}px`);
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
      // A box is pressed anywhere on its title strip.
      ...placement.groups.map((group): CanvasItem => ({
        key: group.key,
        kind: "group",
        point: group.point,
        radius: TITLE / 2 + 4,
        box: { x: group.box.x, y: group.box.y, width: group.box.width, height: TITLE, corners: group.folded ? [7, 7, 7, 7] : [7, 7, 0, 0] },
        label: group.label,
        sub: words.groupSub(group.count, group.online),
      })),
      ...scene.adds.flatMap((hub): CanvasItem[] => {
        const point = this.pointOf(addKey(hub));
        return point ? [{ key: addKey(hub), kind: "machine", point, radius: RING, label: scene.waiting.has(hub) ? words.waitingJoin : words.addMachine }] : [];
      }),
      ...scene.nodes.flatMap((node): CanvasItem[] => {
        const point = placement.machines.get(node.key);
        return point ? [{ key: node.key, kind: "machine", point, radius: RING, label: node.name, sub: node.sub }] : [];
      }),
    ];
    // Narrow, the drawing is fitted to the width and read from the top,
    // moving down for the rest.
    const view = this.surface.camera.view;
    // The page's head lies over the top of a phone's drawing: room above.
    const top = { ...placement.bounds, y: placement.bounds.y - 40, height: placement.bounds.height + 40 };
    const fitted = this.narrow && view.width > 0 ? { ...top, height: Math.min(top.height, (top.width * view.height) / view.width) } : placement.bounds;
    // Folding or opening a box moves nothing else on the screen.
    const folds = [...scene.folded].sort().join("|");
    const folding = this.placedFolds !== null && folds !== this.placedFolds;
    this.placedFolds = folds;
    this.surface.setScene(items, [], fitted, folding);
    const selected = scene.selected && items.some((item) => item.key === scene.selected) ? scene.selected : null;
    if (this.surface.selection !== selected) this.surface.setSelection(selected);
  }

  /** The ink, the boxes and the names, composed again only when what they
   *  show changes. */
  private compose(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const { words } = this.options;
    const near = this.level >= 1;
    const key = JSON.stringify([
      near,
      scene.hubs,
      [...scene.hubLabels],
      scene.adds,
      [...scene.waiting],
      scene.nodes.map((node) => [node.key, node.name, node.sub, node.online, node.hold, node.reaching, node.here, near ? node.marks : null, node.directAt, node.through]),
      [...scene.opening],
      [...placement.machines],
      placement.groups.map((group) => [group.key, group.folded, group.box, group.count, group.online]),
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
      const at: Point = [group.box.x + TRUNK, group.box.y + TITLE / 2];
      add(blot(at, 4.2, seeded(`folded:${group.key}`)), around(at, 6), "marker", graphite, 0.85);
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
      // works on, clay and broken where an agent alone is lent.
      if (node.hold === "lent") {
        for (const d of ticks(ringPoints(point, RING, 64), 9, 5.5, 2, random)) add(d, around(point, RING + 4), online ? "marker" : "dry", online ? clay : graphite, online ? 1 : 0.5);
      } else if (online) {
        add(circle(point, RING, 2, random), around(point, RING + 4), "marker", graphite, 0.9);
        add(circle(point, RING + 1.2, 1, random), around(point, RING + 4), "marker", graphite, 0.45);
      } else {
        add(circle(point, RING, 2.1, random), around(point, RING + 4), "dry", graphite, 0.5);
      }
      if (node.here) add(blot(point, 3, random), around(point, 5), "marker", graphite, 0.9);
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
      // The agents on it, round the upper right of its ring, clear of its
      // name on its right.
      node.marks.forEach((mark, place) => {
        const at = -1.15 + place * 0.36;
        const spot: Point = [x + Math.cos(at) * (RING + 13), y + Math.sin(at) * (RING + 13)];
        const own = seeded(`map-agent:${node.key}:${place}`);
        if (mark === "waiting") add(circle(spot, 4.6, 1.6, own), around(spot, 8), "marker", clay);
        else if (mark === "busy") add(blot(spot, 4, own), around(spot, 7), "marker", clay);
        else add(circle(spot, 3.2, 1.1, own), around(spot, 6), "marker", graphite, 0.9);
      });
    }
    void this.pens.update(strokes).catch((error: unknown) => console.warn("the machines drawing could not be inked", error));

    // The boxes: a wash for the body and a deeper one for the title strip,
    // no line round either; folded, the strip alone.
    this.boxes.innerHTML = placement.groups
      .map((group) => {
        const { x, y, width, height } = group.box;
        const tone = group.kind === "mine" ? "mine" : group.kind.startsWith("lent:") ? "lent" : "others";
        const strip = group.folded
          ? `<rect class="box-title" x="${x}" y="${y}" width="${width}" height="${TITLE}" rx="7"/>`
          : `<rect class="box-body" x="${x}" y="${y}" width="${width}" height="${height}" rx="7"/><path class="box-title" d="M${x} ${y + TITLE}V${y + 7}Q${x} ${y} ${x + 7} ${y}H${x + width - 7}Q${x + width} ${y} ${x + width} ${y + 7}V${y + TITLE}Z"/>`;
        return `<g class="box ${tone}">${strip}</g>`;
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
      const { x, y } = group.box;
      const middle = y + TITLE / 2;
      const arrowAt = x + TRUNK + 20;
      if (!still) {
        const d = group.folded ? `M${arrowAt - 2} ${middle - 4}L${arrowAt + 2} ${middle}L${arrowAt - 2} ${middle + 4}` : `M${arrowAt - 4} ${middle - 2}L${arrowAt} ${middle + 2}L${arrowAt + 4} ${middle - 2}`;
        text.push(`<path class="box-arrow" d="${d}"/>`);
      }
      text.push(`<text class="box-name" x="${still ? x + TRUNK + 14 : arrowAt + 12}" y="${middle + 5}"><tspan>${escapeText(group.label)}</tspan><tspan class="box-count" dx="10">${escapeText(words.groupSub(group.count, group.online))}</tspan></text>`);
    }
    const nameAt = (key: string, point: Point, name: string, sub: string, quiet = false) => {
      const x = point[0] + RING + 15;
      text.push(
        sub
          ? `<text class="machine-name${quiet ? " quiet" : ""}" data-name="${escapeText(key)}" x="${x}" y="${point[1] - 2}">${escapeText(name)}<tspan class="machine-sub" x="${x}" dy="1.43em">${escapeText(sub)}</tspan></text>`
          : `<text class="machine-name${quiet ? " quiet" : ""}" data-name="${escapeText(key)}" x="${x}" y="${point[1] + 5}">${escapeText(name)}</text>`,
      );
    };
    for (const hub of scene.adds) {
      const at = this.pointOf(addKey(hub));
      if (at) nameAt(addKey(hub), at, scene.waiting.has(hub) ? words.waitingJoin : words.addMachine, "", true);
    }
    for (const node of scene.nodes) {
      const point = placement.machines.get(node.key);
      if (!point) continue;
      const through = node.through ? words.viaShort(node.through) : "";
      nameAt(node.key, point, node.name, [node.sub, through].filter(Boolean).join(" · "), node.online === false);
    }
    this.names.innerHTML = text.join("");
  }

  /** Drawn live over the ink: the clay circle round the one chosen and the
   *  ones framed, the circles round machines just come, and strokes moving
   *  along the twig of a machine something works on. */
  private mark(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const near = this.level >= 1;
    const lit = scene.lit ?? null;
    const key = JSON.stringify([scene.selected, scene.framed, [...scene.fresh], near, scene.nodes.map((node) => [node.key, node.online, node.marks.includes("busy")]), [...placement.machines], lit && [...lit]]);
    if (key === this.marked) return;
    this.marked = key;
    const circles: string[] = [];
    const chosen = scene.selected && !scene.selected.startsWith("group:") ? this.pointOf(scene.selected) : null;
    if (chosen) circles.push(`<path class="chosen" d="${circle(chosen, scene.selected!.startsWith("hub:") ? KNOT + 9 : RING + 8, 3, seeded(`chosen:${scene.selected}`))}"/>`);
    for (const framed of scene.framed) {
      const point = this.pointOf(framed);
      if (point && framed !== scene.selected) circles.push(`<path class="chosen" d="${circle(point, RING + 8, 3, seeded(`chosen:${framed}`))}"/>`);
    }
    for (const fresh of scene.fresh) {
      const point = this.pointOf(fresh);
      if (point && fresh !== scene.selected) circles.push(`<path class="arrived" d="${circle(point, RING + 8, 3, seeded(`arrived:${fresh}`))}"/>`);
    }
    // The machines that can take the server glow with a broken clay ring,
    // a small moon beside one that goes to sleep.
    const moons: string[] = [];
    for (const [key, candidate] of lit ?? []) {
      const point = this.pointOf(key);
      if (!point) continue;
      circles.push(`<path class="lit" d="${circle(point, RING + 8, 3, seeded(`lit:${key}`))}"/>`);
      if (candidate.sleeps) moons.push(`<path class="moon" d="${crescent([point[0] - (RING + 18) * 0.72, point[1] - (RING + 18) * 0.72], 7)}"/>`);
    }
    for (const name of this.names.querySelectorAll<SVGTextElement>(".machine-name")) {
      name.classList.toggle("unlit", !!lit && !lit.has(name.dataset.name ?? ""));
    }
    const flows: string[] = [];
    if (near) {
      for (const branch of placement.branches) {
        const node = branch.machine ? scene.nodes.find((one) => one.key === branch.machine) : undefined;
        if (!node || node.online !== true || !node.marks.includes("busy")) continue;
        const [from, to] = [branch.points[0]!, branch.points[branch.points.length - 1]!];
        flows.push(`<path class="flow" d="M${from[0].toFixed(1)} ${from[1].toFixed(1)}L${to[0].toFixed(1)} ${to[1].toFixed(1)}"/>`);
      }
    }
    this.marks.innerHTML = `<g class="clay" filter="url(#pen-marker)">${circles.join("")}</g><g class="moons">${moons.join("")}</g><g class="flows">${flows.join("")}</g>`;
  }
}

/** A crescent moon of radius `r` round `centre`, opening to the right. */
function crescent(centre: Point, r: number): string {
  const [x, y] = centre;
  const tip = (sign: number) => `${(x + r * 0.35).toFixed(1)} ${(y + sign * r * 0.94).toFixed(1)}`;
  return `M${tip(-1)}A${r} ${r} 0 1 0 ${tip(1)}A${(r * 0.78).toFixed(1)} ${(r * 0.78).toFixed(1)} 0 1 1 ${tip(-1)}Z`;
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

/** Text for SVG, its markup characters written as entities. */
function escapeText(text: string): string {
  return text.replace(/[&<>"]/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[character]!);
}
