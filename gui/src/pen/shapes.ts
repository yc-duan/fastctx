// Shapes as a hand makes them: every mark is a filled outline around a
// centre line that is never quite straight, of a width that is never quite
// even, thinning where the pen lands and lifts. Seeded, so the same thing
// is always drawn the same way.

export type Point = readonly [number, number];

/** A small deterministic random source. */
export function seeded(seed: string): () => number {
  let state = 2166136261;
  for (const char of seed) state = Math.imul(state ^ char.charCodeAt(0), 16777619);
  return () => {
    state = Math.imul(state ^ (state >>> 15), 2246822507);
    state = Math.imul(state ^ (state >>> 13), 3266489909);
    state ^= state >>> 16;
    return (state >>> 0) / 4294967296;
  };
}

const f = (n: number) => (Math.round(n * 10) / 10).toString();

/** A slow random walk, for pressure and drift that wander rather than
 *  jump: `count` values around zero, each within `amount`. */
function wander(count: number, amount: number, random: () => number): number[] {
  const values: number[] = [];
  let value = 0;
  let velocity = 0;
  for (let index = 0; index < count; index += 1) {
    velocity = velocity * 0.7 + (random() - 0.5) * amount * 0.6;
    value = Math.max(-amount, Math.min(amount, value * 0.92 + velocity));
    values.push(value);
  }
  return values;
}

/** The filled outline of a stroke along `points`: `width` across, varied
 *  by `pressure` (a fraction of the width), tapering over `taper` of its
 *  length at each end. */
export function outline(
  points: Point[],
  width: number,
  random: () => number,
  { pressure = 0.25, taper = 0.12, taperStart = taper, taperEnd = taper }: { pressure?: number; taper?: number; taperStart?: number; taperEnd?: number } = {},
): string {
  if (points.length < 2) return "";
  const lengths = [0];
  for (let index = 1; index < points.length; index += 1) {
    lengths.push(lengths[index - 1] + Math.hypot(points[index][0] - points[index - 1][0], points[index][1] - points[index - 1][1]));
  }
  const total = lengths[lengths.length - 1] || 1;
  const press = wander(points.length, pressure, random);
  const left: string[] = [];
  const right: string[] = [];
  points.forEach((point, index) => {
    const before = points[Math.max(0, index - 1)];
    const after = points[Math.min(points.length - 1, index + 1)];
    const dx = after[0] - before[0];
    const dy = after[1] - before[1];
    const length = Math.hypot(dx, dy) || 1;
    const nx = -dy / length;
    const ny = dx / length;
    const t = lengths[index] / total;
    const ends = Math.min(1, taperStart > 0 ? t / taperStart : 1, taperEnd > 0 ? (1 - t) / taperEnd : 1);
    const eased = ends * ends * (3 - 2 * ends);
    const half = (width / 2) * (1 + press[index]) * (0.25 + 0.75 * eased);
    left.push(`${f(point[0] + nx * half)} ${f(point[1] + ny * half)}`);
    right.push(`${f(point[0] - nx * half)} ${f(point[1] - ny * half)}`);
  });
  return `M${left.join("L")}L${right.reverse().join("L")}Z`;
}

/** The centre line of a line drawn against a straight edge: near
 *  straight, drifting by a fraction of a unit along its way. */
export function ruledPoints(from: Point, to: Point, random: () => number): Point[] {
  const length = Math.hypot(to[0] - from[0], to[1] - from[1]);
  const steps = Math.max(2, Math.round(length / 6));
  const drift = wander(steps + 1, 0.35, random);
  const nx = -(to[1] - from[1]) / (length || 1);
  const ny = (to[0] - from[0]) / (length || 1);
  const points: Point[] = [];
  for (let index = 0; index <= steps; index += 1) {
    const t = index / steps;
    points.push([from[0] + (to[0] - from[0]) * t + nx * drift[index], from[1] + (to[1] - from[1]) * t + ny * drift[index]]);
  }
  return points;
}

/** A line drawn against a straight edge. */
export function ruled(from: Point, to: Point, width: number, random: () => number): string {
  return outline(ruledPoints(from, to, random), width, random, { pressure: 0.18, taper: 0.02 });
}

