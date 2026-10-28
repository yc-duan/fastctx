// Still drawings in the film's hand (`world-gui.md`, 审美): things are
// solid washes as a marker lays them, structure is fine ink that thins where
// the pen lands and lifts, words stand in as a line of handwriting, and the
// accent is one or two marks. Never an even outline, never a jittering
// filter. Seeded, so a drawing is the same each time it is drawn.

import "./sketch.css";
import { blot, circlePoints, outline, ruledPoints, seeded, swatch, type Point } from "./shapes";

/** The pens: graphite for structure, clay for the one mark that matters,
 *  paper for a mark laid over a wash, and the colour of the text around it
 *  for a mark whose colour says how something went. */
export type SketchInk = "graphite" | "clay" | "paper" | "current";
/** The washes: a thing as the interface's boxes are, a field a step lighter
 *  than it, the accent's solid, the accent's light wash for the one thing
 *  that matters among others, and graphite for a small dark part. */
export type SketchWash = "box" | "field" | "clay" | "accent" | "graphite";

export class Sketch {
  private readonly random: () => number;
  private readonly parts: string[] = [];

  constructor(seed: string) {
    this.random = seeded(seed);
  }

  /** A thing: a solid wash with softened corners. */
  wash(x: number, y: number, width: number, height: number, tone: SketchWash, corner = 6): this {
    return this.put(swatch(x, y, width, height, this.random, corner), `sk-${tone}`);
  }

  /** A line drawn against a straight edge. */
  rule(from: Point, to: Point, width: number, ink: SketchInk): this {
    return this.put(outline(ruledPoints(from, to, this.random), width, this.random, { pressure: 0.18, taper: 0.04 }), `sk-${ink}`);
  }

  /** A free stroke along `points`, thinning at its ends. */
  stroke(points: Point[], width: number, ink: SketchInk, taper = 0.12): this {
    return this.put(outline(points, width, this.random, { pressure: 0.25, taper }), `sk-${ink}`);
  }

  /** A drop of ink. */
  blot(center: Point, radius: number, ink: SketchInk): this {
    return this.put(blot(center, radius, this.random), `sk-${ink}`);
  }

  /** A circle as a hand draws one. */
  ring(center: Point, radius: number, width: number, ink: SketchInk): this {
    return this.put(outline(circlePoints(center, radius, this.random), width, this.random, { pressure: 0.3, taper: 0.08 }), `sk-${ink}`);
  }

  /** Part of a circle, from angle `from` to `to` in radians, as a hand
   *  sweeps it. */
  arc(center: Point, radius: number, from: number, to: number, width: number, ink: SketchInk): this {
    const steps = Math.max(8, Math.round((Math.abs(to - from) * radius) / 3));
    const points: Point[] = [];
    for (let index = 0; index <= steps; index += 1) {
      const angle = from + ((to - from) * index) / steps;
      const r = radius * (1 + (this.random() - 0.5) * 0.03);
      points.push([center[0] + Math.cos(angle) * r, center[1] + Math.sin(angle) * r]);
    }
    return this.stroke(points, width, ink, 0.12);
  }

  /** A line of handwriting standing in for words. */
  scribble(from: Point, length: number, ink: SketchInk, width = 1.2, amplitude = 1.6): this {
    const phase = this.random() * 6;
    const points: Point[] = [];
    for (let x = 0; x <= length; x += 3) points.push([from[0] + x, from[1] + Math.sin(x / 5 + phase) * amplitude + Math.sin(x / 13 + phase) * amplitude * 0.5]);
    return this.put(outline(points, width, this.random, { pressure: 0.3, taper: 0.1 }), `sk-${ink}`);
  }

  /** Short ruled strokes along a line, for a way not yet taken. */
  dashed(from: Point, to: Point, width: number, ink: SketchInk, dash = 5, gap = 5): this {
    const length = Math.hypot(to[0] - from[0], to[1] - from[1]);
    const ux = (to[0] - from[0]) / (length || 1);
    const uy = (to[1] - from[1]) / (length || 1);
    for (let at = 0; at < length; at += dash + gap) {
      const end = Math.min(length, at + dash);
      this.rule([from[0] + ux * at, from[1] + uy * at], [from[0] + ux * end, from[1] + uy * end], width, ink);
    }
    return this;
  }

  /** One of the interface's own marks, drawn as the interface draws it. */
  mark(inner: string, at: Point, size: number, ink: SketchInk): this {
    this.parts.push(`<g class="sk-mark sk-${ink}" transform="translate(${at[0]} ${at[1]}) scale(${size / 20})">${inner}</g>`);
    return this;
  }

  /** The drawing, `width` by `height`. */
  svg(width: number, height: number, kind: string): string {
    return `<svg class="${kind}" viewBox="0 0 ${width} ${height}" aria-hidden="true">${this.parts.join("")}</svg>`;
  }

  private put(d: string, kind: string): this {
    this.parts.push(`<path class="${kind}" d="${d}"/>`);
    return this;
  }
}
