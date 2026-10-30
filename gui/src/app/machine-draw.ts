// The drawings of a machine's page: a ring the pen draws round as far as a
// share goes, the trace of a rate over the last minutes, a disk's bar, and
// the strip of a line's days. The paper stays still and the ink moves on
// it: each is drawn once in place and only its marks change with a
// reading. Seeded, so the same value is always drawn the same way; text is
// never drawn by the pen and stays sharp.

import { blot, circle, dashed, outline, ruled, seeded, type Point } from "../pen/shapes";

const f = (n: number) => (Math.round(n * 10) / 10).toString();

/** Whether the interface keeps still (`motion.ts`). */
const still = () => document.documentElement.hasAttribute("data-reduced");

/** How full a share is, for its colour: past nine tenths it warns, at the
 *  top it is spent. */
export type Level = "" | "high" | "full";

export function levelOf(share: number | null): Level {
  return share === null ? "" : share >= 0.98 ? "full" : share >= 0.9 ? "high" : "";
}

const RING_SIZE = 72;
const RING_RADIUS = 28;

/** A share drawn as a ring: a pencil circle, and the clay arc of the share
 *  round it from the top, the number in its middle. A new share is drawn
 *  on from the old one. */
export class Ring {
  readonly element: HTMLElement;
  private readonly arc: SVGPathElement;
  private readonly markPath: SVGPathElement;
  private marked: number | null = null;
  private readonly number: HTMLElement;
  private shown = 0;
  private target = 0;
  private frame = 0;

  constructor(private readonly seed: string) {
    this.element = document.createElement("div");
    this.element.className = "mp-ring";
    const track = circle([RING_SIZE / 2, RING_SIZE / 2], RING_RADIUS, 1.3, seeded(`${seed}:track`));
    this.element.innerHTML = `<svg viewBox="0 0 ${RING_SIZE} ${RING_SIZE}" aria-hidden="true"><path class="mp-ring-track" d="${track}"/><g filter="url(#pen-marker-lite)"><path class="mp-ring-arc"/></g><path class="mp-ring-mark"/></svg><span class="mp-ring-number"></span>`;
    this.arc = this.element.querySelector<SVGPathElement>(".mp-ring-arc")!;
    this.markPath = this.element.querySelector<SVGPathElement>(".mp-ring-mark")!;
    this.number = this.element.querySelector<HTMLElement>(".mp-ring-number")!;
  }

  /** Shows `share` (0 to 1, null for not read) with `said` in the middle. */
  set(share: number | null, said: string): void {
    const figure = `<span class="mp-ring-figure">${said}</span>`;
    if (this.number.innerHTML !== figure) this.number.innerHTML = figure;
    this.element.dataset.level = levelOf(share);
    this.element.classList.toggle("unread", share === null);
    const target = Math.max(0, Math.min(1, share ?? 0));
    if (target === this.target && !this.frame) return;
    this.target = target;
    cancelAnimationFrame(this.frame);
    this.frame = 0;
    if (still() || !this.element.isConnected) {
      this.shown = target;
      this.draw(target);
      return;
    }
    const from = this.shown;
    const began = performance.now();
    const step = (now: number) => {
      const t = Math.min(1, (now - began) / 520);
      const eased = 1 - (1 - t) ** 3;
      this.shown = from + (target - from) * eased;
      this.draw(this.shown);
      this.frame = t < 1 ? requestAnimationFrame(step) : 0;
    };
    this.frame = requestAnimationFrame(step);
  }

  private draw(share: number): void {
    this.arc.setAttribute("d", ringArc(share, this.seed));
  }

  /** A pencil tick across the ring at `share`, where a line the share must
   *  not pass stands; none with null. */
  mark(share: number | null): void {
    if (share === this.marked) return;
    this.marked = share;
    if (share === null) {
      this.markPath.removeAttribute("d");
      return;
    }
    const angle = -Math.PI / 2 + Math.max(0, Math.min(1, share)) * Math.PI * 2;
    const at = (radius: number): Point => [RING_SIZE / 2 + Math.cos(angle) * radius, RING_SIZE / 2 + Math.sin(angle) * radius];
    this.markPath.setAttribute("d", ruled(at(RING_RADIUS - 6), at(RING_RADIUS + 6), 1.4, seeded(`${this.seed}:mark`)));
  }

  dispose(): void {
    cancelAnimationFrame(this.frame);
  }
}

/** The arc of `share` round the ring, clockwise from the top: a stroke that
 *  lands a little heavy and lifts thin; a speck where there is almost
 *  none. */
function ringArc(share: number, seed: string): string {
  const center: Point = [RING_SIZE / 2, RING_SIZE / 2];
  const random = seeded(`${seed}:arc`);
  if (share < 0.012) return share > 0 ? blot([center[0], center[1] - RING_RADIUS], 2, random) : "";
  const sweep = share * Math.PI * 2;
  const steps = Math.max(8, Math.round((sweep * RING_RADIUS) / 2.5));
  const points: Point[] = [];
  for (let index = 0; index <= steps; index += 1) {
    const angle = -Math.PI / 2 + (sweep * index) / steps;
    points.push([center[0] + Math.cos(angle) * RING_RADIUS, center[1] + Math.sin(angle) * RING_RADIUS]);
  }
  return outline(points, 5.2, random, { pressure: 0.2, taperStart: 0.03, taperEnd: Math.min(0.2, 0.05 / share) });
}

