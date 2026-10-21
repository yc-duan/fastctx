// One stroke of a broad felt-tip marker dragged along a line, the ink of
// the interface's bars (`world-gui.md`, 审美): its edges run nearly
// straight, rough in the small and drifting a little in the large, as a
// hand drags a chisel tip; it starts blunt and a touch fuller where the tip
// came down, slanted as the chisel meets the paper, and lifts at its finish,
// thinning a little and rounding off. The felt's streaks along the way and
// its fibrous edge come from the `pen-felt` filter (`filters.ts`). Strokes
// laid end to end overlap a little, the later one over the earlier's tail.

type Point = [number, number];

const f = (n: number) => (Math.round(n * 100) / 100).toString();

/** A gentle wave along a stroke, `amount` at most, of random phase: what
 *  a hand drifts by over a stroke's length. */
function drifting(random: () => number, amount: number): (t: number) => number {
  const slow = 0.6 + random() * 1.1;
  const quick = 2.4 + random() * 2.6;
  const a = random() * Math.PI * 2;
  const b = random() * Math.PI * 2;
  return (t) => amount * (0.7 * Math.sin(t * slow * Math.PI + a) + 0.3 * Math.sin(t * quick * Math.PI + b));
}

/** A closed curve through `points`, rounded between them (Catmull-Rom as
 *  Béziers), so a rough outline reads as drawn and not as a polygon. */
function closedCurve(points: Point[]): string {
  const n = points.length;
  const at = (index: number) => points[(index + n) % n]!;
  let path = `M${f(at(0)[0])} ${f(at(0)[1])}`;
  for (let index = 0; index < n; index += 1) {
    const [p0, p1, p2, p3] = [at(index - 1), at(index), at(index + 1), at(index + 2)];
    const c1: Point = [p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6];
    const c2: Point = [p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6];
    path += `C${f(c1[0])} ${f(c1[1])} ${f(c2[0])} ${f(c2[1])} ${f(p2[0])} ${f(p2[1])}`;
  }
  return `${path}Z`;
}

/** The outline of one felt stroke from `x0` to `x1` along `y`, `height`
 *  across. One shorter than it is wide is a dab of the same tip. */
export function felt(x0: number, x1: number, y: number, height: number, random: () => number): string {
  const length = Math.max(height * 0.45, x1 - x0);
  const end = x0 + length;
  const half = height / 2;
  const centre = drifting(random, height * 0.045);
  const width = drifting(random, 0.06);
  // How far along each end shapes the stroke.
  const down = Math.min(length * 0.4, height * 0.5);
  const lift = Math.min(length * 0.5, height * 0.95);
  // The chisel meets the paper at a slant: its top edge comes down first.
  const slant = Math.min(length * 0.18, height * 0.2);
  const steps = Math.max(6, Math.round(length / 2.6));
  const rough = () => (random() - 0.5) * height * 0.07;
  const top: Point[] = [];
  const bottom: Point[] = [];
  for (let index = 0; index <= steps; index += 1) {
    const t = index / steps;
    const x = x0 + length * t;
    const from = x - x0;
    const left = end - x;
    let w = half * (1 + width(t));
    if (from < down) w *= 1.03 + 0.04 * (1 - from / down);
    if (left < lift) w *= 0.8 + 0.2 * Math.sqrt(left / lift);
    const c = y + centre(t);
    const ahead = from < down ? slant * (1 - from / down) : 0;
    top.push([x + ahead, c - w + rough()]);
    bottom.push([x, c + w + rough()]);
  }
  // The finish rounds off where the tip lifts, a little unevenly.
  const last = top.length - 1;
  const tipTop = top[last]!;
  const tipBottom = bottom[last]!;
  const r = (tipBottom[1] - tipTop[1]) / 2;
  const cx = end - r * 0.55;
  const cy = (tipTop[1] + tipBottom[1]) / 2;
  const finish: Point[] = [];
  for (const angle of [-55, -20, 15, 50]) {
    const radius = r * (0.92 + random() * 0.14);
    finish.push([cx + Math.cos((angle * Math.PI) / 180) * radius * 0.9, cy + Math.sin((angle * Math.PI) / 180) * radius]);
  }
  // The start is blunt: a short, nearly straight edge where the tip came down.
  const first = bottom[0]!;
  const startEdge: Point = [x0 - height * 0.04 + slant * 0.4, (first[1] + top[0]![1]) / 2];
  const outline: Point[] = [...top.slice(0, last), ...finish, ...bottom.slice(1, last).reverse(), first, startEdge];
  return closedCurve(outline);
}
