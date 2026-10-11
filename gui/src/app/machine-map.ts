// The machines page's drawing as a canvas (`world-gui.md`, 账号、机器、共享与
// 借出): the account's World drawn by hand on graph paper, each hub a knot
// and each machine a ring at its own place round the hub it hangs on
// (`map-layout.ts`), moved by the hand and zoomed (`../map-canvas`). What
// it shows grows as it comes nearer: far off, the rings and their names;
// nearer, what each machine is, the agents on it, and strokes moving along
// a line while something on it works. Beside each hub the account may add
// to, a dashed ring adds a machine. A machine of another whose agent alone
// is lent is a broken ring of clay; one out of reach is drawn with a dry
// pen. A machine that relays for others carries a small mark, and one
// connected through it has its line bent through it. The machine chosen has
// a handle on its line: dragged onto a relay or a hub, it asks to change how
// that machine reaches the hub, and the way it was asked for is drawn as a
// pencil line until the machine really goes that way. What floats beside a
// ring chosen is the page's.

import { MapCanvas, PenLayer, type Bounds, type CanvasEvent, type CanvasFrame, type CanvasHandle, type CanvasItem, type PenStroke, type Point } from "../map-canvas";
import { blot, circle, outline, quadratic, seeded, ticks } from "../pen/shapes";
import type { MachinesWords } from "./account-words";
import { KNOT, MapLayout, RING, type Placement } from "./map-layout";
import type { MapHub, MapNode } from "./network-map";

export interface MachineMapOptions {
  words: MachinesWords;
  /** Where this device remembers the places, or nowhere. */
  storageKey: string | null;
  event(event: CanvasEvent): void;
  /** Each painted frame, for what the page floats over the drawing. */
  frame?(frame: CanvasFrame): void;
  /** Whether the machine `source` may be made to reach its hub through
   *  `target`: a relay's machine, or the hub itself. */
  validRoute(source: string, target: string): boolean;
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
  /** The machine whose line carries the handle that changes how it
   *  reaches its hub. */
  route: string | null;
  /** Ways asked for and not yet taken: a machine, and what it was asked to
   *  go through, a relay's machine or a hub. */
  pending: ReadonlyMap<string, string>;
  /** The chosen machine's ways to its hub, in the order it tries them:
   *  each relay of its list that is a machine on the drawing, then the hub.
   *  The one it is on is its line; the others are drawn faint and dashed. */
  ways: Way[];
}

/** One of a machine's ways to its hub. */
export interface Way {
  machine: string;
  /** A relay's machine, or `hub:<id>`. */
  to: string;
  /** Its place in the order, from 1. */
  order: number;
  /** The way the machine is on now. */
  current: boolean;
}

/** The key of the handle on a machine's line. */
export const routeKey = (machine: string) => `route:${machine}`;

/** The key of a hub's dashed ring that adds a machine. */
export const addKey = (hub: string) => `add:${hub}`;

/** Zoom levels: below the first the drawing is far, above the second near. */
const LEVELS: [number, number] = [0.55, 1.2];
const SVG = "http://www.w3.org/2000/svg";

