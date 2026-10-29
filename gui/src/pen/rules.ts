// Drawn marks for the page itself, made once as SVG images and used as
// masks, so CSS gives them the theme's colour: hairlines ruled with the
// ruling pen, across and down, a marker's swash for what is set off, and
// its underline.
// Several rules are drawn from different seeds, so the rules on one page
// are not copies of each other.

import { penDefs } from "./filters";
import { outline, ruled, seeded, type Point } from "./shapes";

function image(width: number, height: number, filter: string, d: string): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><defs>${penDefs()}</defs><g filter="url(#${filter})"><path d="${d}" fill="white"/></g></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}

/** A marker's swash: one wide stroke across, a little uneven, its ends
 *  ragged where the marker landed and lifted. */
function swash(seed: string): string {
  const width = 900;
  const height = 60;
  const random = seeded(seed);
  const points: Point[] = [];
  for (let x = 6; x <= width - 6; x += 12) points.push([x, height / 2 + (random() - 0.5) * 3 + Math.sin(x / 140) * 2]);
  return image(width, height, "pen-marker", outline(points, height * 0.72, random, { pressure: 0.12, taper: 0.015 }));
}

/** An underline drawn under a title: short, a touch rising, firmer at the
 *  start than the end. */
function underline(seed: string): string {
  const width = 300;
  const height = 14;
  const random = seeded(seed);
  const points: Point[] = [];
  for (let x = 4; x <= width - 4; x += 8) points.push([x, height / 2 + 2 - (x / width) * 3 + (random() - 0.5) * 0.8]);
  return image(width, height, "pen-marker", outline(points, 3.6, random, { pressure: 0.3, taper: 0.1 }));
}

/** Sets the page's drawn marks as custom properties on the root:
 *  `--pen-rule-1..3` across, `--pen-rule-down` down, `--pen-swash`,
 *  `--pen-underline`. */
export function installRules(): void {
  const root = document.documentElement.style;
  for (const index of [1, 2, 3]) {
    root.setProperty(`--pen-rule-${index}`, image(900, 8, "pen-rule", ruled([2, 4], [898, 4], 1.25, seeded(`rule-${index}`))));
  }
  root.setProperty("--pen-rule-down", image(8, 900, "pen-rule", ruled([4, 2], [4, 898], 1.6, seeded("rule-down"))));
  root.setProperty("--pen-swash", swash("swash"));
  root.setProperty("--pen-underline", underline("underline"));
}
