// A film written as a reel of timed marks (`world-gui.md`, 功能): pen
// strokes written along their line, washes of colour swept in as a marker
// sweeps, the interface's marks set down, groups that move, fold and fade.
// Times are milliseconds from the start; `film.ts` plays the reel on a
// loop. The reel is markup and data only: what moves, and when.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { blot as blotPath, circlePoints, outline, quadratic, ruledPoints, seeded, type Point } from "../pen/shapes";

/** The pens a stroke is drawn in; `add` and `del` are the colours a
 *  change's lines are shown in. */
export type Ink = "ink" | "graphite" | "clay" | "add" | "del";
/** The washes a surface is laid in, as the interface's own surfaces. */
export type Tone = "page" | "panel" | "raised" | "bubble" | "clay" | "add" | "del";

/** A pose of a group at time `t`: where it stands, its scale, turn and
 *  opacity. What a pose leaves out it keeps from the pose before. */
export interface Pose {
  t: number;
  x?: number;
  y?: number;
  sx?: number;
  sy?: number;
  r?: number;
  o?: number;
}

export interface Caption {
  at: number;
  text: string;
}

/** A reel ready to play: its drawing and its size, and how long it runs. */
export interface Built {
  markup: string;
  width: number;
  height: number;
  duration: number;
  captions: Caption[];
}

/** How a film is laid out: wide for a computer's page, tall for a phone's. */
export type Shape = "wide" | "tall";

const f = (n: number) => (Math.round(n * 10) / 10).toString();
let reels = 0;

export class Reel {
  private readonly random: () => number;
  private readonly prefix: string;
  private readonly defs: string[] = [];
  private out: string[] = [];
  private serial = 0;
  private readonly captions: Caption[] = [];

  constructor(
    seed: string,
    readonly width: number,
    readonly height: number,
  ) {
    this.random = seeded(seed);
    this.prefix = `fl${++reels}-`;
  }

  /** What is said under the film from `at` on. */
  say(at: number, text: string): void {
    this.captions.push({ at, text });
  }

  /** A stroke along `points`, written from the first to the last over
   *  `dur` (by default as fast as a hand writes it). */
  pen(points: Point[], width: number, ink: Ink, at: number, dur?: number, taper = 0.12): void {
    if (points.length < 2) return;
    let length = 0;
    for (let index = 1; index < points.length; index += 1) length += Math.hypot(points[index]![0] - points[index - 1]![0], points[index]![1] - points[index - 1]![1]);
    const took = dur ?? Math.min(900, Math.max(140, length / 1.5));
    const id = `${this.prefix}${++this.serial}`;
    const centre = `M${points.map((point) => `${f(point[0])} ${f(point[1])}`).join("L")}`;
    this.defs.push(
      `<mask id="${id}"><path d="${centre}" fill="none" stroke="#fff" stroke-width="${f(width * 2 + 8)}" stroke-linecap="round" stroke-linejoin="round" pathLength="1" stroke-dasharray="1 2" stroke-dashoffset="1"/></mask>`,
    );
    const shape = outline(points, width, this.random, { pressure: 0.25, taper });
    this.out.push(`<path class="fl-${ink}" d="${shape}" data-w="${Math.round(at)},${Math.round(at + took)}" data-m="${id}" visibility="hidden"/>`);
  }

  /** A line drawn against a straight edge. */
  line(from: Point, to: Point, width: number, ink: Ink, at: number, dur?: number): void {
    this.pen(ruledPoints(from, to, this.random), width, ink, at, dur, 0.04);
  }

  /** A curve from one point to another, bent toward `control`. */
  curve(from: Point, control: Point, to: Point, width: number, ink: Ink, at: number, dur?: number): void {
    this.pen(quadratic(from, control, to, 6), width, ink, at, dur);
  }

  /** A circle drawn by hand around `centre`. */
  ring(centre: Point, radius: number, width: number, ink: Ink, at: number, dur?: number): void {
    this.pen(circlePoints(centre, radius, this.random), width, ink, at, dur, 0.06);
  }

  /** A line of handwriting standing in for words: `length` long. */
  scribble(from: Point, length: number, ink: Ink, at: number, dur?: number, width = 3.2): void {
    const phase = this.random() * 6;
    const points: Point[] = [];
    for (let x = 0; x <= length; x += 7) points.push([from[0] + x, from[1] + Math.sin(x / 9 + phase) * 4 + Math.sin(x / 23 + phase) * 2]);
    this.pen(points, width, ink, at, dur ?? Math.min(1100, Math.max(160, length * 2.2)), 0.08);
  }

  /** A tick, as a hand checks a thing off. */
  check(at: Point, size: number, ink: Ink, when: number): void {
    const [x, y] = at;
    this.pen(
      [
        [x, y],
        [x + size * 0.35, y + size * 0.38],
        [x + size, y - size * 0.55],
      ],
      size * 0.16,
      ink,
      when,
      260,
      0.2,
    );
  }

  /** An arrow along a curve, its head at `to`. */
  arrow(from: Point, control: Point, to: Point, width: number, ink: Ink, at: number, dur = 600): void {
    const points = quadratic(from, control, to, 6);
    this.pen(points, width, ink, at, dur);
    const [px, py] = points[Math.max(0, points.length - 4)]!;
    const angle = Math.atan2(to[1] - py, to[0] - px);
    const wing = (turn: number): Point => [to[0] - Math.cos(angle + turn) * width * 6, to[1] - Math.sin(angle + turn) * width * 6];
    this.line(wing(0.5), to, width, ink, at + dur, 140);
    this.line(wing(-0.5), to, width, ink, at + dur + 80, 140);
  }

