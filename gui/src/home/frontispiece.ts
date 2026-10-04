// The front page's drawing: the World as it stands, drawn by hand on graph
// paper. Each machine a ring around the knot where they all meet, the
// agents on it as marks on its far side: a blot of clay for one at work, a
// ring of clay for one waiting on the reader, a small graphite ring for one
// idle. The line from a machine to the knot carries moving strokes while
// something on it works. A machine out of reach is drawn with a dry pen,
// faint and broken. The names are text on the page, never drawn, so they
// stay sharp.

import { blot, circle, outline, quadratic, ruled, seeded, ticks, type Point } from "../pen/shapes";
import type { WorldState } from "./model";
import type { ResidentNow } from "../world/world";

const ringRadius = 24;
const knotRadius = 7;

interface Composition {
  width: number;
  height: number;
  spread: { x: number; y: number };
  grid: number;
}

const wide: Composition = { width: 960, height: 320, spread: { x: 0.34, y: 0.3 }, grid: 32 };
const narrow: Composition = { width: 420, height: 400, spread: { x: 0.27, y: 0.31 }, grid: 28 };

interface Label {
  x: number;
  y: number;
  anchor: "start" | "middle" | "end";
  name: string;
  os: string;
  lost: boolean;
}

export class Frontispiece {
  private composition: Composition | null = null;
  private shown: { world: WorldState; residents: ResidentNow[] } | null = null;
  private drawn = "";
  private readonly drawing: HTMLElement;
  private readonly labels: HTMLElement;

  constructor(figure: HTMLElement) {
    figure.innerHTML = `<div class="frontispiece-drawing"></div><div class="frontispiece-labels"></div>`;
    this.drawing = figure.querySelector<HTMLElement>(".frontispiece-drawing")!;
    this.labels = figure.querySelector<HTMLElement>(".frontispiece-labels")!;
    new ResizeObserver(() => {
      const fit = figure.clientWidth < 600 ? narrow : wide;
      if (fit === this.composition) return;
      this.composition = fit;
      figure.style.aspectRatio = `${fit.width} / ${fit.height}`;
      this.draw();
    }).observe(figure);
  }

  update(world: WorldState, residents: ResidentNow[]): void {
    this.shown = { world, residents };
    this.draw();
  }

  private draw(): void {
    const { composition, shown } = this;
    if (!composition || !shown) return;
    // Only what the drawing shows decides whether it is drawn again: the
    // pen's grain is costly, and a drawing that has not changed is kept.
    const key = JSON.stringify([
      composition.width,
      shown.world.here,
      shown.world.nodes.map((node) => [node.name, node.online, "Ok" in node.facts && node.facts.Ok ? node.facts.Ok.os : ""]),
      shown.residents.map((r) => [r.about.id, r.about.machine, r.now?.activity, r.now?.waiting_for_user]),
    ]);
    if (key === this.drawn) return;
    const first = this.drawn === "";
    this.drawn = key;
    const { svg, labels } = compose(shown.world, shown.residents, composition);
    this.drawing.innerHTML = svg;
    this.drawing.classList.toggle("arriving", first);
    this.labels.innerHTML = labels
      .map((label) => {
        const left = (label.x / composition.width) * 100;
        const top = (label.y / composition.height) * 100;
        return `<span class="label ${label.anchor}${label.lost ? " lost" : ""}" style="left:${left.toFixed(2)}%;top:${top.toFixed(2)}%"><span class="name">${escapeHtml(label.name)}</span>${label.os ? `<span class="os">${escapeHtml(label.os)}</span>` : ""}</span>`;
      })
      .join("");
  }
}

