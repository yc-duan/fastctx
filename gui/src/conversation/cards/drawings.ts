// What a card shows of the thing it was, drawn in the film's hand
// (`world-gui.md`, 审美; `film/`): a solid wash as a marker lays it, its
// writing a line of handwriting, its structure fine graphite that thins
// where the pen lands and lifts; the accent only in a commit's seal. A file
// is a sheet, a change marked across its lines in the colours of what came
// and went; tests are a card of small squares; a check is a shield with a
// tick or drops; a commit is a seal, its push a dashed way up to the
// branch's flag. Seeded, so each is the same every time.

import "./drawings.css";
import { glyph, type GlyphName } from "../glyphs";
import { blot, circlePoints, dot, outline, quadratic, ruledPoints, seeded, swatch, type Point } from "../../pen/shapes";
import type { ChecksDeed, CommitDeed, PushDeed, TestsDeed } from "../deeds/types";

/** The washes and pens: a thing, a part a step lighter behind or within
 *  it, graphite for structure and writing, clay for the one mark that
 *  matters, a change's colours, how something went, and the stage's own
 *  colour for a mark cut into a solid thing. */
export type Ink = "thing" | "part" | "graphite" | "clay" | "added" | "removed" | "wrong" | "caution" | "cut" | "add" | "del";

export interface Figure {
  svg: string;
  width: number;
  height: number;
}


const f = (n: number) => (Math.round(n * 10) / 10).toString();

export class Pen {
  private readonly random: () => number;
  private readonly parts: string[] = [];

  constructor(seed: string) {
    this.random = seeded(seed);
  }

  wash(x: number, y: number, width: number, height: number, ink: Ink, corner = 4): this {
    return this.put(swatch(x, y, width, height, this.random, corner), ink);
  }

  rule(from: Point, to: Point, width: number, ink: Ink): this {
    return this.put(outline(ruledPoints(from, to, this.random), width, this.random, { pressure: 0.18, taper: 0.06 }), ink);
  }

  stroke(points: Point[], width: number, ink: Ink, taper = 0.14): this {
    return this.put(outline(points, width, this.random, { pressure: 0.25, taper }), ink);
  }

  ring(center: Point, radius: number, width: number, ink: Ink): this {
    return this.put(outline(circlePoints(center, radius, this.random), width, this.random, { pressure: 0.3, taper: 0.08 }), ink);
  }

  disc(center: Point, radius: number, ink: Ink): this {
    return this.put(dot(center, radius, this.random), ink);
  }

  blot(center: Point, radius: number, ink: Ink): this {
    return this.put(blot(center, radius, this.random), ink);
  }

  /** A line of handwriting standing in for words. */
  scribble(from: Point, length: number, ink: Ink = "graphite", width = 1.3, amplitude = 1.15): this {
    const phase = this.random() * 6;
    const points: Point[] = [];
    for (let x = 0; x <= length; x += 2.5) points.push([from[0] + x, from[1] + Math.sin(x / 4.2 + phase) * amplitude + Math.sin(x / 11 + phase) * amplitude * 0.45]);
    return this.put(outline(points, width, this.random, { pressure: 0.3, taper: 0.12 }), ink);
  }

  /** Short strokes along a gentle curve, a way not walked but marked. */
  dashes(from: Point, control: Point, to: Point, width: number, ink: Ink, every = 5.5): this {
    const points = quadratic(from, control, to, 1);
    let carried = 0;
    for (let index = 1; index < points.length; index += 1) {
      carried += Math.hypot(points[index]![0] - points[index - 1]![0], points[index]![1] - points[index - 1]![1]);
      if (carried < every) continue;
      carried = 0;
      const back = points[Math.max(0, index - 2)]!;
      this.stroke([back, points[index]!], width, ink, 0.3);
    }
    return this;
  }

  /** Marks drawn inside a group of their own, which moves as `kind`
   *  says (`drawings.css`): a sheet carried, a seal set down. */
  group(kind: string, draw: (pen: this) => void): this {
    const outer = this.parts.splice(0);
    draw(this);
    const inner = this.parts.splice(0);
    this.parts.push(...outer, `<g class="${kind}">${inner.join("")}</g>`);
    return this;
  }