/** One trace on a drawing of rates: moments and values (null for a
 *  reading that had none), up to `max`. */
export interface Trace {
  points: [number, number | null][];
  max: number;
  kind: "clay" | "graphite";
  /** Washed in underneath. */
  area: boolean;
  /** Readings further apart than this break the trace: the page was away,
   *  or the machine did not answer. A few times how often it is read. */
  gap: number;
}

/** The traces of rates over a span of time, drawn at the size the page
 *  gives them, so a line is as thick at any width. */
export class Spark {
  readonly element: HTMLElement;
  private readonly svg: SVGSVGElement;
  private width = 0;
  private height = 0;
  private traces: Trace[] = [];
  private limit: number | null = null;
  private from = 0;
  private to = 1;
  private readonly sized: ResizeObserver;

  constructor(className = "") {
    this.element = document.createElement("div");
    this.element.className = `mp-spark${className ? ` ${className}` : ""}`;
    this.element.innerHTML = `<svg aria-hidden="true"><g class="mp-spark-ink" filter="url(#pen-marker-lite)"></g><g class="mp-spark-rule"></g></svg>`;
    this.svg = this.element.querySelector("svg")!;
    this.sized = new ResizeObserver(([entry]) => {
      const box = entry!.contentRect;
      if (Math.abs(box.width - this.width) < 0.5 && Math.abs(box.height - this.height) < 0.5) return;
      this.width = box.width;
      this.height = box.height;
      this.render();
    });
    this.sized.observe(this.element);
  }

  /** Draws `traces` from `from` to `to`, with a dashed line at `limit`, a
   *  value on the first trace's scale that it should stay under, where one
   *  stands. */
  draw(traces: Trace[], from: number, to: number, limit: number | null = null): void {
    this.traces = traces;
    this.limit = limit;
    this.from = from;
    this.to = to;
    this.render();
  }

  private render(): void {
    const { width, height } = this;
    if (width < 2 || height < 2) return;
    this.svg.setAttribute("viewBox", `0 0 ${f(width)} ${f(height)}`);
    const pad = 3;
    const x = (at: number) => ((at - this.from) / (this.to - this.from)) * width;
    const html = this.traces
      .map((trace) => {
        const y = (value: number) => height - pad - (Math.min(value, trace.max) / (trace.max || 1)) * (height - 2 * pad);
        // A reading just before the span starts the trace at its left edge.
        const runs: Point[][] = [];
        let run: Point[] = [];
        let last = -Infinity;
        for (const [at, value] of trace.points) {
          if (at < this.from - trace.gap) continue;
          if ((value === null || at - last > trace.gap) && run.length) {
            runs.push(run);
            run = [];
          }
          if (value === null) {
            last = -Infinity;
            continue;
          }
          run.push([x(at), y(value)]);
          last = at;
        }
        if (run.length) runs.push(run);
        const lines = runs.map((points) => (points.length === 1 ? `M${f(points[0]![0])} ${f(points[0]![1])}h0.1` : `M${points.map(([px, py]) => `${f(px)} ${f(py)}`).join("L")}`));
        const areas = trace.area
          ? runs.filter((points) => points.length > 1).map((points) => `M${f(points[0]![0])} ${f(height)}L${points.map(([px, py]) => `${f(px)} ${f(py)}`).join("L")}L${f(points[points.length - 1]![0])} ${f(height)}Z`)
          : [];
        return `${areas.length ? `<path class="mp-spark-area ${trace.kind}" d="${areas.join("")}"/>` : ""}<path class="mp-spark-line ${trace.kind}" d="${lines.join("")}"/>`;
      })
      .join("");
    // A dotted rule along the foot: the paper the trace is drawn on, so a
    // trace just begun reads as begun, not as missing.
    const first = this.traces[0];
    const limitY = first && this.limit !== null ? height - pad - (Math.min(this.limit, first.max) / (first.max || 1)) * (height - 2 * pad) : null;
    const base = `<line class="mp-spark-base" x1="0" y1="${f(height - pad)}" x2="${f(width)}" y2="${f(height - pad)}"/>${
      limitY === null ? "" : `<path class="mp-spark-limit" d="${dashed([2, limitY], [width - 2, limitY], 1, seeded(`limit:${f(limitY)}`), 4, 4)}"/>`
    }`;
    const ink = this.svg.firstElementChild!;
    if (ink.innerHTML !== html) ink.innerHTML = html;
    const rule = this.svg.querySelector(".mp-spark-rule")!;
    if (rule.innerHTML !== base) rule.innerHTML = base;
  }

  dispose(): void {
    this.sized.disconnect();
  }
}

/** A share of a disk, as a bar drawn with a broad marker over a pencil
 *  line the length of the whole. */
