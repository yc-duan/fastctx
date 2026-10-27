// Long lists drawn as far as they are read: past a number of rows the rest
// wait under a mark at the list's end, and more are drawn as the mark comes
// within a screenful of the bottom of what scrolls the list. The list of every
// conversation, the machines page's list and update view, the picker of
// agents and the choice of machines are drawn so.

/** How far below what is seen the mark starts more being drawn. */
const AHEAD = "0px 0px 800px 0px";

/** The mark at the end of the rows drawn, where more wait. */
export const readOn = `<div class="read-on" aria-hidden="true"></div>`;

/** Watches the mark in `list`, the end of its rows drawn, coming near the
 *  bottom of `scroller`, what scrolls the list; `more` then draws more of
 *  them. Says how to stop watching. Nothing is watched where there is no
 *  mark: every row is drawn. */
export function whenNear(list: ParentNode, scroller: Element | null, more: () => void): () => void {
  const mark = list.querySelector(".read-on");
  if (!mark) return () => {};
  const watch = new IntersectionObserver(
    (entries) => {
      if (entries.some((entry) => entry.isIntersecting)) more();
    },
    { root: scroller, rootMargin: AHEAD },
  );
  watch.observe(mark);
  return () => watch.disconnect();
}