  /** One of the interface's own marks, drawn as the interface draws it. */
  mark(name: GlyphName, at: Point, size: number, ink: Ink): this {
    const inner = /<svg[^>]*>([\s\S]*)<\/svg>/.exec(glyph(name, 20))?.[1] ?? "";
    this.parts.push(`<g class="wsk-mark wsk-${ink}" transform="translate(${f(at[0])} ${f(at[1])}) scale(${f(size / 20)})">${inner}</g>`);
    return this;
  }

  figure(width: number, height: number, scale = 1): Figure {
    const w = width * scale;
    const h = height * scale;
    return { svg: `<svg class="wsk" width="${f(w)}" height="${f(h)}" viewBox="0 0 ${f(width)} ${f(height)}" aria-hidden="true">${this.parts.join("")}</svg>`, width: w, height: h };
  }

  /** A shape of its own, as a path. */
  shape(d: string, ink: Ink): this {
    return this.put(d, ink);
  }

  private put(d: string, ink: Ink): this {
    if (d) this.parts.push(`<path class="wsk-${ink}" d="${d}"/>`);
    return this;
  }
}

/** The lines of a sheet, and how long each one's writing runs. */
const LINES = [12, 20.5, 29, 37.5, 46];
const LENGTHS = [25, 19, 23, 15, 21];

/** A file: a sheet with its lines of writing; changed, the lines it gained
 *  marked in the colour of what came, those it lost in the colour of what
 *  went, as many as the change was large. */
export function sheet(seed: string, change: { how: "added" | "modified" | "deleted" | "moved" | null; added: number; removed: number } | null): Figure {
  const pen = new Pen(`sheet ${seed}`);
  pen.wash(4, 2, 38, 52, "thing", 4);
  const bands: (Ink | null)[] = LINES.map(() => null);
  if (change?.how === "added") bands.fill("added");
  else if (change?.how === "deleted") bands.fill("removed");
  else if (change) {
    const gone = change.removed ? (change.removed >= 8 ? 2 : 1) : 0;
    const came = change.added ? (change.added >= 20 ? 3 : change.added >= 4 ? 2 : 1) : 0;
    let at = 1;
    for (let n = 0; n < gone && at < LINES.length; n += 1) bands[at++] = "removed";
    for (let n = 0; n < came && at < LINES.length; n += 1) bands[at++] = "added";
  }
  LINES.forEach((y, index) => {
    const band = bands[index];
    if (band) pen.wash(7.5, y - 4.4, 31, 8.8, band, 2);
    pen.scribble([10, y], LENGTHS[index]!);
  });
  return pen.figure(46, 56);
}

/** Many files read: a few sheets laid one on another. */
export function pile(seed: string): Figure {
  const pen = new Pen(`pile ${seed}`);
  pen.wash(15, 2, 32, 44, "part", 4);
  pen.wash(10, 6, 33, 46, "part", 4);
  pen.wash(4, 10, 34, 46, "thing", 4);
  [20, 28, 36, 44].forEach((y, index) => pen.scribble([10, y], [21, 15, 19, 12][index]!));
  return pen.figure(50, 58);
}

/** A search: a lens laid over a sheet. */
export function lens(seed: string): Figure {
  const pen = new Pen(`lens ${seed}`);
  pen.wash(4, 8, 32, 44, "thing", 4);
  [18, 26, 34, 42].forEach((y, index) => pen.scribble([9, y], [19, 14, 20, 11][index]!));
  pen.disc([31, 24], 10.5, "part");
  pen.ring([31, 24], 10.5, 2.2, "graphite");
  pen.stroke([[38.5, 31.5], [47, 40]], 3.6, "graphite", 0.1);
  return pen.figure(52, 54);
}

/** Tests: a card of small squares, one a test, or as many as it takes to
 *  keep to forty; passed in graphite, skipped faint, failed in the colour
 *  of what went wrong. The runs before the last lie behind it, a drop on
 *  each that failed. */
