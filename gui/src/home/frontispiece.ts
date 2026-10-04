// The front page's drawing: the World as it stands. Each machine a ring of
// ink around the knot where they all meet, the agents on it as marks on its
// far side (filled while at work), and the line to the knot flowing while
// something on that machine is busy. A machine out of reach is drawn faint
// and broken. Lines are a few slightly unsteady strands, seeded by name, so
// the same World is always the same drawing.

import type { ResidentNow } from "../world/world";
import type { WorldState } from "./model";

const ringRadius = 24;
const knotRadius = 7;

/** Draws the World into `figure`, and again whenever the figure changes
 *  between a wide and a narrow shape: a phone gets a taller composition
 *  rather than the wide one shrunk. */
export function drawFrontispiece(figure: HTMLElement, world: WorldState, residents: ResidentNow[]): void {
  let narrow: boolean | null = null;
  const observer = new ResizeObserver(() => {
    const now = figure.clientWidth < 600;
    if (now === narrow) return;
    narrow = now;
    draw(figure, world, residents, now);
  });
  observer.observe(figure);
}

function draw(figure: HTMLElement, world: WorldState, residents: ResidentNow[], narrow: boolean): void {
  const width = narrow ? 420 : 960;
  const height = narrow ? 400 : 320;
  const spread = narrow ? { x: 0.27, y: 0.31 } : { x: 0.34, y: 0.3 };
  const cx = width / 2;
  const cy = height / 2;
  const nodes = world.nodes;
  const links: string[] = [];
  const rings: string[] = [];
  const labels: string[] = [];

  nodes.forEach((node, index) => {
    // Evenly round an ellipse from upper left; the ellipse is wide, as the
    // drawing is, so machines keep their distance on a phone too.
    const angle = Math.PI * 1.1 + (index / nodes.length) * Math.PI * 2;
    const ux = Math.cos(angle);
    const uy = Math.sin(angle);
    const x = cx + ux * width * spread.x;
    const y = cy + uy * height * spread.y;
    const online = node.online === true;
    const busy = residents.some((r) => r.about.machine === node.name && r.now?.activity === "busy");

    // The line runs from the ring's edge to the knot's, a little bowed.
    const dx = cx - x;
    const dy = cy - y;
    const length = Math.hypot(dx, dy);
    const sx = x + (dx / length) * (ringRadius + 5);
    const sy = y + (dy / length) * (ringRadius + 5);
    const ex = cx - (dx / length) * (knotRadius + 5);
    const ey = cy - (dy / length) * (knotRadius + 5);
    const random = seeded(node.name);
    const bow = (random() - 0.5) * 0.22 * length;
    const mx = (sx + ex) / 2 + (-dy / length) * bow;
    const my = (sy + ey) / 2 + (dx / length) * bow;
    for (let strand = 0; strand < 3; strand += 1) {
      const jitter = () => (random() - 0.5) * 2;
      links.push(
        `<path class="link${online ? "" : " lost"}" d="M${f(sx + jitter())} ${f(sy + jitter())} Q${f(mx + jitter() * 3)} ${f(my + jitter() * 3)} ${f(ex + jitter())} ${f(ey + jitter())}"/>`,
      );
    }
    if (online && busy) links.push(`<path class="flow" d="M${f(sx)} ${f(sy)} Q${f(mx)} ${f(my)} ${f(ex)} ${f(ey)}"/>`);

    const ringRandom = seeded(`ring:${node.name}`);
    for (let strand = 0; strand < 4; strand += 1) {
      rings.push(`<path class="ring${online ? "" : " lost"}" d="${ring(x, y, ringRadius + strand * 0.9, ringRandom)}"/>`);
    }
    if (node.name === world.here) rings.push(`<circle class="here" cx="${f(x)}" cy="${f(y)}" r="3.2"/>`);

    // Agents sit on the side away from the knot, spread along the ring.
    const outward = Math.atan2(-dy, -dx);
    const on = residents.filter((r) => r.about.machine === node.name && r.now?.activity !== "asleep");
    on.forEach((resident, index) => {
      const at = outward + (index - (on.length - 1) / 2) * 0.42 - 0.9;
      const state = resident.now?.waiting_for_user ? "waiting" : resident.now?.activity === "busy" ? "busy" : "idle";
      rings.push(
        `<circle class="mark ${state}" cx="${f(x + Math.cos(at) * (ringRadius + 12))}" cy="${f(y + Math.sin(at) * (ringRadius + 12))}" r="${state === "idle" ? 3.3 : 4.2}"/>`,
      );
    });

    // The name stands off the far side of the ring, read away from the knot.
    const lx = x + Math.cos(outward) * (ringRadius + 18);
    const ly = y + Math.sin(outward) * (ringRadius + 18);
    const anchor = Math.abs(Math.cos(outward)) < 0.35 ? "middle" : Math.cos(outward) > 0 ? "start" : "end";
    const below = Math.sin(outward) > 0.35;
    const above = Math.sin(outward) < -0.35;
    const nameY = above ? ly - 14 : below ? ly + 16 : ly + 4;
    const os = "Ok" in node.facts ? node.facts.Ok.os : "";
    labels.push(
      `<text class="name${online ? "" : " lost"}" x="${f(lx)}" y="${f(nameY)}" text-anchor="${anchor}">${escapeXml(node.name)}</text>`,
    );
    if (os) {
      labels.push(`<text class="os" x="${f(lx)}" y="${f(nameY + 18)}" text-anchor="${anchor}">${escapeXml(os)}</text>`);
    }
  });

  // The knot where every machine's line meets.
  const knot = seeded("knot");
  for (let strand = 0; strand < 4; strand += 1) rings.push(`<path class="knot" d="${ring(cx, cy, knotRadius + strand * 0.6, knot)}"/>`);

  figure.innerHTML = `<svg viewBox="0 0 ${width} ${height}" preserveAspectRatio="xMidYMid meet" role="img">${links.join("")}${rings.join("")}${labels.join("")}</svg>`;
}

const f = (n: number) => n.toFixed(1);

/** A hand-drawn circle: a closed curve through points a little off true. */
function ring(x: number, y: number, r: number, random: () => number): string {
  const count = 14;
  const start = random() * Math.PI * 2;
  const points = Array.from({ length: count }, (_, index) => {
    const angle = start + (index / count) * Math.PI * 2;
    const radius = r * (1 + (random() - 0.5) * 0.06);
    return [x + Math.cos(angle) * radius, y + Math.sin(angle) * radius] as const;
  });
  // Catmull-Rom through the points, closed.
  let d = `M${f(points[0][0])} ${f(points[0][1])}`;
  for (let index = 0; index < count; index += 1) {
    const p0 = points[(index - 1 + count) % count];
    const p1 = points[index];
    const p2 = points[(index + 1) % count];
    const p3 = points[(index + 2) % count];
    const c1 = [p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6];
    const c2 = [p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6];
    d += ` C${f(c1[0])} ${f(c1[1])} ${f(c2[0])} ${f(c2[1])} ${f(p2[0])} ${f(p2[1])}`;
  }
  return d;
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

function escapeXml(text: string): string {
  return text.replace(/[&<>"]/g, (char) => `&#${char.charCodeAt(0)};`);
}