function compose(world: WorldState, residents: ResidentNow[], fit: Composition): { svg: string; labels: Label[] } {
  const { width, height } = fit;
  const cx = width / 2;
  const cy = height / 2;
  const paper: string[] = [];
  const graphite: string[] = [];
  const dry: string[] = [];
  const clay: string[] = [];
  const flows: string[] = [];
  const labels: Label[] = [];

  // Graph paper, ruled by hand, every fourth line a little firmer.
  const grid = seeded("paper");
  for (let x = fit.grid / 2, index = 0; x < width; x += fit.grid, index += 1) {
    paper.push(`<path class="${index % 4 === 2 ? "major" : "minor"}" d="${ruled([x, -4], [x, height + 4], 0.9, grid)}"/>`);
  }
  for (let y = fit.grid / 2, index = 0; y < height; y += fit.grid, index += 1) {
    paper.push(`<path class="${index % 4 === 2 ? "major" : "minor"}" d="${ruled([-4, y], [width + 4, y], 0.9, grid)}"/>`);
  }

  // The knot where every machine's line meets.
  const knot = seeded("knot");
  graphite.push(`<path d="${circle([cx, cy], knotRadius, 1.6, knot)}"/>`);

  world.nodes.forEach((node, index) => {
    // Evenly round an ellipse from upper left; wide, as the drawing is.
    const angle = Math.PI * 1.1 + (index / world.nodes.length) * Math.PI * 2;
    const x = cx + Math.cos(angle) * width * fit.spread.x;
    const y = cy + Math.sin(angle) * height * fit.spread.y;
    const online = node.online === true;
    const busy = residents.some((r) => r.about.machine === node.name && r.now?.activity === "busy");
    const random = seeded(`machine:${node.name}`);

    // The line runs from the ring's edge to the knot's, a little bowed.
    const dx = cx - x;
    const dy = cy - y;
    const length = Math.hypot(dx, dy);
    const start: Point = [x + (dx / length) * (ringRadius + 5), y + (dy / length) * (ringRadius + 5)];
    const end: Point = [cx - (dx / length) * (knotRadius + 5), cy - (dy / length) * (knotRadius + 5)];
    const bow = (random() - 0.5) * 0.22 * length;
    const control: Point = [(start[0] + end[0]) / 2 + (-dy / length) * bow, (start[1] + end[1]) / 2 + (dx / length) * bow];
    const line = quadratic(start, control, end);
    if (online) graphite.push(`<path d="${outline(line, 1.5, random, { pressure: 0.3, taper: 0.06 })}"/>`);
    else dry.push(...ticks(line, 9, 4, 1.4, random).map((d) => `<path d="${d}"/>`));
    if (online && busy) {
      // Work under way: strokes that move along the line, drawn outside
      // the pen's grain so moving them costs nothing.
      const d = `M${start[0].toFixed(1)} ${start[1].toFixed(1)}Q${control[0].toFixed(1)} ${control[1].toFixed(1)} ${end[0].toFixed(1)} ${end[1].toFixed(1)}`;
      flows.push(`<path class="flow" d="${d}"/>`);
    }

    // The machine, and this one marked as where the reader is.
    if (online) {
      graphite.push(`<path d="${circle([x, y], ringRadius, 2.1, random)}"/>`);
      graphite.push(`<path class="second" d="${circle([x, y], ringRadius + 1.2, 1.1, random)}"/>`);
    } else {
      dry.push(`<path d="${circle([x, y], ringRadius, 2.2, random)}"/>`);
    }
    if (node.name === world.here) graphite.push(`<path d="${blot([x, y], 2.6, random)}"/>`);

    // Agents sit on the side away from the knot, spread along the ring.
    const outward = Math.atan2(-dy, -dx);
    const on = residents.filter((r) => r.about.machine === node.name && r.now?.activity !== "asleep");
    on.forEach((resident, place) => {
      const at = outward + (place - (on.length - 1) / 2) * 0.42 - 0.9;
      const mark: Point = [x + Math.cos(at) * (ringRadius + 13), y + Math.sin(at) * (ringRadius + 13)];
      const own = seeded(`agent:${resident.about.id}@${node.name}`);
      if (resident.now?.waiting_for_user) clay.push(`<path d="${circle(mark, 5, 1.8, own)}"/>`);
      else if (resident.now?.activity === "busy") clay.push(`<path d="${blot(mark, 4.4, own)}"/>`);
      else graphite.push(`<path d="${circle(mark, 3.4, 1.2, own)}"/>`);
    });

    // The name stands off the far side of the ring, read away from the knot.
    const reach = ringRadius + 18;
    const below = Math.sin(outward) > 0.35;
    const above = Math.sin(outward) < -0.35;
    labels.push({
      x: x + Math.cos(outward) * reach,
      y: y + Math.sin(outward) * reach + (above ? -10 : below ? 12 : 0),
      anchor: Math.abs(Math.cos(outward)) < 0.35 ? "middle" : Math.cos(outward) > 0 ? "start" : "end",
      name: node.name,
      os: "Ok" in node.facts && node.facts.Ok ? osName(node.facts.Ok.os) : "",
      lost: !online,
    });
  });

  const svg = `<svg viewBox="0 0 ${width} ${height}" preserveAspectRatio="xMidYMid meet" aria-hidden="true">
    <defs>
      <radialGradient id="paper-fade" cx="50%" cy="50%" r="60%"><stop offset="0.35" stop-color="white"/><stop offset="1" stop-color="black"/></radialGradient>
      <mask id="paper-edge"><rect x="0" y="0" width="${width}" height="${height}" fill="url(#paper-fade)"/></mask>
    </defs>
    <g class="paper" mask="url(#paper-edge)"><g filter="url(#pen-rule)">${paper.join("")}</g></g>
    <g class="ink graphite" filter="url(#pen-marker)">${graphite.join("")}</g>
    <g class="ink dry" filter="url(#pen-dry)">${dry.join("")}</g>
    <g class="ink clay" filter="url(#pen-marker)">${clay.join("")}</g>
    <g class="flows">${flows.join("")}</g>
  </svg>`;
  return { svg, labels };
}

/** A system as people write it: Windows, Linux, macOS. */
export function osName(os: string): string {
  return ({ windows: "Windows", linux: "Linux", macos: "macOS" } as Record<string, string>)[os] ?? os;
}

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (char) => `&#${char.charCodeAt(0)};`);
}
