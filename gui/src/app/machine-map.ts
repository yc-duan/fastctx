// The machines page's drawing as a canvas (`world-gui.md`, 账号、机器、共享与
// 借出): the account's World drawn by hand on graph paper as a tree with
// square forks (`map-layout.ts`), moved and zoomed (`../map-canvas`). The hub
// at the top, a knot for each group of machines by whose they are, and the
// machines hanging from their group's trunk, names to the right. What it
// shows grows as it comes nearer: far off, the tree and the groups; nearer,
// what each machine is, the agents on it, and strokes moving along its twig
// while something on it works. The account's own end with a dashed ring that
// adds a machine. A machine of another whose agent alone is lent is a broken
// ring of clay; one out of reach is drawn with a dry pen and hangs by a
// broken twig. A machine others may connect to directly wears a dashed halo,
// in pencil until the hub has checked it: every machine that reaches it
// links with it, so the links are not drawn one by one; which are up is
// said in its details. A group of many can be folded to its knot, filled.
// What floats beside a ring chosen is the page's.

import { MapCanvas, PenLayer, type Bounds, type CanvasEvent, type CanvasFrame, type CanvasItem, type PenStroke, type Point } from "../map-canvas";
import { blot, circle, outline, ruled, seeded, ticks } from "../pen/shapes";
import type { MachinesWords } from "./account-words";
import { GROUP_KNOT, KNOT, MapLayout, RING, type Branch, type Placement } from "./map-layout";
import type { MapHub, MapLink, MapNode } from "./network-map";

export interface MachineMapOptions {
  words: MachinesWords;
  /** Where this device remembers the places, or nowhere. */
  storageKey: string | null;
  event(event: CanvasEvent): void;
  /** Each painted frame, for what the page floats over the drawing. */
  frame?(frame: CanvasFrame): void;
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
   *  in their group. */
  links: MapLink[];
  /** Machines direct connections were just allowed to, whose halo is in
   *  pencil until the hub has checked them. */
  opening: ReadonlySet<string>;
  /** Machines framed with the mouse, or inside the frame being drawn. */
  framed: readonly string[];
  /** The groups folded to their knot, by key. */
  folded: ReadonlySet<string>;
}

/** A line on the drawing, as a pointer resting on it is told of it: a
 *  machine's twig, the end of its way to the hub. */
export type LineSeen = { kind: "hub"; machine: string; hub: string; ms?: number | undefined } | { kind: "through"; machine: string; through: string };