export function diskBar(share: number, seed: string): string {
  const width = 240;
  const random = seeded(`disk:${seed}`);
  const whole = ruled([3, 9], [width - 3, 9], 1, random);
  const end = 3 + (width - 6) * Math.max(0, Math.min(1, share));
  const points: Point[] = [];
  for (let x = 3; x < end; x += 8) points.push([x, 9 + Math.sin(x / 19) * 0.5]);
  points.push([end, 9]);
  const used = share > 0.004 ? outline(points, 9, random, { pressure: 0.14, taperStart: 0.02, taperEnd: 0.03 }) : "";
  return `<svg class="mp-disk-bar" viewBox="0 0 ${width} 18" preserveAspectRatio="none" aria-hidden="true" data-level="${levelOf(share)}"><path class="mp-disk-whole" d="${whole}"/><g filter="url(#pen-marker-lite)"><path class="mp-disk-used" d="${used}"/></g></svg>`;
}

/** A stretch of a line's days. */
export interface LineSpan {
  kind: "up" | "down" | "none";
  from: number;
  to: number;
  title: string;
}

/** A moment on a line's strip: a drop, or the node starting. */
export interface LineMark {
  kind: "drop" | "start";
  at: number;
  title: string;
}

/** A line's days as a strip: up is a firm ruled stroke, down is the bare
 *  paper with a red speck where it dropped, a start a short upright tick;
 *  what was not recorded is a faint dotted rule. Each stretch says what it
 *  was when pointed at. */
export class Strip {
  readonly element: HTMLElement;
  private readonly svg: SVGSVGElement;
  private readonly hits: HTMLElement;
  private width = 0;
  private spans: LineSpan[] = [];
  private marks: LineMark[] = [];
  private from = 0;
  private to = 1;
  private readonly sized: ResizeObserver;

  constructor(private readonly seed: string) {
    this.element = document.createElement("div");
    this.element.className = "mp-strip";
    this.element.innerHTML = `<svg aria-hidden="true"></svg><div class="mp-strip-hits"></div>`;
    this.svg = this.element.querySelector("svg")!;
    this.hits = this.element.querySelector<HTMLElement>(".mp-strip-hits")!;
    this.sized = new ResizeObserver(([entry]) => {
      const width = entry!.contentRect.width;
      if (Math.abs(width - this.width) < 0.5) return;
      this.width = width;
      this.render();
    });
    this.sized.observe(this.element);
  }

  draw(spans: LineSpan[], marks: LineMark[], from: number, to: number): void {
    this.spans = spans;
    this.marks = marks;
    this.from = from;
    this.to = to;
    this.render();
  }

  private render(): void {
    const width = this.width;
    if (width < 2) return;
    const height = 22;
    const middle = 11;
    const x = (at: number) => Math.max(0, Math.min(width, ((at - this.from) / (this.to - this.from)) * width));
    const random = seeded(`strip:${this.seed}:${Math.round(width)}`);
    const up = this.spans
      .filter((span) => span.kind === "up" && x(span.to) - x(span.from) >= 0.6)
      .map((span) => ruled([x(span.from), middle], [Math.max(x(span.from) + 0.6, x(span.to)), middle], 6, random));
    const drops = this.marks.filter((mark) => mark.kind === "drop").map((mark) => blot([x(mark.at), middle], 2.6, random));
    const starts = this.marks.filter((mark) => mark.kind === "start").map((mark) => ruled([x(mark.at), middle - 7], [x(mark.at), middle + 7], 1.3, random));
    this.svg.setAttribute("viewBox", `0 0 ${f(width)} ${height}`);
    this.svg.innerHTML = `<line class="mp-strip-rule" x1="0" y1="${middle}" x2="${f(width)}" y2="${middle}"/><g filter="url(#pen-rule-lite)"><path class="mp-strip-up" d="${up.join("")}"/><path class="mp-strip-start" d="${starts.join("")}"/></g><g filter="url(#pen-marker-lite)"><path class="mp-strip-drop" d="${drops.join("")}"/></g>`;
    // Each stretch wide enough to point at says what it was; a moment, too
    // short to point at on its own, has a few pixels round its mark.
    const percent = (at: number) => `${((x(at) / width) * 100).toFixed(3)}%`;
    const stretches = this.spans
      .filter((span) => span.kind !== "none" && x(span.to) - x(span.from) >= 0.5)
      .map((span) => `<span class="mp-strip-hit ${span.kind}" style="left:${percent(span.from)};width:${(((x(span.to) - x(span.from)) / width) * 100).toFixed(3)}%" title="${escapeAttribute(span.title)}"></span>`);
    const moments = this.marks.map((mark) => `<span class="mp-strip-hit mark ${mark.kind}" style="left:calc(${percent(mark.at)} - 5px)" title="${escapeAttribute(mark.title)}"></span>`);
    this.hits.innerHTML = [...stretches, ...moments].join("");
  }

  dispose(): void {
    this.sized.disconnect();
  }
}

function escapeAttribute(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}