/** The centre line of a circle as a hand draws one: begun anywhere, a
 *  little off round, and closed with a small overlap where the pen comes
 *  back past its start. */
export function circlePoints(center: Point, radius: number, random: () => number): Point[] {
  const start = random() * Math.PI * 2;
  const sweep = Math.PI * 2 + 0.25 + random() * 0.25;
  const steps = Math.max(24, Math.round((radius * sweep) / 3));
  const wobble = wander(steps + 1, 0.05, random);
  const points: Point[] = [];
  for (let index = 0; index <= steps; index += 1) {
    const angle = start + (index / steps) * sweep;
    const r = radius * (1 + wobble[index]);
    points.push([center[0] + Math.cos(angle) * r, center[1] + Math.sin(angle) * r]);
  }
  return points;
}

/** A circle as a hand draws one. */
export function circle(center: Point, radius: number, width: number, random: () => number): string {
  return outline(circlePoints(center, radius, random), width, random, { pressure: 0.3, taper: 0.08 });
}

/** A blot: a small filled mark, round but not a circle. */
export function blot(center: Point, radius: number, random: () => number): string {
  const count = 9;
  const start = random() * Math.PI * 2;
  const points = Array.from({ length: count }, (_, index) => {
    const angle = start + (index / count) * Math.PI * 2;
    const r = radius * (0.8 + random() * 0.4);
    return [center[0] + Math.cos(angle) * r, center[1] + Math.sin(angle) * r] as const;
  });
  let d = `M${f(points[0][0])} ${f(points[0][1])}`;
  for (let index = 0; index < count; index += 1) {
    const p0 = points[(index - 1 + count) % count];
    const p1 = points[index];
    const p2 = points[(index + 1) % count];
    const p3 = points[(index + 2) % count];
    d += ` C${f(p1[0] + (p2[0] - p0[0]) / 6)} ${f(p1[1] + (p2[1] - p0[1]) / 6)} ${f(p2[0] - (p3[0] - p1[0]) / 6)} ${f(p2[1] - (p3[1] - p1[1]) / 6)} ${f(p2[0])} ${f(p2[1])}`;
  }
  return `${d}Z`;
}

/** Points along a quadratic curve, about every `step` units. */
export function quadratic(from: Point, control: Point, to: Point, step = 4): Point[] {
  const rough = Math.hypot(control[0] - from[0], control[1] - from[1]) + Math.hypot(to[0] - control[0], to[1] - control[1]);
  const count = Math.max(2, Math.round(rough / step));
  return Array.from({ length: count + 1 }, (_, index) => {
    const t = index / count;
    const u = 1 - t;
    return [u * u * from[0] + 2 * u * t * control[0] + t * t * to[0], u * u * from[1] + 2 * u * t * control[1] + t * t * to[1]] as const;
  });
}

/** Short strokes laid along a line, as a hand hatches a path: each a few
 *  units long, a little off the line and its angle. */
export function ticks(points: Point[], every: number, length: number, width: number, random: () => number): string[] {
  const marks: string[] = [];
  let carried = random() * every;
  for (let index = 1; index < points.length; index += 1) {
    const [x0, y0] = points[index - 1];
    const [x1, y1] = points[index];
    const segment = Math.hypot(x1 - x0, y1 - y0);
    let at = carried;
    while (at < segment) {
      const t = at / segment;
      const angle = Math.atan2(y1 - y0, x1 - x0) + (random() - 0.5) * 0.25;
      const cx = x0 + (x1 - x0) * t + (random() - 0.5) * width;
      const cy = y0 + (y1 - y0) * t + (random() - 0.5) * width;
      const half = (length * (0.75 + random() * 0.5)) / 2;
      const ux = Math.cos(angle) * half;
      const uy = Math.sin(angle) * half;
      marks.push(
        outline(
          [
            [cx - ux, cy - uy],
            [cx, cy],
            [cx + ux, cy + uy],
          ],
          width * (0.8 + random() * 0.5),
          random,
          { pressure: 0.2, taper: 0.35 },
        ),
      );
      at += every * (0.8 + random() * 0.4);
    }
    carried = at - segment;
  }
  return marks;
}
