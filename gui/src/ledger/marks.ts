// The marks a hand makes on a ledger, and no others: a line ruled across
// under the column heads, the double rule over a total, and the stroke
// that says how full something is. Figures and words are never drawn, and
// nothing is ringed: what wants looking at is said by the colour of its
// figure (user 2026-11-15).

import { dot, outline, ruled, ruledPoints, seeded } from "../pen/shapes";

/** One line ruled across, edge to edge: the pen set down before the page's
 *  edge and lifted after it, so no end of it shows. Drawn a thousand units
 *  long and stretched to the width it rules; its thickness does not stretch. */
export function rule(which: 1 | 2 | 3 = 1): string {
  const random = seeded(`ledger-rule-${which}`);
  const d = ruled([-60, 4], [1060, 4], 1, random);
  return `<svg class="lg-rule" viewBox="0 0 1000 8" preserveAspectRatio="none" aria-hidden="true"><path d="${d}"/></svg>`;
}

/** The double rule a total stands under. */
export function doubleRule(): string {
  return `<span class="lg-double" aria-hidden="true">${rule(2)}${rule(3)}</span>`;
}

/** How full, of a hundred: one stroke of the pen along a faint line, as
 *  long as what is used, and a short line across where it stops, if it
 *  stops before the end. `key` keeps a stroke the same each time. */
export function filled(key: string, percent: number, stop?: number): string {
  const random = seeded(`ledger-filled-${key}`);
  const length = (Math.min(Math.max(percent, 0), 100) / 100) * 120;
  const stroke =
    length <= 0
      ? ""
      : length < 5
        ? `<path d="${dot([3, 6], 2.1, random)}"/>`
        : `<path d="${outline(ruledPoints([1.5, 6], [1 + length, 6], random), 4.4, random, { pressure: 0.2, taperStart: 0.03, taperEnd: 0.08 })}"/>`;
  const across = stop === undefined ? "" : `<line x1="${1 + stop * 1.2}" x2="${1 + stop * 1.2}" y1="0.5" y2="11.5"/>`;
  return `<svg viewBox="0 0 122 12" aria-hidden="true"><rect x="1" y="5" width="120" height="2" rx="1"/>${stroke}${across}</svg>`;
}
