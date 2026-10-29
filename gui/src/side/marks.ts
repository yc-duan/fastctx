// How a goal stands, as a hand marks it wherever a goal is drawn (its page
// and the conversation's whole, `goal-page.ts` and `map.ts`): a drop of ink
// that breathes while it is worked at, two strokes while it waits, a bar in
// clay where it is stopped, a tick in clay once it is done. The strokes are
// laid out on a 16 by 18 field and drawn at the size asked for.

import { Sketch } from "../pen/sketch";
import type { GoalStatus } from "./types";

export function goalMark(status: GoalStatus, seed: string, size = 16, className = "gp-mark-svg"): string {
  const s = size / 16;
  const at = (x: number, y: number): [number, number] => [x * s, y * s];
  const sketch = new Sketch(seed);
  if (status === "active") sketch.blot(at(8, 9), 3.4 * s, "current");
  else if (status === "paused") sketch.rule(at(5.5, 4.5), at(5.5, 13.5), 1.6 * s, "graphite").rule(at(10.5, 4.5), at(10.5, 13.5), 1.6 * s, "graphite");
  else if (status === "blocked") sketch.rule(at(3, 9.2), at(13, 8.6), 2.4 * s, "clay");
  else sketch.stroke([at(2.5, 9), at(6.2, 12.8), at(13.5, 4)], 1.8 * s, "clay");
  return sketch.svg(16 * s, 18 * s, className);
}
