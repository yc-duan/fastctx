import type { InkPath, Point, Segment } from './types';

const distance = (a: Point, b: Point) => Math.hypot(b[0] - a[0], b[1] - a[1]);
const midpoint = (a: Point, b: Point): Point => [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];

export const paths = {
  line: (from: Point, to: Point): InkPath => ({ start: from, segments: [{ kind: 'line', to }] }),
  polyline(points: readonly Point[], closed = false): InkPath {
    if (points.length < 2) throw new RangeError('A polyline needs at least two points');
    return { start: points[0], segments: points.slice(1).map(to => ({ kind: 'line', to })), closed };
  },
  quadratic: (from: Point, control: Point, to: Point): InkPath => ({
    start: from, segments: [{ kind: 'quadratic', control, to }],
  }),
  cubic: (from: Point, control1: Point, control2: Point, to: Point): InkPath => ({
    start: from, segments: [{ kind: 'cubic', control1, control2, to }],
  }),
  /** Angles are radians; positive sweep is clockwise in screen coordinates. */
  arc(center: Point, radius: number, start: number, sweep: number): InkPath {
    if (![...center, radius, start, sweep].every(Number.isFinite) || radius <= 0 || sweep === 0)
      throw new RangeError('An arc needs finite coordinates, positive radius and nonzero sweep');
    // Cubic arcs span at most a quarter turn; later subdivision controls mesh quality.
    const count = Math.ceil(Math.abs(sweep) / (Math.PI / 2));
    const delta = sweep / count;
    const point = (a: number): Point => [center[0] + radius * Math.cos(a), center[1] + radius * Math.sin(a)];
    const segments: Segment[] = [];
    for (let i = 0; i < count; i++) {
      const a = start + i * delta, b = a + delta, k = 4 / 3 * Math.tan(delta / 4) * radius;
      const p = point(a), q = point(b);
      segments.push({ kind: 'cubic',
        control1: [p[0] - k * Math.sin(a), p[1] + k * Math.cos(a)],
        control2: [q[0] + k * Math.sin(b), q[1] - k * Math.cos(b)], to: q });
    }
    return { start: point(start), segments };
  },
  circle(center: Point, radius: number): InkPath {
    const path = paths.arc(center, radius, 0, Math.PI * 2);
    return { ...path, segments: path.segments.map((s, i) => i === path.segments.length - 1 ? { ...s, to: path.start } : s), closed: true };
  },
};

export interface Mesh { vertices: Float32Array; length: number; points: number; }

export function tessellate(path: InkPath, step: number, tolerance: number): Mesh {
  const points: Point[] = [path.start];
  const validatePoint = (p: Point) => {
    if (!p || p.length !== 2 || !p.every(Number.isFinite)) throw new RangeError('Path coordinates must be finite pairs');
  };
  validatePoint(path.start);
  const append = (p: Point) => {
    validatePoint(p);
    const a = points[points.length - 1];
    const n = Math.ceil(distance(a, p) / step);
    for (let i = 1; i <= n; i++) points.push(i === n ? p : [a[0] + (p[0] - a[0]) * i / n, a[1] + (p[1] - a[1]) * i / n]);
  };
  const curve = (control: Point[]) => {
    control.forEach(validatePoint);
    const pending = [control];
    while (pending.length) {
      const p = pending.pop()!;
      const a = p[0], b = p[p.length - 1];
      const chord = distance(a, b);
      const polygon = p.slice(1).reduce((sum, q, i) => sum + distance(p[i], q), 0);
      const flatness = Math.max(...p.slice(1, -1).map(q => {
        if (!chord) return distance(a, q);
        const t = Math.max(0, Math.min(1, ((q[0] - a[0]) * (b[0] - a[0]) + (q[1] - a[1]) * (b[1] - a[1])) / chord ** 2));
        return distance(q, [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
      }));
      if (flatness <= tolerance && polygon <= step) { append(b); continue; }
      const left = [a], right = [b];
      let level = p;
      while (level.length > 1) {
        level = level.slice(1).map((q, i) => midpoint(level[i], q));
        left.push(level[0]); right.unshift(level[level.length - 1]);
      }
      if (left.every((q, i) => q[0] === p[i][0] && q[1] === p[i][1]))
        throw new RangeError('Curve subdivision exceeded coordinate precision');
      pending.push(right, left);
    }
  };
  for (const segment of path.segments) {
    const from = points[points.length - 1];
    if (segment.kind === 'line') append(segment.to);
    else if (segment.kind === 'quadratic') curve([from, segment.control, segment.to]);
    else if (segment.kind === 'cubic') curve([from, segment.control1, segment.control2, segment.to]);
    else throw new TypeError('Unknown path segment');
  }
  if (path.closed) append(path.start);
  if (points.length < 2) throw new RangeError('A stroke needs a nonzero path length');
  const lengths = [0];
  for (let i = 1; i < points.length; i++) lengths.push(lengths[i - 1] + distance(points[i - 1], points[i]));
  const vertices = new Float32Array(points.length * 12);
  const normal = (a: Point, b: Point): Point => {
    const d = distance(a, b); return [-(b[1] - a[1]) / d, (b[0] - a[0]) / d];
  };
  points.forEach((p, i) => {
    const prev = i === 0 ? (path.closed ? points[points.length - 2] : p) : points[i - 1];
    const next = i === points.length - 1 ? (path.closed ? points[1] : p) : points[i + 1];
    const incoming = distance(prev, p) ? normal(prev, p) : normal(p, next);
    const outgoing = distance(p, next) ? normal(p, next) : incoming;
    const nx = incoming[0] + outgoing[0], ny = incoming[1] + outgoing[1], magnitude = Math.hypot(nx, ny);
    // A unit bisector gives a bounded bevel-like join, including a path reversal.
    const n = magnitude ? [nx / magnitude, ny / magnitude] : outgoing;
    vertices.set([p[0], p[1], n[0], n[1], lengths[i], -1, p[0], p[1], n[0], n[1], lengths[i], 1], i * 12);
  });
  return { vertices, length: lengths[lengths.length - 1], points: points.length };
}
