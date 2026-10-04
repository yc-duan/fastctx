// The front page's drawing: the World as it stands, in ink. Each machine a
// ring around the knot where they all meet, the agents on it as marks on
// its far side, the line to the knot flowing while something on that
// machine is at work. A machine out of reach is drawn faint and broken.
// The strokes tremble a little, as living things do, and keep still where
// the system asks for less motion; the names are text on the page, not
// paint, so they stay sharp, selectable and read aloud.

import { InkScene, defaultStyle, paths, type InkStroke, type Point } from "../ink";
import type { WorldState } from "./model";
import type { ResidentNow } from "../world/world";

const ringRadius = 24;
const knotRadius = 7;

interface Composition {
  width: number;
  height: number;
  spread: { x: number; y: number };
}

const wide: Composition = { width: 960, height: 320, spread: { x: 0.34, y: 0.3 } };
const narrow: Composition = { width: 420, height: 400, spread: { x: 0.27, y: 0.31 } };

interface Label {
  x: number;
  y: number;
  anchor: "start" | "middle" | "end";
  name: string;
  os: string;
  lost: boolean;
}

export class Frontispiece {
  private scene: InkScene | null = null;
  private composition: Composition | null = null;
  private drawn = new Map<string, string>();
  private shown: { world: WorldState; residents: ResidentNow[] } | null = null;
  private readonly canvas: HTMLCanvasElement;
  private readonly labels: HTMLElement;

  constructor(figure: HTMLElement) {
    figure.innerHTML = `<canvas></canvas><div class="frontispiece-labels"></div>`;
    this.canvas = figure.querySelector("canvas")!;
    this.labels = figure.querySelector<HTMLElement>(".frontispiece-labels")!;
    new ResizeObserver(() => {
      const fit = figure.clientWidth < 600 ? narrow : wide;
      if (fit === this.composition) return;
      this.composition = fit;
      figure.style.aspectRatio = `${fit.width} / ${fit.height}`;
      // A new shape is a new drawing: the scene starts over in it.
      this.scene?.dispose();
      this.scene = new InkScene(this.canvas, { seed: "world", viewBox: [fit.width, fit.height] });
      this.drawn.clear();
      this.draw();
    }).observe(figure);
  }

  update(world: WorldState, residents: ResidentNow[]): void {
    this.shown = { world, residents };
    this.draw();
  }

  private draw(): void {
    const { scene, composition, shown } = this;
    if (!scene || !composition || !shown) return;
    const first = this.drawn.size === 0;
    const { strokes, labels } = compose(shown.world, shown.residents, composition);
    const wanted = new Set<string>();
    for (const [id, stroke] of strokes) {
      wanted.add(id);
      const signature = JSON.stringify(stroke);
      const was = this.drawn.get(id);
      if (was === signature) continue;
      if (was === undefined) scene.add(id, stroke, { drawIn: first ? 1400 : 700 });
      // A style is a patch on the one before: given whole, nothing stale stays.
      else scene.update(id, { path: stroke.path, style: { ...defaultStyle, ...stroke.style } });
      this.drawn.set(id, signature);
    }
    for (const id of [...this.drawn.keys()]) {
      if (wanted.has(id)) continue;
      scene.delete(id, { fadeOut: 600 });
      this.drawn.delete(id);
    }
    this.labels.innerHTML = labels
      .map((label) => {
        const left = (label.x / composition.width) * 100;
        const top = (label.y / composition.height) * 100;
        return `<span class="label ${label.anchor}${label.lost ? " lost" : ""}" style="left:${left.toFixed(2)}%;top:${top.toFixed(2)}%"><span class="name">${escapeHtml(label.name)}</span>${label.os ? `<span class="os">${escapeHtml(label.os)}</span>` : ""}</span>`;
      })
      .join("");
  }
}