export function tests(seed: string, runs: readonly TestsDeed[], built: { warnings: number; errors: number } = { warnings: 0, errors: 0 }): Figure {
  const pen = new Pen(`tests ${seed}`);
  const last = runs[runs.length - 1]!;
  const total = last.passed + last.failed + last.ignored;
  const each = Math.max(1, Math.ceil(total / 40));
  const failed = Math.ceil(last.failed / each);
  const ignored = Math.round(last.ignored / each);
  const cells = Math.max(failed + ignored, Math.ceil(total / each), 1);
  const rows = Math.max(1, Math.ceil(cells / 10));
  const columns = Math.ceil(cells / rows);
  const cell = 5.4;
  const gap = 2.3;
  const pad = 7;
  const width = Math.max(32, pad * 2 + columns * (cell + gap) - gap);
  const height = Math.max(26, pad * 2 + rows * (cell + gap) - gap);
  const behind = runs.slice(-3, -1);
  const shift = 4.5;
  const top = 2 + behind.length * shift;
  behind.forEach((run, index) => {
    const offset = (behind.length - index) * shift;
    pen.wash(2 + offset, top - offset, width, height, "part", 6);
    if (run.failed) pen.blot([2 + offset + width - 5, top - offset + 4.5], 2.4, "wrong");
  });
  pen.wash(2, top, width, height, "thing", 6);
  const left = 2 + (width - (columns * (cell + gap) - gap)) / 2;
  const upper = top + (height - (rows * (cell + gap) - gap)) / 2;
  for (let index = 0; index < cells; index += 1) {
    const ink: Ink = index >= cells - failed ? "wrong" : index >= cells - failed - ignored ? "part" : "graphite";
    pen.wash(left + (index % columns) * (cell + gap), upper + Math.floor(index / columns) * (cell + gap), cell, cell, ink, 1.3);
  }
  if (built.errors || built.warnings) pen.blot([width - 2.5, top + height - 2.5], 2.6, built.errors ? "wrong" : "caution");
  return pen.figure(width + 4 + behind.length * shift, top + height + 2);
}

/** A check: a shield, a tick on it where it found nothing; else a drop for
 *  each error, in the colour of what went wrong, and for each warning. */
export function shield(seed: string, run: ChecksDeed): Figure {
  const pen = new Pen(`shield ${seed}`);
  const random = seeded(`shield ${seed} shape`);
  const j = () => (random() - 0.5) * 0.8;
  const d = `M${f(21 + j())} ${f(3 + j())}C${f(27)} ${f(6)} ${f(31)} ${f(7)} ${f(36 + j())} ${f(8 + j())}L${f(35.5 + j())} ${f(23)}C${f(35)} ${f(32)} ${f(29)} ${f(39)} ${f(21 + j())} ${f(44 + j())}C${f(13)} ${f(39)} ${f(7)} ${f(32)} ${f(6.5 + j())} ${f(23)}L${f(6 + j())} ${f(8 + j())}C${f(11)} ${f(7)} ${f(15)} ${f(6)} ${f(21 + j())} ${f(3 + j())}Z`;
  pen.shape(d, "thing");
  const errors = run.items.filter((item) => item.level === "error").length;
  const warnings = run.items.length - errors;
  if (run.clean || !run.items.length) pen.stroke([[14, 23.5], [19, 29.5], [29, 16.5]], 2.6, "graphite", 0.2);
  else {
    const drops: Ink[] = [...Array<Ink>(Math.min(errors, 3)).fill("wrong"), ...Array<Ink>(Math.min(warnings, 3 - Math.min(errors, 3))).fill("caution")];
    const span = (drops.length - 1) * 7.5;
    drops.forEach((ink, index) => pen.blot([21 - span / 2 + index * 7.5, 23], 2.9, ink));
  }
  return pen.figure(42, 47);
}

/** A commit: a seal in clay, its script cut into it; more than one, the
 *  ones before it behind it. Sent, a dashed way from it up to the flag of
 *  the branch it went to, or to a drop where it was turned away. */
