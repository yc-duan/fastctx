// The key of a conversation's usage pop-up (`usage-pop.ts`): a stroke of
// marker that names an ink, in the pen of the account's usage page
// (`app/usage/draw.ts`).

import { seeded, swatch } from "../pen/shapes";

/** A short stroke of marker in `ink`, where a key names what it stands for. */
export function inkMark(ink: string, seed: string): string {
  return `<svg class="talk-usage-mark" viewBox="0 0 16 8" aria-hidden="true"><g filter="url(#pen-marker-lite)"><path style="fill:${ink}" d="${swatch(1, 1.5, 14, 5, seeded(`mark:${seed}`), 1.6)}"/></g></svg>`;
}
