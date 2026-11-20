// The desk carried through a change of its room (the user, 2026-11-19: the
// sidebar and the side should push the page aside softly, as they once did,
// not have it leap before they slide). The desk is laid out once for where
// the change leaves it; what stands on it is then drawn where it stood, and
// moved to where it stands over one glide, as the sidebar or the side slides
// in step. Laid out once, a long conversation is set once rather than every
// moment of the glide, which made it halt (5a0df94fe), and nothing on it is
// drawn anew as it goes: it is only moved.
//
// What is carried may stand, as it sets out, past the edges of the room it
// is carried in: those edges cut nothing while it goes. A region that
// scrolls cannot leave what is in it uncut, so it is widened over the glide
// to reach where it stood and where it goes, what it holds laid out where
// the change put it.

import { msOf, reducedMotion } from "../motion";

/** What is carried: the parts of a conversation's page that stand where the
 *  room around them puts them, and the side's sheet. A column the page
 *  scrolls is carried across only: what its lines set again moves it up or
 *  down, which the scroll keeps where the reader was. */
const CARRIED = ".page-head-title, .page-head-actions, .composer-dock > *, .sp-sheet";
const ACROSS = ".transcript, .starting-scroller > *";
/** The regions that scroll what is carried across. */
const SCROLLERS = ".talk-scroller, .starting-scroller";

/** What is under way, one glide at a time: each part's and region's inline
 *  style to put back, and when the glide ends. */
let carrying: { styles: [HTMLElement, string, string][]; timer: number } | null = null;

/** Makes `change` to `desk`'s room at once, and carries what stands on the
 *  desk from where it stood to where the change puts it. A glide under way
 *  is taken over from where what it carries is seen now. */
export function carry(desk: HTMLElement, change: () => void): void {
  const glide = reducedMotion() ? 0 : msOf(getComputedStyle(desk).getPropertyValue("--glide"));
  if (!glide) {
    letGo();
    change();
    return;
  }
  const shown = (element: HTMLElement) => element.getClientRects().length > 0;
  const parts = [
    ...[...desk.querySelectorAll<HTMLElement>(CARRIED)].map((part) => ({ part, across: false })),
    ...[...desk.querySelectorAll<HTMLElement>(ACROSS)].map((part) => ({ part, across: true })),
  ].filter(({ part }) => shown(part));
  const scrollers = [...desk.querySelectorAll<HTMLElement>(SCROLLERS)].filter(shown);
  const before = new Map([...parts.map(({ part }) => part), ...scrollers].map((element) => [element, element.getBoundingClientRect()]));
  letGo();
  change();
  const styles: [HTMLElement, string, string][] = [];
  const set = (element: HTMLElement, name: string, value: string) => {
    styles.push([element, name, element.style.getPropertyValue(name)]);
    element.style.setProperty(name, value);
  };
  // Each region that scrolls reaches where it stood as well as where it now
  // stands, what it holds where it is.
  for (const scroller of scrollers) {
    const was = before.get(scroller)!;
    if (!scroller.isConnected || !shown(scroller)) continue;
    const now = scroller.getBoundingClientRect();
    const left = Math.max(0, now.left - was.left);
    const right = Math.max(0, was.right - now.right);
    if (left < 0.5 && right < 0.5) continue;
    const style = getComputedStyle(scroller);
    set(scroller, "margin-left", `${parseFloat(style.marginLeft) - left}px`);
    set(scroller, "padding-left", `${parseFloat(style.paddingLeft) + left}px`);
    set(scroller, "margin-right", `${parseFloat(style.marginRight) - right}px`);
    set(scroller, "padding-right", `${parseFloat(style.paddingRight) + right}px`);
  }
  const moved: { part: HTMLElement; x: number; y: number }[] = [];
  for (const { part, across } of parts) {
    if (!part.isConnected || !shown(part)) continue;
    const was = before.get(part)!;
    const now = part.getBoundingClientRect();
    const x = was.left - now.left;
    const y = across ? 0 : was.top - now.top;
    if (Math.abs(x) >= 0.5 || Math.abs(y) >= 0.5) moved.push({ part, x, y });
  }
  if (!moved.length) {
    for (const [element, name, value] of styles.reverse()) element.style.setProperty(name, value);
    return;
  }
  for (const { part, x, y } of moved) {
    set(part, "transition", "none");
    set(part, "transform", `translate(${x}px, ${y}px)`);
  }
  void desk.offsetWidth;
  for (const { part } of moved) {
    part.style.transition = `transform ${glide}ms var(--glide-easing)`;
    part.style.transform = "";
  }
  carrying = { styles, timer: window.setTimeout(letGo, glide + 50) };
}

/** What is carried back where the page has it, carried no further. */
function letGo(): void {
  const under = carrying;
  if (!under) return;
  window.clearTimeout(under.timer);
  carrying = null;
  for (const [element, name, value] of under.styles.reverse()) {
    if (value) element.style.setProperty(name, value);
    else element.style.removeProperty(name);
  }
}