export function seal(seed: string, commits: readonly CommitDeed[], pushes: readonly PushDeed[]): Figure {
  const pen = new Pen(`seal ${seed}`);
  const sent = pushes.length > 0;
  const refused = pushes.some((push) => push.refs.some((ref) => !ref.ok));
  const centre: Point = [17, 39];
  if (commits.length) {
    if (commits.length > 1) pen.disc([centre[0] + 5, centre[1] - 5], 12, "part");
    pen.group("wsk-stamp", (seal) => {
      seal.disc(centre, 12.5, "clay");
      seal.stroke([[11.5, 35], [22.5, 34.5]], 1.7, "cut", 0.3);
      seal.stroke([[17.2, 30.5], [16.8, 47]], 1.7, "cut", 0.3);
      seal.stroke([[12, 42], [16, 39.5]], 1.6, "cut", 0.3);
      seal.stroke([[18.5, 40], [22.5, 43.5]], 1.6, "cut", 0.3);
    });
  } else pen.blot(centre, 3.6, "graphite");
  if (!sent) return pen.figure(36, 54);
  const from: Point = commits.length ? [27, 29] : [21, 35];
  if (refused) {
    pen.dashes(from, [36, 14], [46, 15], 1.5, "graphite");
    pen.blot([50, 16], 3.2, "wrong");
    return pen.figure(58, 54);
  }
  pen.dashes(from, [37, 8], [52, 10], 1.5, "graphite");
  pen.rule([57, 3], [57.4, 27], 1.8, "graphite");
  pen.wash(57.6, 3, 12, 8.5, "graphite", 1.5);
  return pen.figure(72, 54);
}

/** What the working tree holds: a card of short rows, a tick on it where
 *  nothing is changed. */
export function tree(seed: string, files: number): Figure {
  const pen = new Pen(`tree ${seed}`);
  pen.wash(3, 4, 40, 44, "thing", 5);
  if (!files) pen.stroke([[14, 26], [19, 31.5], [30, 19]], 2.4, "graphite", 0.2);
  else
    [14, 22, 30, 38].slice(0, Math.min(4, files)).forEach((y, index) => {
      pen.blot([11, y], 1.9, "graphite");
      pen.rule([16, y], [16 + [20, 14, 18, 11][index]!, y], 1.4, "graphite");
    });
  return pen.figure(46, 50);
}

/** The history: a line with a dot for each commit read. */
export function history(seed: string, commits: number): Figure {
  const pen = new Pen(`history ${seed}`);
  pen.rule([12, 5], [12, 48], 1.6, "graphite");
  [10, 21, 32, 43].slice(0, Math.max(1, Math.min(4, commits))).forEach((y, index) => {
    pen.disc([12, y], 3, "graphite");
    pen.scribble([20, y], [18, 13, 16, 10][index]!);
  });
  return pen.figure(44, 52);
}

/** A card with one of the interface's marks on it and its writing under:
 *  a command, a page of the web, a call of another kind. A drop in its
 *  corner where it went wrong. */
export function card(seed: string, mark: GlyphName, failed: boolean): Figure {
  const pen = new Pen(`card ${seed}`);
  pen.wash(2, 6, 58, 42, "thing", 6);
  pen.mark(mark, [8, 11], 17, "graphite");
  pen.scribble([11, 35], 36);
  pen.scribble([11, 42], 24);
  if (failed) pen.blot([52, 14], 3.1, "wrong");
  return pen.figure(62, 50);
}

/** A picture: a frame with a hill and a sun in it. */
export function picture(seed: string): Figure {
  const pen = new Pen(`picture ${seed}`);
  pen.wash(2, 6, 52, 42, "thing", 6);
  pen.disc([17, 19], 4, "graphite");
  pen.stroke([[8, 41], [20, 29], [30, 37], [38, 31], [48, 40]], 1.9, "graphite", 0.1);
  return pen.figure(56, 50);
}

/** Several files changed one after another: their sheets fanned, each
 *  with its marks. */