function compose(world: WorldState, residents: ResidentNow[], fit: Composition): { strokes: Map<string, InkStroke>; labels: Label[] } {
  const strokes = new Map<string, InkStroke>();
  const labels: Label[] = [];
  const cx = fit.width / 2;
  const cy = fit.height / 2;
  const nodes = world.nodes;

  strokes.set("knot", {
    path: paths.circle([cx, cy], knotRadius),
    style: { color: "var(--text-2)", fibers: 22, width: 0.6, spread: 2.4, opacity: 0.18, roughness: 0.7 },
  });

  nodes.forEach((node, index) => {
    // Evenly round an ellipse from upper left; wide, as the drawing is.
    const angle = Math.PI * 1.1 + (index / nodes.length) * Math.PI * 2;
    const x = cx + Math.cos(angle) * fit.width * fit.spread.x;
    const y = cy + Math.sin(angle) * fit.height * fit.spread.y;
    const online = node.online === true;
    const busy = residents.some((r) => r.about.machine === node.name && r.now?.activity === "busy");

    // The line runs from the ring's edge to the knot's, a little bowed.
    const dx = cx - x;
    const dy = cy - y;
    const length = Math.hypot(dx, dy);
    const start: Point = [x + (dx / length) * (ringRadius + 5), y + (dy / length) * (ringRadius + 5)];
    const end: Point = [cx - (dx / length) * (knotRadius + 5), cy - (dy / length) * (knotRadius + 5)];
    const bow = (seeded(node.name)() - 0.5) * 0.22 * length;
    const control: Point = [
      (start[0] + end[0]) / 2 + (-dy / length) * bow,
      (start[1] + end[1]) / 2 + (dx / length) * bow,
    ];
    const line = paths.quadratic(start, control, end);
    strokes.set(`link/${node.name}`, {
      path: line,
      style: online
        ? { color: "var(--text-3)", fibers: 16, width: 0.5, spread: 2.4, opacity: 0.14, roughness: 0.8 }
        : { color: "var(--text-3)", fibers: 8, width: 0.5, spread: 1.2, opacity: 0.12, dash: { length: 2, gap: 6, speed: 0 } },
    });
    if (online && busy) {
      strokes.set(`flow/${node.name}`, {
        path: line,
        style: {
          color: "var(--accent)",
          fibers: 8,
          width: 0.75,
          spread: 1.1,
          opacity: 0.42,
          dash: { length: 5, gap: 13, speed: 16 },
        },
      });
    }

    strokes.set(`ring/${node.name}`, {
      path: paths.circle([x, y], ringRadius),
      style: online
        ? { color: "var(--text-2)", fibers: 34, width: 0.6, spread: 3.4, opacity: 0.13, roughness: 0.9 }
        : { color: "var(--text-3)", fibers: 14, width: 0.55, spread: 1.8, opacity: 0.13, dash: { length: 3, gap: 4, speed: 0 } },
    });
    if (node.name === world.here) {
      strokes.set(`here/${node.name}`, {
        path: paths.circle([x, y], 2.2),
        style: { color: "var(--text-2)", fibers: 14, width: 0.7, spread: 2.2, opacity: 0.3 },
      });
    }

    // Agents sit on the side away from the knot, spread along the ring.
    const outward = Math.atan2(-dy, -dx);
    const on = residents.filter((r) => r.about.machine === node.name && r.now?.activity !== "asleep");
    on.forEach((resident, place) => {
      const at = outward + (place - (on.length - 1) / 2) * 0.42 - 0.9;
      const mark: Point = [x + Math.cos(at) * (ringRadius + 12), y + Math.sin(at) * (ringRadius + 12)];
      const id = `agent/${node.name}/${resident.about.id}`;
      if (resident.now?.waiting_for_user) {
        strokes.set(id, {
          path: paths.circle(mark, 5),
          style: { color: "var(--accent)", fibers: 12, width: 0.7, spread: 1.2, opacity: 0.55, dash: { length: 3, gap: 3, speed: 8 } },
        });
      } else if (resident.now?.activity === "busy") {
        strokes.set(id, {
          path: paths.circle(mark, 1.8),
          style: { color: "var(--accent)", fibers: 30, width: 0.8, spread: 5.2, opacity: 0.3, roughness: 1.2, jitter: 0.7, jitterSpeed: 0.45 },
        });
      } else {
        strokes.set(id, {
          path: paths.circle(mark, 3.2),
          style: { color: "var(--text-3)", fibers: 10, width: 0.55, spread: 1.2, opacity: 0.22 },
        });
      }
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
  return { strokes, labels };
}

/** A system as people write it: Windows, Linux, macOS. */
export function osName(os: string): string {
  return { windows: "Windows", linux: "Linux", macos: "macOS" }[os] ?? os;
}

/** A small deterministic random source, seeded by a name. */
function seeded(name: string): () => number {
  let state = 2166136261;
  for (const char of name) state = Math.imul(state ^ char.charCodeAt(0), 16777619);
  return () => {
    state = Math.imul(state ^ (state >>> 15), 2246822507);
    state = Math.imul(state ^ (state >>> 13), 3266489909);
    state ^= state >>> 16;
    return (state >>> 0) / 4294967296;
  };
}

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (char) => `&#${char.charCodeAt(0)};`);
}