/** The key of a hub's dashed ring that adds a machine. */
export const addKey = (hub: string) => `add:${hub}`;

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
  private readonly marks: SVGSVGElement;
  private readonly themes: MutationObserver;
  private scene: MachineMapScene | null = null;
  private placement: Placement | null = null;
  private level = -1;
  private composed = "";
  private marked = "";

  constructor(private readonly options: MachineMapOptions) {
    const { words } = options;
    this.layout = new MapLayout(options.storageKey);
    this.surface = new MapCanvas({
      label: words.map,
      controls: { minus: words.zoomOut, plus: words.zoomIn, fit: words.fit, scale: words.scale },
      hints: { zoom: words.hintZoom, pan: words.hintPan },
      minScale: 1 / 16,
      maxScale: 4,
      levels: LEVELS,
      validTarget: () => false,
      event: (event) => options.event(event),
      frame: (frame) => this.onFrame(frame),
    });
    this.element = this.surface.element;
    this.element.classList.add("machine-map");
    this.pens = new PenLayer(this.surface.ink, "texture");
    // The circle round the one chosen and the strokes moving along a busy
    // twig are few, and drawn live over the ink.
    this.marks = document.createElementNS(SVG, "svg");
    this.marks.classList.add("map-live");
    this.marks.setAttribute("width", "1");
    this.marks.setAttribute("height", "1");
    this.marks.setAttribute("aria-hidden", "true");
    this.surface.ink.append(this.marks);
    // The ink's colours are resolved for the textures: drawn again when the
    // theme changes.
    this.themes = new MutationObserver(() => {
      this.composed = "";
      this.compose();
    });
    this.themes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class"] });
  }

  /** The machine, group or hub's place on the screen, in the canvas's
   *  pixels. */
  screenOf(key: string): Point | null {
    const point = this.pointOf(key);
    return point ? this.surface.camera.toScreen(point) : null;
  }

  /** The machine's twig within a few pixels of `screen`, a point in the
   *  canvas's pixels, if any. A point within a ring is not on a twig. */
  lineAt(screen: Point): LineSeen | null {
    const { scene, placement } = this;
    if (!scene || !placement) return null;
    const scale = this.surface.camera.view.scale;
    const at = this.surface.camera.toWorld(screen);
    for (const point of placement.machines.values()) if (Math.hypot(at[0] - point[0], at[1] - point[1]) * scale <= RING * scale + 4) return null;
    // Within 7 pixels on the screen, however far the drawing is zoomed.
    let best: { branch: Branch; distance: number } | null = null;
    for (const branch of placement.branches) {
      if (!branch.machine) continue;
      const distance = distanceTo(branch.points, at) * scale;
      if (distance <= 7 && (!best || distance < best.distance)) best = { branch, distance };
    }
    const found = best as { branch: Branch } | null;
    const node = found ? scene.nodes.find((one) => one.key === found.branch.machine) : undefined;
    if (!found || !node) return null;
    const through = node.through ? scene.nodes.find((one) => one.name === node.through) : undefined;
    return through ? { kind: "through", machine: node.key, through: through.key } : { kind: "hub", machine: node.key, hub: found.branch.hub, ms: node.hubMs };
  }

  /** How wide a name is on the screen, whether or not it is shown now. */
  nameWidth(key: string): number {
    return this.surface.labelSize(key)?.width ?? 0;
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
   *  system asks for less motion. */
  glide(dx: number, dy: number): void {
    if (Math.hypot(dx, dy) < 1) return;
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
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
    this.placement = this.layout.update(scene.hubs, scene.nodes, scene.links, scene.folded);
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

  private onFrame(frame: CanvasFrame): void {
    this.pens.view(frame);
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

  /** The hit areas and names: every hub, group, dashed ring and machine;
   *  a hub's name above it, the others' to their right. */
  private place(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const { words } = this.options;
    const near = this.level >= 1;
    const items: CanvasItem[] = [
      ...scene.hubs.map((hub): CanvasItem => ({
        key: `hub:${hub.id}`,
        kind: "hub",
        point: placement.hubs.get(hub.id)!,
        radius: KNOT + 6,
        label: hub.name,
        sub: scene.hubLabels.get(hub.id) ?? "",
        priority: 3,
        side: "above",
      })),
      ...placement.groups.map((group): CanvasItem => ({
        key: group.key,
        kind: "group",
        point: group.point,
        radius: GROUP_KNOT + 8,
        // Open, on one line so it keeps clear of the first machine under
        // it; folded, its count under its name, narrow beside its
        // neighbours.
        label: group.folded ? group.label : `${group.label} · ${words.groupSub(group.count, group.online)}`,
        sub: group.folded ? words.groupFolded(group.count, group.online) : "",
        priority: 2,
        side: "right",
      })),
      ...scene.adds.flatMap((hub): CanvasItem[] => {
        const point = this.pointOf(addKey(hub));
        return point ? [{ key: addKey(hub), kind: "machine", point, radius: RING, labelRadius: RING + 6, label: scene.waiting.has(hub) ? words.waitingJoin : words.addMachine, priority: -1, side: "right" }] : [];
      }),
      ...scene.nodes.flatMap((node): CanvasItem[] => {
        const point = placement.machines.get(node.key);
        if (!point) return [];
        const through = node.through ? words.via(node.through) : "";
        return [{ key: node.key, kind: "machine", point, radius: RING, labelRadius: RING + 6, label: node.name, sub: near ? [node.sub, through].filter(Boolean).join(" · ") : through, priority: node.here ? 1 : 0, side: "right" }];
      }),
    ];
    this.surface.setScene(items, [], placement.bounds);
    const selected = scene.selected && items.some((item) => item.key === scene.selected) ? scene.selected : null;
    if (this.surface.selection !== selected) this.surface.setSelection(selected);
  }

  /** The ink, composed again only when what it shows changes. */
  private compose(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const near = this.level >= 1;
    const key = JSON.stringify([
      near,
      scene.hubs,
      scene.adds,
      [...scene.waiting],
      scene.nodes.map((node) => [node.key, node.online, node.hold, node.reaching, node.here, near ? node.marks : null, node.directAt, node.through]),
      [...scene.opening],
      [...placement.machines],
      placement.groups.map((group) => [group.key, group.folded, group.point]),
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

    // The tree: ruled lines, square at the corners; a twig to a machine out
    // of reach, broken.
    for (const branch of placement.branches) {
      const [from, to] = [branch.points[0]!, branch.points[branch.points.length - 1]!];
      if (Math.hypot(to[0] - from[0], to[1] - from[1]) < 1) continue;
      const random = seeded(`branch:${from.join(",")}:${to.join(",")}`);
      if (branch.out) for (const d of ticks(branch.points, 6, 4, 1.2, random)) add(d, spanning(branch.points, 3), "dry", graphite, 0.45);
      else add(ruled(from, to, 1.3, random), spanning(branch.points, 3), "marker", graphite, 0.7);
    }

    for (const hub of scene.hubs) {
      const knot = placement.hubs.get(hub.id)!;
      const random = seeded(`knot:${hub.id}`);
      add(circle(knot, KNOT, 1.8, random), around(knot, KNOT + 4), hub.online ? "marker" : "dry", graphite, hub.online ? 0.9 : 0.5);
      if (hub.online) add(blot(knot, 2.4, random), around(knot, 5), "marker", graphite, 0.9);
    }

    // A group's knot: open while its machines hang below it, filled when
    // it is folded and holds them.
    for (const group of placement.groups) {
      const random = seeded(`group:${group.key}`);
      if (group.folded) add(blot(group.point, GROUP_KNOT, random), around(group.point, GROUP_KNOT + 3), "marker", graphite, 0.85);
      else add(circle(group.point, GROUP_KNOT, 1.4, random), around(group.point, GROUP_KNOT + 3), "marker", graphite, 0.85);
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
        add(circle(point, RING, 2.1, random), around(point, RING + 4), "marker", graphite, 0.9);
        add(circle(point, RING + 1.2, 1.1, random), around(point, RING + 4), "marker", graphite, 0.45);
      } else {
        add(circle(point, RING, 2.2, random), around(point, RING + 4), "dry", graphite, 0.5);
      }
      if (node.here) add(blot(point, 3, random), around(point, 5), "marker", graphite, 0.9);
      // Others may connect to it directly: a dashed halo round its ring,
      // in pencil while the hub checks it.
      const opening = scene.opening.has(node.key);
      if (node.directAt?.length || opening) {
        for (const d of ticks(ringPoints(point, RING + 8, 72), 5, 3.4, 1.2, random)) add(d, around(point, RING + 12), opening ? "dry" : "marker", graphite, opening ? 0.5 : 0.75);
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
      // The agents on it, round the upper right of its ring, clear of the
      // twig on its left and of its name on its right.
      node.marks.forEach((mark, place) => {
        const at = -1.15 + place * 0.36;
        const spot: Point = [x + Math.cos(at) * (RING + 15), y + Math.sin(at) * (RING + 15)];
        const own = seeded(`map-agent:${node.key}:${place}`);
        if (mark === "waiting") add(circle(spot, 5, 1.8, own), around(spot, 8), "marker", clay);
        else if (mark === "busy") add(blot(spot, 4.4, own), around(spot, 7), "marker", clay);
        else add(circle(spot, 3.4, 1.2, own), around(spot, 6), "marker", graphite, 0.9);
      });
    }
    void this.pens.update(strokes).catch((error: unknown) => console.warn("the machines drawing could not be inked", error));
  }

  /** Drawn live over the ink: the clay circle round the one chosen and the
   *  ones framed, the circles round machines just come, and the strokes
   *  moving along the twig of a machine something works on. */
  private mark(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const near = this.level >= 1;
    const key = JSON.stringify([scene.selected, scene.framed, [...scene.fresh], near, scene.nodes.map((node) => [node.key, node.online, node.marks.includes("busy")]), [...placement.machines], placement.groups.map((group) => group.point)]);
    if (key === this.marked) return;
    this.marked = key;
    const circles: string[] = [];
    const chosen = scene.selected ? this.pointOf(scene.selected) : null;
    if (chosen) {
      const small = scene.selected!.startsWith("hub:") ? KNOT + 9 : scene.selected!.startsWith("group:") ? GROUP_KNOT + 8 : RING + 9;
      circles.push(`<path class="chosen" d="${circle(chosen, small, 3.2, seeded(`chosen:${scene.selected}`))}"/>`);
    }
    for (const framed of scene.framed) {
      const point = this.pointOf(framed);
      if (point && framed !== scene.selected) circles.push(`<path class="chosen" d="${circle(point, RING + 9, 3.2, seeded(`chosen:${framed}`))}"/>`);
    }
    for (const fresh of scene.fresh) {
      const point = this.pointOf(fresh);
      if (point && fresh !== scene.selected) circles.push(`<path class="arrived" d="${circle(point, RING + 9, 3.2, seeded(`arrived:${fresh}`))}"/>`);
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
    this.marks.innerHTML = `<g class="clay" filter="url(#pen-marker)">${circles.join("")}</g><g class="flows">${flows.join("")}</g>`;
  }
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

/** The distance from `point` to the nearest of the segments joining
 *  `points`, in the drawing's units. */
function distanceTo(points: Point[], point: Point): number {
  let best = Infinity;
  for (let index = 1; index < points.length; index += 1) {
    const [ax, ay] = points[index - 1]!;
    const [bx, by] = points[index]!;
    const dx = bx - ax;
    const dy = by - ay;
    const length = dx * dx + dy * dy;
    const t = length ? Math.max(0, Math.min(1, ((point[0] - ax) * dx + (point[1] - ay) * dy) / length)) : 0;
    best = Math.min(best, Math.hypot(point[0] - (ax + t * dx), point[1] - (ay + t * dy)));
  }
  return best;
}