export function fan(seed: string, count: number): Figure {
  const pen = new Pen(`fan ${seed}`);
  const shown = Math.min(3, count);
  for (let index = shown - 1; index >= 0; index -= 1) {
    const x = 4 + index * 9;
    const y = 2 + (shown - 1 - index) * 2;
    pen.wash(x, y, 34, 48, index ? "part" : "thing", 4);
    if (!index)
      [12, 20, 28, 36, 44].forEach((line, at) => {
        if (at === 1) pen.wash(x + 3, line + y - 6.4, 27, 8, "removed", 2);
        if (at === 2 || at === 3) pen.wash(x + 3, line + y - 6.4, 27, 8, "added", 2);
        pen.scribble([x + 6, line + y - 2], [21, 15, 19, 12, 17][at]!);
      });
  }
  return pen.figure(4 + (shown - 1) * 9 + 38, 56);
}

/** Several commands run one after another: their windows laid one on
 *  another. */
export function terminals(seed: string, count: number): Figure {
  const pen = new Pen(`terminals ${seed}`);
  const shown = Math.min(3, count);
  for (let index = shown - 1; index >= 0; index -= 1) {
    const x = 2 + index * 6;
    const y = 4 + (shown - 1 - index) * 5;
    pen.wash(x, y, 54, 38, index ? "part" : "thing", 6);
    if (!index) {
      pen.mark("command", [x + 6, y + 5], 16, "graphite");
      pen.scribble([x + 9, y + 27], 32);
      pen.scribble([x + 9, y + 33.5], 20);
    }
  }
  return pen.figure(2 + (shown - 1) * 6 + 56, 4 + (shown - 1) * 5 + 40);
}

/** A copy: two machines' screens, the dashed way between them, the sheet
 *  come to rest at the far one, carried along the way once as it is first
 *  shown; a drop where some did not arrive. */
export function carried(seed: string, failed: boolean): Figure {
  const pen = new Pen(`carried ${seed}`);
  pen.wash(2, 26, 24, 17, "thing", 3);
  pen.rule([14, 44], [14, 48], 1.6, "graphite");
  pen.rule([9, 49.5], [19, 49.5], 1.7, "graphite");
  pen.wash(54, 26, 24, 17, "thing", 3);
  pen.rule([66, 44], [66, 48], 1.6, "graphite");
  pen.rule([61, 49.5], [71, 49.5], 1.7, "graphite");
  pen.dashes([20, 22], [40, 0], [60, 22], 1.4, "graphite");
  pen.group("wsk-travel", (sheet) => {
    sheet.wash(60, 6, 12, 15, "thing", 2);
    sheet.scribble([62.5, 11.5], 7, "graphite", 1, 0.8);
    sheet.scribble([62.5, 16], 5, "graphite", 1, 0.8);
  });
  if (failed) pen.blot([48, 14], 3, "wrong");
  return pen.figure(80, 52, 1.5);
}

/** A job's terminal: a tick on it where it ended well, a drop where it did
 *  not, a square where it was stopped; still going, its cursor. */
export function terminal(seed: string, state: "done" | "failed" | "stopped" | "going"): Figure {
  const pen = new Pen(`terminal ${seed}`);
  pen.wash(2, 4, 58, 42, "thing", 6);
  pen.mark("command", [8, 9], 17, "graphite");
  pen.scribble([11, 32], 30);
  pen.scribble([11, 39], 20);
  if (state === "done") pen.stroke([[44, 13], [48, 17.5], [55, 9]], 2.2, "graphite", 0.2);
  else if (state === "failed") pen.blot([51, 13], 3.2, "wrong");
  else if (state === "stopped") pen.wash(46.5, 9, 8, 8, "graphite", 1.6);
  else pen.wash(37, 35, 6, 5, "graphite", 1);
  return pen.figure(62, 48, 1.2);
}

/** A page published: a window with its writing and a button on it. */
export function published(seed: string): Figure {
  const pen = new Pen(`page ${seed}`);
  pen.wash(2, 4, 58, 44, "thing", 6);
  pen.wash(2, 4, 58, 8, "part", 6);
  pen.scribble([9, 20], 34);
  pen.scribble([9, 27], 26);
  pen.wash(9, 33, 18, 8, "clay", 3);
  pen.scribble([31, 37], 18);
  return pen.figure(62, 50, 1.2);
}

