// The interface's bar: how much of a whole. A grey track round at both
// ends, filled from its start, the fill round where it stops; parts of
// several kinds stand end to end with a sliver of the track between them.
// It is the plain bar, drawn by a careful hand and no more than that: its
// edges are a fraction of a pixel off straight, its ends a little off the
// compass's, and its ink has the pencil's grain (`pen-bar`, `filters.ts`).
// No marker's stroke and no line under it (the user, 2026-11-16).
//
// Seeded, so the same bar is always drawn the same way. A bar is drawn at
// the width it is shown at, since ends stretched would squash: `bar` gives
// an element that draws itself there and again whenever its width changes,
// so no page has to measure for it.

import "./bar.css";
import { seeded } from "./shapes";

const f = (n: number) => (Math.round(n * 100) / 100).toString();

/** How thick a bar is where nothing says. */
export const BAR = 8;
/** The sliver of track between two parts. */
const GAP = 1.5;
/** The shortest part still drawn. */
const SHORTEST = 1;
/** How much of a quarter circle its control points take. */
const KAPPA = 0.5523;

export interface BarPart {
  /** How much of the whole it is. */
  share: number;
  /** Its ink: a class its path carries, or a colour. */
  className?: string;
  fill?: string;
}

export interface BarDrawn {
  /** The track's outline. */
  track: string;
  /** Each part drawn, with where along the bar it stands. */
  fills: { d: string; part: BarPart; from: number; to: number }[];
}

type Edge = (x: number) => number;

/** How an end is rounded: how far it reaches, how full its corners are,
 *  and how far off the middle its tip sits. */
interface Cap {
  reach: number;
  full: number;
  tip: number;
}

/** What a hand drifts by along an edge: a slow wave and a quicker one over
 *  it, `amount` at most. */
function drift(random: () => number, amount: number): Edge {
  const slow = 70 + random() * 50;
  const quick = 24 + random() * 16;
  const a = random() * Math.PI * 2;
  const b = random() * Math.PI * 2;
  return (x) => amount * (0.65 * Math.sin((x / slow) * Math.PI * 2 + a) + 0.35 * Math.sin((x / quick) * Math.PI * 2 + b));
}

function capOf(random: () => number, radius: number, thick: number): Cap {
  return { reach: radius * (0.97 + random() * 0.07), full: KAPPA * (0.95 + random() * 0.1), tip: (random() - 0.5) * thick * 0.05 };
}

/** The hand that draws along `middle`, `thick` across: its two edges, each
 *  drifting by a fraction of a pixel, and the round ends it makes. */
interface Hand {
  radius: number;
  /** A round end of its own. */
  cap(): Cap;
  /** The outline from `a` to `b`: each end round as `start` and `end` say,
   *  or cut straight where there is none. */
  outline(a: number, b: number, start: Cap | null, end: Cap | null): string;
}

function hand(middle: number, thick: number, random: () => number): Hand {
  const radius = thick / 2;
  const wander = Math.min(0.5, thick * 0.055);
  const over = drift(random, wander);
  const under = drift(random, wander);
  const top: Edge = (x) => middle - radius + over(x);
  const bottom: Edge = (x) => middle + radius + under(x);
  /** The points of an edge from `a` to `b`, every few pixels. */
  const along = (edge: Edge, a: number, b: number): string[] => {
    const steps = Math.max(1, Math.round((b - a) / 9));
    const points: string[] = [];
    for (let index = 0; index <= steps; index += 1) {
      const x = a + ((b - a) * index) / steps;
      points.push(`${f(x)} ${f(edge(x))}`);
    }
    return points;
  };
  /** A round end at `x`, from the edge left to the other: `side` +1 turns
   *  right of the bar, -1 left of it. */
  const round = (x: number, side: 1 | -1, cap: Cap): string => {
    const [from, to] = side === 1 ? [top(x), bottom(x)] : [bottom(x), top(x)];
    const tip = (from + to) / 2 + cap.tip;
    const out = x + side * cap.reach;
    const near = x + side * cap.reach * cap.full;
    return `C${f(near)} ${f(from)} ${f(out)} ${f(tip - (tip - from) * cap.full)} ${f(out)} ${f(tip)}C${f(out)} ${f(tip + (to - tip) * cap.full)} ${f(near)} ${f(to)} ${f(x)} ${f(to)}`;
  };
  return {
    radius,
    cap: () => capOf(random, radius, thick),
    outline(a, b, start, end) {
      const from = start ? Math.min(a + radius, b) : a;
      const to = end ? Math.max(b - radius, from) : b;
      const upper = along(top, from, to);
      const lower = along(bottom, from, to).reverse();
      return `M${upper.join("L")}${end ? round(to, 1, end) : `L${lower[0]}`}L${lower.join("L")}${start ? round(from, -1, start) : ""}Z`;
    },
  };
}

