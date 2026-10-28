// The conversation's column carried through a change of the room beside it
// (`world-gui.md`, 对话的侧面板): as the sidebar or the panel beside a
// conversation comes or goes, the room the column has changes over a glide.
// Its lines broken again at every frame of it, the words the reader was on
// would shiver all the way; instead the column takes at once the width it
// will end with, so its lines break once, and is carried from where its
// words stood to where they will stand as the room changes round it. Where
// the lines break anew, the words dip and come back up as they settle.

import { msOf, reducedMotion } from "../motion";

const holding = new WeakMap<HTMLElement, number>();

/** `page`'s conversation is about to have `by` pixels more room across, or
 *  fewer where negative, over the page's `--glide`. */
export function holdColumn(page: HTMLElement, by: number): void {
  release(page);
  const scroller = page.querySelector<HTMLElement>(".talk-scroller");
  const column = scroller?.querySelector<HTMLElement>(":scope > .transcript");
  if (!scroller || !column || !by || reducedMotion()) return;
  const glide = msOf(getComputedStyle(page).getPropertyValue("--glide"));
  if (!glide) return;
  // The room inside the scroller to the fraction of a pixel, less what it
  // keeps for its bar.
  const room = scroller.getBoundingClientRect().width - (scroller.offsetWidth - scroller.clientWidth);
  const most = parseFloat(getComputedStyle(column).maxWidth);
  const width = column.getBoundingClientRect().width;
  const next = Math.min(Number.isFinite(most) ? most : Infinity, room + by);
  if (!(next > 0)) return;
  // Centred at its next width in the room as it is now, the column would
  // move by the difference; it is carried back from there to where it is.
  const shift = Math.max(0, (room - width) / 2) - Math.max(0, (room - next) / 2);
  page.style.setProperty("--held", `${next}px`);
  page.style.setProperty("--held-shift", `${shift}px`);
  page.classList.add("column-held");
  void column.offsetWidth;
  page.classList.add("column-moving");
  if (Math.abs(next - width) >= 0.5) {
    column.classList.remove("column-dip");
    void column.offsetWidth;
    column.classList.add("column-dip");
  }
  holding.set(
    page,
    window.setTimeout(() => release(page), glide + 50),
  );
}

/** The column back in the page's hands, at the width it now has anyway. */
function release(page: HTMLElement): void {
  const timer = holding.get(page);
  if (timer === undefined) return;
  window.clearTimeout(timer);
  holding.delete(page);
  page.classList.remove("column-held", "column-moving");
  page.style.removeProperty("--held");
  page.style.removeProperty("--held-shift");
  page.querySelector(".talk-scroller > .transcript")?.classList.remove("column-dip");
}