/** A project entered: a few houses of a town under a ruled ground. */
export function town(seed: string): Figure {
  const pen = new Pen(`town ${seed}`);
  pen.wash(6, 22, 16, 20, "thing", 2);
  pen.stroke([[4.5, 23], [14, 13], [23.5, 23]], 1.8, "graphite", 0.1);
  pen.wash(28, 16, 18, 26, "thing", 2);
  pen.stroke([[26.5, 17], [37, 6], [47.5, 17]], 1.8, "graphite", 0.1);
  pen.wash(34, 32, 6, 10, "part", 1);
  pen.wash(51, 26, 12, 16, "thing", 2);
  pen.rule([2, 43.5], [66, 43.5], 1.6, "graphite");
  return pen.figure(68, 48, 1.2);
}

/** The plan written: a sheet with its items ticked and to be done. */
export function planned(seed: string): Figure {
  const pen = new Pen(`plan ${seed}`);
  pen.wash(4, 2, 40, 52, "thing", 4);
  [13, 23, 33, 43].forEach((y, index) => {
    if (index < 2) pen.stroke([[9, y - 1], [11.5, y + 1.6], [15.5, y - 3]], 1.6, "graphite", 0.2);
    else pen.ring([12, y - 0.5], 2.4, 1.2, "graphite");
    pen.scribble([19, y], [19, 15, 20, 13][index]!);
  });
  return pen.figure(48, 56, 1.1);
}

/** A goal: its flag; complete, a tick beside it; blocked, a drop; paused,
 *  the flag furled. */
export function pennant(seed: string, state: "complete" | "blocked" | "pause"): Figure {
  const pen = new Pen(`goal ${seed}`);
  pen.rule([10, 4], [10.4, 50], 2, "graphite");
  if (state === "pause") pen.wash(10.5, 5, 6, 20, "graphite", 1.6);
  else pen.wash(10.5, 5, 26, 15, state === "complete" ? "clay" : "graphite", 2);
  if (state === "complete") pen.stroke([[34, 38], [38.5, 43], [47, 32]], 2.4, "graphite", 0.2);
  if (state === "blocked") pen.blot([42, 38], 3.4, "wrong");
  pen.rule([3, 51], [20, 51], 1.6, "graphite");
  return pen.figure(52, 54, 1.1);
}

/** A sentinel: an eye open, watching. */
export function watching(seed: string): Figure {
  const pen = new Pen(`eye ${seed}`);
  pen.stroke([[4, 26], [16, 14], [34, 10], [52, 14], [64, 26]], 2, "graphite", 0.1);
  pen.stroke([[4, 26], [16, 36], [34, 40], [52, 36], [64, 26]], 2, "graphite", 0.1);
  pen.disc([34, 25], 8.5, "thing");
  pen.disc([34, 25], 4, "graphite");
  return pen.figure(68, 50, 1.2);
}

/** Tests as a card draws them: a small square each, as many to a row as
 *  `columns`; past three hundred, each square as many as it takes. Passed
 *  in graphite, skipped faint, failed in the colour of what went wrong,
 *  last, where the eye ends. */
export function grid(seed: string, deed: TestsDeed, columns = 30): string {
  const pen = new Pen(`grid ${seed}`);
  const total = deed.passed + deed.failed + deed.ignored;
  const each = Math.max(1, Math.ceil(total / 300));
  const failed = Math.ceil(deed.failed / each);
  const ignored = Math.round(deed.ignored / each);
  const cells = Math.max(failed + ignored, Math.ceil(total / each), 1);
  const across = Math.min(columns, cells);
  const rows = Math.ceil(cells / across);
  const cell = 7;
  const gap = 2.6;
  for (let index = 0; index < cells; index += 1) {
    const ink: Ink = index >= cells - failed ? "wrong" : index >= cells - failed - ignored ? "part" : "graphite";
    pen.wash(1 + (index % across) * (cell + gap), 1 + Math.floor(index / across) * (cell + gap), cell, cell, ink, 1.6);
  }
  return pen.figure(across * (cell + gap) - gap + 2, rows * (cell + gap) - gap + 2).svg;
}