  /** A dashed line along `points`, its dashes drawn one after another. */
  dashes(points: Point[], every: number, width: number, ink: Ink, at: number, dur = 700): void {
    const marks = ticksAlong(points, every);
    marks.forEach((dash, index) => this.pen(dash, width, ink, at + (index / Math.max(1, marks.length)) * dur, 90, 0.2));
  }

  /** A drop of ink running along `points` from `at` for `dur`, fading in
   *  as it leaves and out as it arrives. */
  travel(points: Point[], radius: number, ink: Ink, at: number, dur: number): void {
    const [x0, y0] = points[0]!;
    const steps = 8;
    const poses: Pose[] = [{ t: at, x: 0, y: 0, o: 0 }];
    for (let index = 1; index <= steps; index += 1) {
      const point = points[Math.round((index / steps) * (points.length - 1))]!;
      poses.push({ t: at + (index / steps) * dur, x: point[0] - x0, y: point[1] - y0, o: index === steps ? 0 : 1 });
    }
    this.group([x0, y0], poses, () => this.out.push(`<path class="fl-${ink}" d="${blotPath([x0, y0], radius, this.random)}"/>`));
  }

  /** A blot of ink, set down at `at`. */
  blot(centre: Point, radius: number, ink: Ink, at: number): void {
    this.group(centre, land(at), () => this.out.push(`<path class="fl-${ink}" d="${blotPath(centre, radius, this.random)}"/>`));
  }

  /** A surface laid in one sweep of a wide marker, from its left edge. */
  wash(x: number, y: number, width: number, height: number, tone: Tone, at: number, dur = 420, radius = 22): void {
    this.group([x, y + height / 2], [{ t: at, sx: 0, o: 0 }, { t: at + 60, o: 1 }, { t: at + dur, sx: 1 }], () =>
      this.out.push(`<rect class="fw-${tone}" x="${f(x)}" y="${f(y)}" width="${f(width)}" height="${f(height)}" rx="${radius}"/>`),
    );
  }

  /** One of the interface's marks, `size` across, set down at `at`. */
  mark(name: GlyphName, at: Point, size: number, ink: Ink, when: number): void {
    const inner = /<svg[^>]*>([\s\S]*)<\/svg>/.exec(glyph(name, 20))?.[1] ?? "";
    const centre: Point = [at[0] + size / 2, at[1] + size / 2];
    this.group(centre, land(when), () =>
      this.out.push(
        `<g class="fl-mark fl-${ink}" transform="translate(${f(at[0])} ${f(at[1])}) scale(${f(size / 20)})" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">${inner}</g>`,
      ),
    );
  }

  /** Words set in the interface's own type, clear and never drawn;
   *  `mono` for a program's own output. */
  text(at: Point, words: string, size: number, ink: Ink, when: number, { anchor = "start", mono = false }: { anchor?: "start" | "middle" | "end"; mono?: boolean } = {}): void {
    this.group(at, [{ t: when, o: 0, y: 6 }, { t: when + 320, o: 1, y: 0 }], () =>
      this.out.push(
        `<text class="fl-type fl-${ink}${mono ? " fl-mono" : ""}" x="${f(at[0])}" y="${f(at[1])}" font-size="${size}" text-anchor="${anchor}">${escapeText(words)}</text>`,
      ),
    );
  }

  /** A group of marks that moves as `poses` say, turning and scaling
   *  about `origin`. */
  group(origin: Point, poses: Pose[], draw: () => void): void {
    const outer = this.out;
    this.out = [];
    draw();
    const inner = this.out.join("");
    this.out = outer;
    this.out.push(`<g data-k='${JSON.stringify(poses)}' data-o="${f(origin[0])},${f(origin[1])}">${inner}</g>`);
  }

  /** The reel, `duration` long. */
  build(duration: number): Built {
    return {
      markup: `<defs>${this.defs.join("")}</defs>${this.out.join("")}`,
      width: this.width,
      height: this.height,
      duration,
      captions: [...this.captions].sort((a, b) => a.at - b.at),
    };
  }
}

/** Short dashes along a line, one every `every` units, each half that
 *  long. */
function ticksAlong(points: Point[], every: number): Point[][] {
  const dashes: Point[][] = [];
  let carried = 0;
  for (let index = 1; index < points.length; index += 1) {
    const [x0, y0] = points[index - 1]!;
    const [x1, y1] = points[index]!;
    const segment = Math.hypot(x1 - x0, y1 - y0);
    let at = carried;
    while (at < segment) {
      const t0 = at / segment;
      const t1 = Math.min(1, (at + every / 2) / segment);
      dashes.push([
        [x0 + (x1 - x0) * t0, y0 + (y1 - y0) * t0],
        [x0 + (x1 - x0) * t1, y0 + (y1 - y0) * t1],
      ]);
      at += every;
    }
    carried = at - segment;
  }
  return dashes;
}

/** A mark landing: from nothing, a little past its size, then settling. */
function land(at: number): Pose[] {
  return [
    { t: at, sx: 0.4, sy: 0.4, o: 0 },
    { t: at + 180, sx: 1.12, sy: 1.12, o: 1 },
    { t: at + 320, sx: 1, sy: 1 },
  ];
}

function escapeText(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
