// The machines page's drawing (`world-gui.md`, 账号、机器、共享与借出): the
// account's World drawn by hand on graph paper, as the front page draws it,
// but to be pointed at. Each hub a knot, each machine a ring round the hub
// it hangs on, this computer marked with a blot, and between two hubs the
// computer logged in to both. A machine of another is named with whose it
// is; one whose agent alone is lent is a ring of clay, broken, since it is
// no machine of the account's to work on; one whose agents may reach the
// account's machines carries a small clay tick. The agents on a machine sit
// on its far side as on the front page, and a line carries moving strokes
// while something on it works. A machine out of reach is drawn with a dry
// pen. The names are text on the page, and each machine and hub is a
// button: pointing at one opens what is known of it beside the drawing.

import { blot, circle, outline, quadratic, ruled, seeded, ticks, type Point } from "../pen/shapes";
import { escape } from "../conversation/prose";

export interface MapHub {
  id: string;
  name: string;
  online: boolean;
}

export interface MapNode {
  key: string;
  name: string;
  /** Under the name: its system, or whose it is. */
  sub: string;
  hubs: string[];
  here: boolean;
  online: boolean | null;
  hold: "mine" | "others" | "lent";
  /** Its agents may reach the account's machines. */
  reaching: boolean;
  /** The agents on it, by what they are doing. */
  marks: ("busy" | "waiting" | "idle")[];
  /** What it is, said whole, for a pointer resting on it. */
  title: string;
  /** Where it relays to the hub for others, when its owner made it a relay. */
  relayAt?: string[];
  /** The relay it is connected through now, by its label. */
  through?: string | null;
  /** The relays it dials, in order, by their labels; the hub after them. */
  relays?: string[];
}

interface Placed {
  hubs: Map<string, Point>;
  nodes: Map<string, Point>;
  /** A machine's ring and a hub's knot, drawn smaller in a small box. */
  ring: number;
  knot: number;
}

/** Where each hub and machine goes in a box of `width` by `height`: one
 *  hub at the centre with its machines round it; several side by side,
 *  each with its own, and a machine on more than one below between them.
 *  A hub's own machines keep clear of the side that machine is on. */
function place(hubs: MapHub[], nodes: MapNode[], width: number, height: number): Placed {
  // Full size from a box about 640 wide; a phone's draws at about 0.6.
  const scale = Math.max(0.58, Math.min(1, Math.min(width, height * 1.3) / 640));
  const placed: Placed = { hubs: new Map(), nodes: new Map(), ring: 26 * scale, knot: Math.max(6, 8 * scale) };
  if (!hubs.length) {
    nodes.forEach((node, index) => placed.nodes.set(node.key, [width * ((index + 1) / (nodes.length + 1)), height * 0.5]));
    return placed;
  }
  const shared = nodes.filter((node) => node.hubs.filter((id) => hubs.some((hub) => hub.id === id)).length > 1);
  // A ring keeps its name, a line or two, clear of the edge above and below.
  const room = placed.ring + 44 * scale;
  const below = height - room;
  // With a machine shared below, the hubs sit a little high to leave it room.
  const cy = shared.length ? Math.max(room + placed.ring * 2, height * 0.42) : height * 0.5;
  hubs.forEach((hub, index) => placed.hubs.set(hub.id, [width * ((2 * index + 1) / (2 * hubs.length)), cy]));
  const margin = { x: placed.ring + 70 * scale };
  const share = width / hubs.length;
  const rx = Math.max(placed.ring * 2.4, Math.min(share / 2 - margin.x, width * 0.34));
  const ry = Math.max(placed.ring * 1.6, Math.min(cy - room, (shared.length ? below - placed.ring * 2.2 : height - room) - cy, height * 0.36));
  const sharedAt: Point = [width / 2, below];
  for (const hub of hubs) {
    const knot = placed.hubs.get(hub.id)!;
    const own = nodes.filter((node) => node.hubs.includes(hub.id) && !shared.includes(node));
    // Alone, a hub's machines go all the way round, from the upper left;
    // with a machine shared below, they leave a third of the circle on
    // that side open.
    const toward = Math.atan2(sharedAt[1] - knot[1], sharedAt[0] - knot[0]);
    const from = shared.length ? toward + Math.PI / 3 : Math.PI * 1.1;
    const span = shared.length ? (Math.PI * 4) / 3 : Math.PI * 2;
    own.forEach((node, index) => {
      const angle = from + (shared.length ? ((index + 0.5) / own.length) * span : (index / own.length) * span);
      placed.nodes.set(node.key, [knot[0] + Math.cos(angle) * rx, knot[1] + Math.sin(angle) * ry]);
    });
  }
  shared.forEach((node, index) => {
    const knots = node.hubs.map((id) => placed.hubs.get(id)).filter((point): point is Point => !!point);
    const x = knots.reduce((sum, point) => sum + point[0], 0) / knots.length;
    placed.nodes.set(node.key, [x + (index - (shared.length - 1) / 2) * (placed.ring * 4), sharedAt[1]]);
  });
  return placed;
}