export class MachineMap {
  readonly element: HTMLElement;
  private readonly surface: MapCanvas;
  private readonly pens: PenLayer;
  private readonly layout: MapLayout;
  private readonly marks: SVGSVGElement;
  /** The pencil line following a dragged handle, over the drawing. */
  private readonly draft: SVGSVGElement;
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
      minScale: 1 / 16,
      maxScale: 4,
      levels: LEVELS,
      validTarget: (handle, item) => options.validRoute(handle.source, item.key),
      event: (event) => {
        if (event.kind === "draft") this.drawDraft();
        else if (event.kind === "drop" || event.kind === "cancel") this.draft.innerHTML = "";
        options.event(event);
      },
      frame: (frame) => this.onFrame(frame),
    });
    this.element = this.surface.element;
    this.element.classList.add("machine-map");
    this.pens = new PenLayer(this.surface.ink, "texture");
    // The circle round the one chosen and the strokes moving along a busy
    // line are few, and drawn live over the ink.
    this.marks = document.createElementNS(SVG, "svg");
    this.marks.classList.add("map-live");
    this.marks.setAttribute("width", "1");
    this.marks.setAttribute("height", "1");
    this.marks.setAttribute("aria-hidden", "true");
    this.surface.ink.append(this.marks);
    this.draft = document.createElementNS(SVG, "svg");
    this.draft.classList.add("map-draft");
    this.draft.setAttribute("aria-hidden", "true");
    this.surface.overlay.append(this.draft);
    // The ink's colours are resolved for the textures: drawn again when the
    // theme changes.
    this.themes = new MutationObserver(() => {
      this.composed = "";
      this.compose();
    });
    this.themes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class"] });
  }

  /** The machine or hub's place on the screen, in the canvas's pixels. */
  screenOf(key: string): Point | null {
    const point = this.pointOf(key);
    return point ? this.surface.camera.toScreen(point) : null;
  }

  /** Where each of the chosen machine's ways is marked with its order, on
   *  the screen: the middle of its line. */
  wayPoints(): { way: Way; at: Point }[] {
    const { scene, placement } = this;
    if (!scene || !placement) return [];
    return scene.ways.flatMap((way) => {
      const from = placement.machines.get(way.machine);
      const to = this.pointOf(way.to);
      if (!from || !to) return [];
      const line = this.lineOf(from, to, way.current ? this.currentName(way) : `${way.machine}>${way.to}`, way.to.startsWith("hub:") ? KNOT : RING);
      const middle: Point = [
        0.25 * line.from[0] + 0.5 * line.control[0] + 0.25 * line.to[0],
        0.25 * line.from[1] + 0.5 * line.control[1] + 0.25 * line.to[1],
      ];
      return [{ way, at: this.surface.camera.toScreen(middle) }];
    });
  }

  /** The name a machine's current line is bowed by (`routesOf`). */
  private currentName(way: Way): string {
    return `${way.machine}>${way.to.startsWith("hub:") ? way.to.slice(4) : way.to}`;
  }

  /** Where the handle on a machine's line is on the screen, while it has
   *  one. */
  handleOf(machine: string): Point | null {
    return this.surface.handlePoint(routeKey(machine));
  }

  /** The screen box of a ring, for keeping names clear of what floats. */
  reserve(boxes: readonly Bounds[]): void {
    this.surface.reserveLabels(boxes);
  }

  fit(): void {
    this.surface.fit();
  }

  dispose(): void {
    this.themes.disconnect();
    this.pens.dispose();
    this.surface.dispose();
  }

  update(scene: MachineMapScene): void {
    this.scene = scene;
    this.placement = this.layout.update(scene.hubs, scene.nodes);
    this.compose();
    this.place();
    this.mark();
  }

  private pointOf(key: string): Point | null {
    const placement = this.placement;
    if (!placement) return null;
    if (key.startsWith("hub:")) return placement.hubs.get(key.slice(4)) ?? null;
    // A hub's dashed ring waits where its next machine will stand.
    if (key.startsWith("add:")) return this.layout.vacant(key.slice(4));
    return placement.machines.get(key) ?? null;
  }

  private onFrame(frame: CanvasFrame): void {
    this.pens.view(frame);
    if (this.surface.activeDraft) this.drawDraft();
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

  /** The hit areas and names: every hub, dashed ring and machine. */
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
        sub: near ? (scene.hubLabels.get(hub.id) ?? "") : "",
        priority: 2,
      })),
      ...scene.adds.flatMap((hub): CanvasItem[] => {
        const point = this.pointOf(addKey(hub));
        return point ? [{ key: addKey(hub), kind: "machine", point, radius: RING, label: scene.waiting.has(hub) ? words.waitingJoin : words.addMachine, priority: -1 }] : [];
      }),
      ...scene.nodes.map((node): CanvasItem => ({
        key: node.key,
        kind: "machine",
        point: placement.machines.get(node.key)!,
        radius: RING,
        labelRadius: near ? RING + 20 : RING + 6,
        label: node.name,
        sub: near ? node.sub : "",
        priority: node.here ? 1 : 0,
      })),
    ];
    const handles: CanvasHandle[] = [];
    const route = scene.route ? scene.nodes.find((node) => node.key === scene.route) : undefined;
    if (route) {
      const point = placement.machines.get(route.key)!;
      const toward = this.routesOf(route)[0]?.to;
      if (toward) {
        const dx = toward[0] - point[0];
        const dy = toward[1] - point[1];
        const length = Math.hypot(dx, dy) || 1;
        // Along its line, past where its name sits below or beside it.
        const out = Math.min(RING + 44, length / 2);
        handles.push({ key: routeKey(route.key), source: route.key, point: [point[0] + (dx / length) * out, point[1] + (dy / length) * out], label: words.reroute });
      }
    }
    this.surface.setScene(items, handles, placement.bounds);
    const selected = scene.selected && items.some((item) => item.key === scene.selected) ? scene.selected : null;
    if (this.surface.selection !== selected) this.surface.setSelection(selected);
  }

  /** The ink, composed again only when what it shows changes. */
  private compose(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const near = this.level >= 1;
    const key = JSON.stringify([
      this.level >= 1,
      scene.hubs,
      scene.adds,
      [...scene.waiting],
      scene.nodes.map((node) => [node.key, node.hubs, node.online, node.hold, node.reaching, node.here, near ? node.marks : null, node.relayAt, node.through]),
      [...placement.machines],
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

    for (const hub of scene.hubs) {
      const knot = placement.hubs.get(hub.id)!;
      const random = seeded(`knot:${hub.id}`);
      add(circle(knot, KNOT, 1.8, random), around(knot, KNOT + 4), hub.online ? "marker" : "dry", graphite, hub.online ? 0.9 : 0.5);
      if (hub.online) add(blot(knot, 2.4, random), around(knot, 5), "marker", graphite, 0.9);
    }

    // The dashed ring beside a hub, a small cross in it: a machine to add.
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
      const point = placement.machines.get(node.key)!;
      const [x, y] = point;
      const online = node.online === true;
      const random = seeded(`map:${node.key}`);
      // A line from the ring to each knot it hangs on, a little bowed, or
      // to the relay it goes through.
      for (const { to, name, end } of this.routesOf(node)) {
        const line = this.lineOf(point, to, name, end);
        if (online && node.hold !== "lent") add(outline(line.points, 1.5, random, { pressure: 0.3, taper: 0.06 }), spanning(line.points, 4), "marker", graphite, 0.9);
        else for (const d of ticks(line.points, online ? 7 : 9, online ? 5 : 4, online ? 1.3 : 1.4, random)) add(d, spanning(line.points, 4), online ? "marker" : "dry", online ? clay : graphite, online ? 1 : 0.5);
      }
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
      // A relay: two short strokes across the upper left of its ring, as
      // a line passing through it.
      if (node.relayAt?.length) {
        for (const shift of [-4, 4]) {
          const stroke: Point[] = [
            [x - RING * 0.95 + shift, y - RING * 0.55 - shift],
            [x - RING * 0.55 + shift, y - RING * 0.95 - shift],
          ];
          add(outline(stroke, 2, random, { pressure: 0.3, taper: 0.2 }), spanning(stroke, 4), "marker", graphite, 0.9);
        }
      }
      if (!near) continue;
      // Another's machine whose agents may reach the account's: a short
      // clay tick across the top of its ring.
      if (node.reaching) {
        const tick: Point[] = [
          [x + RING * 0.45, y - RING * 1.08],
          [x + RING * 0.62, y - RING * 0.86],
          [x + RING * 1.0, y - RING * 1.3],
        ];
        add(outline(tick, 2.2, random, { pressure: 0.25, taper: 0.2 }), spanning(tick, 4), "marker", clay);
      }
      // The agents on it, on the side away from its hubs.
      const outward = this.outward(node, point);
      node.marks.forEach((mark, place) => {
        const at = outward + (place - (node.marks.length - 1) / 2) * 0.4 - 0.9;
        const spot: Point = [x + Math.cos(at) * (RING + 13), y + Math.sin(at) * (RING + 13)];
        const own = seeded(`map-agent:${node.key}:${place}`);
        if (mark === "waiting") add(circle(spot, 5, 1.8, own), around(spot, 8), "marker", clay);
        else if (mark === "busy") add(blot(spot, 4.4, own), around(spot, 7), "marker", clay);
        else add(circle(spot, 3.4, 1.2, own), around(spot, 6), "marker", graphite, 0.9);
      });
    }
    void this.pens.update(strokes).catch((error: unknown) => console.warn("the machines drawing could not be inked", error));
  }

  /** Where a machine's lines go: to each hub it hangs on, or, where it
   *  is connected through a relay on that hub that is a machine on the
   *  drawing, to that machine. */
  private routesOf(node: MapNode): { to: Point; name: string; end: number }[] {
    const placement = this.placement!;
    const relay = node.through ? this.scene?.nodes.find((one) => one.name === node.through && one.relayAt?.length) : undefined;
    return node.hubs.flatMap((id) => {
      const knot = placement.hubs.get(id);
      if (!knot) return [];
      if (relay && relay.hubs.includes(id)) {
        const at = placement.machines.get(relay.key);
        if (at) return [{ to: at, name: `${node.key}>${relay.key}`, end: RING }];
      }
      return [{ to: knot, name: `${node.key}>${id}`, end: KNOT }];
    });
  }

  /** A machine's line to a hub or a relay: from the ring's edge to the
   *  other end's, bowed the same way each time. */
  private lineOf(point: Point, knot: Point, name: string, end = KNOT): { points: Point[]; from: Point; control: Point; to: Point } {
    const random = seeded(`line:${name}`);
    const dx = knot[0] - point[0];
    const dy = knot[1] - point[1];
    const length = Math.hypot(dx, dy) || 1;
    const from: Point = [point[0] + (dx / length) * (RING + 5), point[1] + (dy / length) * (RING + 5)];
    const to: Point = [knot[0] - (dx / length) * (end + 5), knot[1] - (dy / length) * (end + 5)];
    const bow = (random() - 0.5) * 0.2 * length;
    const control: Point = [(from[0] + to[0]) / 2 + (-dy / length) * bow, (from[1] + to[1]) / 2 + (dx / length) * bow];
    return { points: quadratic(from, control, to), from, control, to };
  }

  /** The direction away from the hubs a machine hangs on: where its agents
   *  go. */
  private outward(node: MapNode, point: Point): number {
    const knots = node.hubs.map((id) => this.placement?.hubs.get(id)).filter((knot): knot is Point => !!knot);
    if (!knots.length) return Math.PI / 2;
    const kx = knots.reduce((sum, knot) => sum + knot[0], 0) / knots.length;
    const ky = knots.reduce((sum, knot) => sum + knot[1], 0) / knots.length;
    if (Math.hypot(point[0] - kx, point[1] - ky) < 1) return Math.PI / 2;
    return Math.atan2(point[1] - ky, point[0] - kx);
  }

  /** The pencil line from a dragged handle to where it is, over the drawing;
   *  clay when it is over a place it may be dropped. */
  private drawDraft(): void {
    const draft = this.surface.activeDraft;
    if (!draft) {
      this.draft.innerHTML = "";
      return;
    }
    const from = this.surface.handlePoint(draft.handle.key);
    if (!from) return;
    const to = this.surface.camera.toScreen(draft.point);
    const control: Point = [(from[0] + to[0]) / 2, (from[1] + to[1]) / 2 - Math.hypot(to[0] - from[0], to[1] - from[1]) * 0.1];
    this.draft.innerHTML = `<path class="${draft.target ? "over" : ""}" d="${outline(quadratic(from, control, to), 2.2, seeded(draft.handle.key))}"/>`;
  }

  /** Drawn live over the ink: the clay circle round the one chosen, the
   *  circles round machines just come, and the strokes moving along the
   *  line of a machine something works on. */
  private mark(): void {
    const { scene, placement } = this;
    if (!scene || !placement) return;
    const near = this.level >= 1;
    const key = JSON.stringify([scene.selected, [...scene.fresh], near, scene.route, [...scene.pending], scene.ways, scene.nodes.map((node) => [node.key, node.online, node.marks.includes("busy"), node.through]), [...placement.machines]]);
    if (key === this.marked) return;
    this.marked = key;
    const circles: string[] = [];
    const chosen = scene.selected ? this.pointOf(scene.selected) : null;
    if (chosen) {
      const hub = scene.selected!.startsWith("hub:");
      circles.push(`<path class="chosen" d="${circle(chosen, hub ? KNOT + 9 : RING + 9, 3.2, seeded(`chosen:${scene.selected}`))}"/>`);
    }
    for (const fresh of scene.fresh) {
      const point = this.pointOf(fresh);
      if (point && fresh !== scene.selected) circles.push(`<path class="arrived" d="${circle(point, RING + 9, 3.2, seeded(`arrived:${fresh}`))}"/>`);
    }
    const flows: string[] = [];
    if (near) {
      for (const node of scene.nodes) {
        if (node.online !== true || !node.marks.includes("busy")) continue;
        const point = placement.machines.get(node.key)!;
        for (const route of this.routesOf(node)) {
          const { from, control, to } = this.lineOf(point, route.to, route.name, route.end);
          flows.push(`<path class="flow" d="M${from[0].toFixed(1)} ${from[1].toFixed(1)}Q${control[0].toFixed(1)} ${control[1].toFixed(1)} ${to[0].toFixed(1)} ${to[1].toFixed(1)}"/>`);
        }
      }
    }
    // A way asked for and not yet taken: a pencil line, dashed, to where the
    // machine was asked to go.
    const asked: string[] = [];
    for (const [machine, target] of scene.pending) {
      const from = placement.machines.get(machine);
      const to = this.pointOf(target);
      if (!from || !to) continue;
      const { from: start, control, to: end } = this.lineOf(from, to, `${machine}>${target}`, target.startsWith("hub:") ? KNOT : RING);
      asked.push(`<path class="asked" d="M${start[0].toFixed(1)} ${start[1].toFixed(1)}Q${control[0].toFixed(1)} ${control[1].toFixed(1)} ${end[0].toFixed(1)} ${end[1].toFixed(1)}"/>`);
    }
    // The ways the chosen machine tries when its line fails: faint, dashed.
    const fallbacks: string[] = [];
    for (const way of scene.ways) {
      if (way.current || scene.pending.has(way.machine)) continue;
      const from = placement.machines.get(way.machine);
      const to = this.pointOf(way.to);
      if (!from || !to) continue;
      const { from: start, control, to: end } = this.lineOf(from, to, `${way.machine}>${way.to}`, way.to.startsWith("hub:") ? KNOT : RING);
      fallbacks.push(`<path class="fallback" d="M${start[0].toFixed(1)} ${start[1].toFixed(1)}Q${control[0].toFixed(1)} ${control[1].toFixed(1)} ${end[0].toFixed(1)} ${end[1].toFixed(1)}"/>`);
    }
    this.marks.innerHTML = `<g class="clay" filter="url(#pen-marker)">${circles.join("")}</g><g class="flows">${fallbacks.join("")}${flows.join("")}${asked.join("")}</g>`;
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
