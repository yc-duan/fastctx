// Long lists drawn as far as they are read: past a number of rows the rest
// wait under a mark at the list's end, and more are drawn as the mark comes
// within a screenful of the bottom of what scrolls the list. The list of every
// conversation, the machines page's list and update view, the picker of
// agents and the choice of machines are drawn so.

/** How far below what is seen the mark starts more being drawn. */
const AHEAD = "0px 0px 800px 0px";

/** The mark at the end of the rows drawn, where more wait. */
export const readOn = `<div class="read-on" aria-hidden="true"></div>`;

/** Watches the mark in `list`, the end of its rows drawn, coming near;
 *  `more` then draws more of them. Says how to stop watching. Nothing is
 *  watched where there is no mark: every row is drawn. */
export function whenNear(list: ParentNode, more: () => void): () => void {
  const mark = list.querySelector(".read-on");
  if (!mark) return () => {};
  const watch = new IntersectionObserver(
    (entries) => {
      if (entries.some((entry) => entry.isIntersecting)) more();
    },
    { root: scrollerOf(mark), rootMargin: AHEAD },
  );
  watch.observe(mark);
  return () => watch.disconnect();
}

/** The nearest box that scrolls `element` up and down; the window where
 *  none does. */
function scrollerOf(element: Element): Element | null {
  for (let at = element.parentElement; at; at = at.parentElement) {
    const { overflowY } = getComputedStyle(at);
    if (overflowY === "auto" || overflowY === "scroll") return at;
  }
  return null;
}