export class NetworkMap {
  readonly element: HTMLElement;
  private readonly drawing: HTMLElement;
  private readonly marks: HTMLElement;
  private readonly labels: HTMLElement;
  private size: { width: number; height: number } | null = null;
  private shown: { hubs: MapHub[]; nodes: MapNode[]; selected: string | null; hubLabels: Map<string, string> } | null = null;
  private drawn = "";
  private labelled = "";
  private marked = "";
  private placed: Placed | null = null;

  constructor(private readonly pick: (key: string) => void) {
    this.element = document.createElement("figure");
    this.element.className = "network-map";
    this.element.innerHTML = `<div class="map-drawing"></div><div class="map-marks"></div><div class="map-labels"></div>`;
    this.drawing = this.element.querySelector<HTMLElement>(".map-drawing")!;
    this.marks = this.element.querySelector<HTMLElement>(".map-marks")!;
    this.labels = this.element.querySelector<HTMLElement>(".map-labels")!;
    this.labels.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-key]");
      if (button) this.pick(button.dataset.key!);
    });
    // Drawn again only when the box changes by more than a little: the
    // pen's grain is costly.
    new ResizeObserver(() => {
      const width = Math.round(this.element.clientWidth);
      const height = Math.round(this.element.clientHeight);
      if (!width || !height) return;
      const last = this.size;
      if (last && Math.abs(last.width - width) < 12 && Math.abs(last.height - height) < 12) return;
      this.size = { width, height };
      this.draw();
    }).observe(this.element);
  }

  /** Draws the hubs and machines, `selected` marked; `hubLabels` is what
   *  stands under each hub's name. */
  update(hubs: MapHub[], nodes: MapNode[], selected: string | null, hubLabels: Map<string, string>): void {
    this.shown = { hubs, nodes, selected, hubLabels };
    this.draw();
  }

  private draw(): void {
    const { size, shown } = this;
    if (!size || !shown) return;
    const key = JSON.stringify([size, shown.hubs, shown.nodes.map((node) => [node.key, node.hubs, node.online, node.hold, node.reaching, node.here, node.marks])]);
    if (key !== this.drawn) {
      const first = this.drawn === "";
      this.drawn = key;
      this.placed = place(shown.hubs, shown.nodes, size.width, size.height);
      this.drawing.innerHTML = this.compose(shown.hubs, shown.nodes, this.placed, size);
      this.drawing.classList.toggle("arriving", first);
    }
    this.drawLabels();
    this.drawSelection();
  }

  /** The names, each a button over its ring and beside it. */
  private drawLabels(): void {
    const { size, shown, placed } = this;
    if (!size || !shown || !placed) return;
    const at = (point: Point) => `left:${((point[0] / size.width) * 100).toFixed(2)}%;top:${((point[1] / size.height) * 100).toFixed(2)}%`;
    const html = [
      ...shown.hubs.map((hub) => {
        const knot = placed.hubs.get(hub.id)!;
        const chosen = shown.selected === `hub:${hub.id}`;
        return `<button type="button" class="map-hub${chosen ? " chosen" : ""}${hub.online ? "" : " lost"}" style="${at([knot[0], knot[1] + placed.knot + 18])}" data-key="hub:${escape(hub.id)}" aria-pressed="${chosen}"><span class="name">${escape(hub.name)}</span><span class="sub">${escape(shown.hubLabels.get(hub.id) ?? "")}</span></button>`;
      }),
      ...shown.nodes.map((node) => {
        const point = placed.nodes.get(node.key);
        if (!point) return "";
        const outward = this.outward(node, point);
        const reach = placed.ring + 16;
        let below = Math.sin(outward) > 0.35;
        let above = Math.sin(outward) < -0.35;
        let anchor = Math.abs(Math.cos(outward)) < 0.35 ? "middle" : Math.cos(outward) > 0 ? "start" : "end";
        let label: Point = [point[0] + Math.cos(outward) * reach, point[1] + Math.sin(outward) * reach + (above ? -10 : below ? 12 : 0)];
        // A name with no room above or below its ring goes beside it, on
        // the side away from the middle.
        const tall = 22;
        if ((anchor === "middle" || above || below) && (label[1] - tall < 0 || label[1] + tall > size.height)) {
          const right = point[0] >= size.width / 2;
          anchor = right ? "start" : "end";
          above = below = false;
          label = [point[0] + (right ? reach : -reach), point[1]];
        }
        const chosen = shown.selected === node.key;
        const state = node.online === false ? " lost" : "";
        const disc = `width:${(placed.ring * 2 + 8).toFixed(0)}px;height:${(placed.ring * 2 + 8).toFixed(0)}px`;
        return `<button type="button" class="map-ring${chosen ? " chosen" : ""}${state}" style="${at(point)};${disc}" data-key="${escape(node.key)}" tabindex="-1" aria-hidden="true" title="${escape(node.title)}"></button><button type="button" class="map-node ${anchor}${chosen ? " chosen" : ""}${state} hold-${node.hold}" style="${at(label)}" data-key="${escape(node.key)}" aria-pressed="${chosen}" title="${escape(node.title)}"><span class="name">${escape(node.name)}</span>${node.sub ? `<span class="sub">${escape(node.sub)}</span>` : ""}</button>`;
      }),
    ].join("");
    if (html === this.labelled) return;
    this.labelled = html;
    this.labels.innerHTML = html;
    // A name near the edge is moved in until the whole of it shows.
    const box = this.element.getBoundingClientRect();
    for (const label of this.labels.querySelectorAll<HTMLElement>(".map-node, .map-hub")) {
      const rect = label.getBoundingClientRect();
      const x = rect.left < box.left ? box.left - rect.left : rect.right > box.right ? box.right - rect.right : 0;
      const y = rect.top < box.top ? box.top - rect.top : rect.bottom > box.bottom ? box.bottom - rect.bottom : 0;
      if (x || y) label.style.translate = `${x.toFixed(1)}px ${y.toFixed(1)}px`;
    }
  }

  /** The machine chosen, circled with the marker in clay. */
  private drawSelection(): void {
    const { size, shown, placed } = this;
    if (!size || !shown || !placed) return;
    const key = JSON.stringify([size, shown.selected, this.drawn.length]);
    if (key === this.marked) return;
    this.marked = key;
    const point = shown.selected?.startsWith("hub:") ? placed.hubs.get(shown.selected.slice(4)) : shown.selected ? placed.nodes.get(shown.selected) : undefined;
    if (!point) {
      this.marks.innerHTML = "";
      return;
    }
    const hub = shown.selected!.startsWith("hub:");
    const random = seeded(`chosen:${shown.selected}`);
    const ring = circle(point, hub ? placed.knot + 9 : placed.ring + 9, 3.2, random);
    this.marks.innerHTML = `<svg viewBox="0 0 ${size.width} ${size.height}" preserveAspectRatio="xMidYMid meet" aria-hidden="true"><g class="ink clay" filter="url(#pen-marker)"><path d="${ring}"/></g></svg>`;
  }

  /** The direction away from the hubs a machine hangs on: where its agents
   *  and its name go. */
  private outward(node: MapNode, point: Point): number {
    const knots = node.hubs.map((id) => this.placed?.hubs.get(id)).filter((knot): knot is Point => !!knot);
    if (!knots.length) return Math.PI / 2;
    const kx = knots.reduce((sum, knot) => sum + knot[0], 0) / knots.length;
    const ky = knots.reduce((sum, knot) => sum + knot[1], 0) / knots.length;
    if (Math.hypot(point[0] - kx, point[1] - ky) < 1) return Math.PI / 2;
    return Math.atan2(point[1] - ky, point[0] - kx);
  }

  private compose(hubs: MapHub[], nodes: MapNode[], placed: Placed, { width, height }: { width: number; height: number }): string {
    const { ring: ringRadius, knot: knotRadius } = placed;
    const paper: string[] = [];
    const graphite: string[] = [];
    const dry: string[] = [];
    const clay: string[] = [];
    const flows: string[] = [];
    const grid = width < 600 ? 28 : 32;

    // Graph paper, ruled by hand, every fourth line a little firmer.
    const ruler = seeded("map-paper");
    for (let x = grid / 2, index = 0; x < width; x += grid, index += 1) {
      paper.push(`<path class="${index % 4 === 2 ? "major" : "minor"}" d="${ruled([x, -4], [x, height + 4], 0.9, ruler)}"/>`);
    }
    for (let y = grid / 2, index = 0; y < height; y += grid, index += 1) {
      paper.push(`<path class="${index % 4 === 2 ? "major" : "minor"}" d="${ruled([-4, y], [width + 4, y], 0.9, ruler)}"/>`);
    }

    for (const hub of hubs) {
      const knot = placed.hubs.get(hub.id)!;
      const random = seeded(`knot:${hub.id}`);
      (hub.online ? graphite : dry).push(`<path d="${circle(knot, knotRadius, 1.8, random)}"/>`);
      if (hub.online) graphite.push(`<path d="${blot(knot, 2.4, random)}"/>`);
    }

    for (const node of nodes) {
      const point = placed.nodes.get(node.key);
      if (!point) continue;
      const [x, y] = point;
      const online = node.online === true;
      const random = seeded(`map:${node.key}`);
      const busy = node.marks.includes("busy");
      // A line from the ring to each knot it hangs on, a little bowed.
      for (const id of node.hubs) {
        const knot = placed.hubs.get(id);
        if (!knot) continue;
        const dx = knot[0] - x;
        const dy = knot[1] - y;
        const length = Math.hypot(dx, dy) || 1;
        const start: Point = [x + (dx / length) * (ringRadius + 5), y + (dy / length) * (ringRadius + 5)];
        const end: Point = [knot[0] - (dx / length) * (knotRadius + 5), knot[1] - (dy / length) * (knotRadius + 5)];
        const bow = (random() - 0.5) * 0.2 * length;
        const control: Point = [(start[0] + end[0]) / 2 + (-dy / length) * bow, (start[1] + end[1]) / 2 + (dx / length) * bow];
        const line = quadratic(start, control, end);
        if (online && node.hold !== "lent") graphite.push(`<path d="${outline(line, 1.5, random, { pressure: 0.3, taper: 0.06 })}"/>`);
        else (online ? clay : dry).push(...ticks(line, online ? 7 : 9, online ? 5 : 4, online ? 1.3 : 1.4, random).map((d) => `<path d="${d}"/>`));
        if (online && busy) {
          flows.push(`<path class="flow" d="M${start[0].toFixed(1)} ${start[1].toFixed(1)}Q${control[0].toFixed(1)} ${control[1].toFixed(1)} ${end[0].toFixed(1)} ${end[1].toFixed(1)}"/>`);
        }
      }
      // The ring: graphite for the account's own and others' machines it
      // works on, clay and broken where an agent alone is lent.
      if (node.hold === "lent") {
        (online ? clay : dry).push(...ticks(circlePoints([x, y], ringRadius, 64), 9, 5.5, 2, random).map((d) => `<path d="${d}"/>`));
      } else if (online) {
        graphite.push(`<path d="${circle([x, y], ringRadius, 2.1, random)}"/>`);
        graphite.push(`<path class="second" d="${circle([x, y], ringRadius + 1.2, 1.1, random)}"/>`);
      } else {
        dry.push(`<path d="${circle([x, y], ringRadius, 2.2, random)}"/>`);
      }
      if (node.here) graphite.push(`<path d="${blot([x, y], 3, random)}"/>`);
      // Another's machine whose agents may reach the account's: a short
      // clay tick across the top of its ring.
      if (node.reaching) {
        const tick: Point[] = [
          [x + ringRadius * 0.45, y - ringRadius * 1.08],
          [x + ringRadius * 0.62, y - ringRadius * 0.86],
          [x + ringRadius * 1.0, y - ringRadius * 1.3],
        ];
        clay.push(`<path d="${outline(tick, 2.2, random, { pressure: 0.25, taper: 0.2 })}"/>`);
      }
      // The agents on it, on the side away from its hubs.
      const outward = this.outward(node, point);
      node.marks.forEach((mark, place) => {
        const at = outward + (place - (node.marks.length - 1) / 2) * 0.4 - 0.9;
        const spot: Point = [x + Math.cos(at) * (ringRadius + 13), y + Math.sin(at) * (ringRadius + 13)];
        const own = seeded(`map-agent:${node.key}:${place}`);
        if (mark === "waiting") clay.push(`<path d="${circle(spot, 5, 1.8, own)}"/>`);
        else if (mark === "busy") clay.push(`<path d="${blot(spot, 4.4, own)}"/>`);
        else graphite.push(`<path d="${circle(spot, 3.4, 1.2, own)}"/>`);
      });
    }

    return `<svg viewBox="0 0 ${width} ${height}" preserveAspectRatio="xMidYMid meet" aria-hidden="true">
      <defs>
        <radialGradient id="map-fade" cx="50%" cy="50%" r="62%"><stop offset="0.4" stop-color="white"/><stop offset="1" stop-color="black"/></radialGradient>
        <mask id="map-edge"><rect x="0" y="0" width="${width}" height="${height}" fill="url(#map-fade)"/></mask>
      </defs>
      <g class="paper" mask="url(#map-edge)"><g filter="url(#pen-rule)">${paper.join("")}</g></g>
      <g class="ink graphite" filter="url(#pen-marker)">${graphite.join("")}</g>
      <g class="ink dry" filter="url(#pen-dry)">${dry.join("")}</g>
      <g class="ink clay" filter="url(#pen-marker)">${clay.join("")}</g>
      <g class="flows">${flows.join("")}</g>
    </svg>`;
  }
}

/** Points round a circle, for a ring drawn as broken strokes. */
function circlePoints(centre: Point, radius: number, count: number): Point[] {
  const points: Point[] = [];
  for (let index = 0; index <= count; index += 1) {
    const angle = (index / count) * Math.PI * 2;
    points.push([centre[0] + Math.cos(angle) * radius, centre[1] + Math.sin(angle) * radius]);
  }
  return points;
}
