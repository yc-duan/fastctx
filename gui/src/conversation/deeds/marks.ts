// How a deed came out, as the pen marks it in its card (`world-gui.md`,
// 审美): a pencil tick where it went well, a drop of ink for each thing that
// went wrong or wants a look. No figures: they are said beside them. Seeded
// by what they mark, so a mark is the same each time it is drawn.

import { Sketch } from "../../pen/sketch";

/** Drops drawn before the rest are counted. */
const DROPS = 3;

/** A drawing at its size in the page. */
function sized(svg: string, width: number, height: number): string {
  return svg.replace("<svg ", `<svg width="${width}" height="${height}" `);
}

/** Done well. */
export function tick(seed: string): string {
  return `<span class="dm dm-tick">${sized(new Sketch(`tick ${seed}`).stroke([[2.4, 8.6], [6, 12], [13.6, 3.4]], 2, "current", 0.24).svg(16, 16, "dm-pen"), 16, 16)}</span>`;
}

/** Things that went wrong, or want a look: a drop of ink each, the colour
 *  of `tone`, a count after the first few. */
export function drops(seed: string, count: number, tone: "wrong" | "caution"): string {
  if (count <= 0) return "";
  const drawn = Math.min(count, DROPS);
  const sketch = new Sketch(`drops ${tone} ${seed}`);
  for (let index = 0; index < drawn; index += 1) sketch.blot([5 + index * 12, 7], 4.2, "current");
  const more = count > DROPS ? `<span class="dm-more">+${count - DROPS}</span>` : "";
  return `<span class="dm dm-drops ${tone}">${sized(sketch.svg(drawn * 12 - 2, 14, "dm-pen"), drawn * 12 - 2, 14)}${more}</span>`;
}