/** A bar `width` across in a box `height` tall, `thick` itself: its track,
 *  and `parts` laid from its start, each as long as its share. The last
 *  part ends round where it stops short of the track's end. */
export function drawBar(width: number, height: number, parts: BarPart[], random: () => number, thick = Math.min(BAR, height)): BarDrawn {
  const x0 = 0.5;
  const x1 = Math.max(x0 + thick, width - 0.5);
  const pen = hand(height / 2, thick, random);
  const starts = pen.cap();
  const ends = pen.cap();
  const drawn: BarDrawn = { track: pen.outline(x0, x1, starts, ends), fills: [] };
  const length = x1 - x0;
  const shown = parts.filter((part) => part.share > 0);
  let at = 0;
  shown.forEach((part, index) => {
    const first = index === 0;
    const last = index === shown.length - 1;
    const from = at;
    const to = Math.min(1, at + part.share);
    at = to;
    const a = x0 + length * from + (first ? 0 : GAP / 2);
    let b = x0 + length * to - (last ? 0 : GAP / 2);
    // A part alone and short is a dot of the bar's own thickness.
    if (first && last) b = Math.max(b, a + thick);
    if (b - a < SHORTEST) return;
    const whole = b >= x1 - 0.25;
    const end = !last ? null : whole ? ends : pen.cap();
    drawn.fills.push({ d: pen.outline(a, Math.min(b, x1), first ? starts : null, end), part, from, to });
  });
  return drawn;
}

/** A stretch from `x0` to `x1` along `y`, `thick` across and round at both
 *  ends, on no track: a span of time on a timeline, a short measure beside
 *  a figure. One shorter than it is thick is a dot. */
export function pill(x0: number, x1: number, y: number, thick: number, random: () => number): string {
  const pen = hand(y, thick, random);
  return pen.outline(x0, Math.max(x1, x0 + thick), pen.cap(), pen.cap());
}

/** The bar's marks, to stand in an `<svg>` as wide and as tall as it was
 *  drawn for. */
export function barMarks(drawn: BarDrawn): string {
  const fills = drawn.fills
    .map(({ d, part }) => `<path class="bar-ink${part.className ? ` ${part.className}` : ""}"${part.fill ? ` style="fill:${part.fill}"` : ""} d="${d}"/>`)
    .join("");
  return `<path class="bar-track" d="${drawn.track}"/><g class="bar-fill" filter="url(#pen-bar)">${fills}</g>`;
}

/** What a bar is of, as its element carries it. */
export interface BarSpec {
  /** Its parts from its start; `said` is what one says when pointed at. */
  parts: (BarPart & { said?: string })[];
  seed: string;
  /** How thick, where not the usual; never more than its element is tall. */
  thick?: number;
}

const attribute = (text: string) => text.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** A bar as markup: `<hand-bar>`, as wide and as tall as its styles make
 *  it, drawn to that. `className` names it for those styles; `more` is
 *  further attributes, each led by a space. */
export function bar(spec: BarSpec, className = "", more = ""): string {
  return `<hand-bar class="hand-bar${className ? ` ${className}` : ""}" data-bar="${attribute(JSON.stringify(spec))}" aria-hidden="true"${more}></hand-bar>`;
}

const sized =
  typeof ResizeObserver === "undefined"
    ? null
    : new ResizeObserver((entries) => {
        for (const entry of entries) (entry.target as HandBar).fit(entry.contentRect.width, entry.contentRect.height);
      });

class HandBar extends HTMLElement {
  /** The size it is drawn at. */
  private drawn = "";

  static get observedAttributes(): string[] {
    return ["data-bar"];
  }

  connectedCallback(): void {
    sized?.observe(this);
  }

  disconnectedCallback(): void {
    sized?.unobserve(this);
  }

  attributeChangedCallback(): void {
    this.drawn = "";
    if (this.isConnected) this.fit(this.clientWidth, this.clientHeight);
  }

  fit(wide: number, tall: number): void {
    const width = Math.round(wide);
    const height = Math.round(tall);
    if (width < 4 || height < 2) return;
    const size = `${width}x${height}`;
    if (size === this.drawn) return;
    let spec: BarSpec;
    try {
      spec = JSON.parse(this.dataset.bar ?? "") as BarSpec;
    } catch {
      return;
    }
    this.drawn = size;
    const drawn = drawBar(width, height, spec.parts, seeded(`bar:${spec.seed}`), Math.min(spec.thick ?? BAR, height));
    const hits = drawn.fills
      .map(({ part, from, to }) => {
        const said = (part as BarSpec["parts"][number]).said;
        return said ? `<rect x="${f(from * width)}" y="0" width="${f((to - from) * width)}" height="${height}" fill="transparent" title="${attribute(said)}"/>` : "";
      })
      .join("");
    this.innerHTML = `<svg width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">${barMarks(drawn)}${hits}</svg>`;
  }
}

if (typeof customElements !== "undefined" && !customElements.get("hand-bar")) customElements.define("hand-bar", HandBar);
