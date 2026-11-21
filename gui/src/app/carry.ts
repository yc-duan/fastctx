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
 *  down, which the scroll keeps where the reader was. It is carried by
 *  where its words begin, inside its margin: the margin is narrower beside
 *  the side (`side.css`), and carried by its own edge the words leapt by
 *  what the margin lost before they went (the user, 2026-11-21: "正文内容，
 *  会轻微左下瞬移一小段，然后再进入滑动动画"). */
const CARRIED = ".page-head-title, .page-head-actions, .composer-dock > *, .sp-sheet";
const ACROSS = ".transcript, .starting-scroller > *";
/** The regions that scroll what is carried across. */
const SCROLLERS = ".talk-scroller, .starting-scroller";

/** What is under way, one glide at a time: each part's and region's inline
 *  style to put back, and how it ends. */
let carrying: {
  styles: [HTMLElement, string, string][];
  timer: number;
  waiting: ResizeObserver | null;
  before: Map<HTMLElement, Stood>;
} | null = null;

/** Where a part was seen: its box, and for a column carried across, where
 *  its words began. */
type Stood = { box: DOMRect; words: number };

/** Where `part` stands now. */
function stands(part: HTMLElement, across: boolean): Stood {
  const box = part.getBoundingClientRect();
  return { box, words: across ? box.left + (parseFloat(getComputedStyle(part).paddingLeft) || 0) : box.left };
}

/** Makes `change` to `desk`'s room at once, and carries what stands on the
 *  desk from where it stood to where the change puts it. A glide under way
 *  is taken over from where what it carries is seen now. */
export function carry(desk: HTMLElement, change: () => void): void {
  const glide = reducedMotion() ? 0 : msOf(getComputedStyle(desk).getPropertyValue("--glide"));
  if (!glide) {
    stopCarrying();
    change();
    return;
  }
  const shown = (element: HTMLElement) => element.getClientRects().length > 0;
  const parts = [
    ...[...desk.querySelectorAll<HTMLElement>(CARRIED)].map((part) => ({ part, across: false })),
    ...[...desk.querySelectorAll<HTMLElement>(ACROSS)].map((part) => ({ part, across: true })),
  ].filter(({ part }) => shown(part));
  const scrollers = [...desk.querySelectorAll<HTMLElement>(SCROLLERS)].filter(shown);
  // Where each was last seen: before a change not yet painted, where it
  // stood before that one.
  const unpainted = carrying?.waiting ? carrying.before : null;
  const before = new Map<HTMLElement, Stood>([
    ...parts.map(({ part, across }): [HTMLElement, Stood] => [part, unpainted?.get(part) ?? stands(part, across)]),
    ...scrollers.map((scroller): [HTMLElement, Stood] => [scroller, unpainted?.get(scroller) ?? stands(scroller, false)]),
  ]);
  stopCarrying();
  change();
  // Where the change puts each part is measured once what it sets off has
  // been said: the side's width for the room the desk now has, the room a
  // head keeps beside it, each told by an observer of sizes as the page is
  // next laid out. Observers are told in the order they were made, after the
  // page is laid out and before it is painted: one made now is told last,
  // and nothing is painted between.
  const styles: [HTMLElement, string, string][] = [];
  const waiting = new ResizeObserver(() => {
    waiting.disconnect();
    if (carrying?.waiting !== waiting) return;
    carrying.waiting = null;
    land(desk, glide, parts, scrollers, before, styles);
  });
  carrying = { styles, timer: window.setTimeout(stopCarrying, glide + 1000), waiting, before };
  waiting.observe(desk);
}

/** Carries each part from where it was seen, `before`, to where it stands. */
function land(
  desk: HTMLElement,
  glide: number,
  parts: { part: HTMLElement; across: boolean }[],
  scrollers: HTMLElement[],
  before: Map<HTMLElement, Stood>,
  styles: [HTMLElement, string, string][],
): void {
  const shown = (element: HTMLElement) => element.isConnected && element.getClientRects().length > 0;
  const set = (element: HTMLElement, name: string, value: string) => {
    styles.push([element, name, element.style.getPropertyValue(name)]);
    element.style.setProperty(name, value);
  };
  // Where everything stands is read before anything is set: a region
  // widened one side at a time, read between, was laid out as wide as it
  // had been for a moment, and a view at the foot of lines that had
  // wrapped anew was put back above it, a frame's leap down the page.
  const widened: { scroller: HTMLElement; left: number; right: number; margins: [number, number]; paddings: [number, number] }[] = [];
  for (const scroller of scrollers) {
    if (!shown(scroller)) continue;
    const was = before.get(scroller)!.box;
    const now = scroller.getBoundingClientRect();
    const left = Math.max(0, now.left - was.left);
    const right = Math.max(0, was.right - now.right);
    if (left < 0.5 && right < 0.5) continue;
    const style = getComputedStyle(scroller);
    widened.push({
      scroller,
      left,
      right,
      margins: [parseFloat(style.marginLeft), parseFloat(style.marginRight)],
      paddings: [parseFloat(style.paddingLeft), parseFloat(style.paddingRight)],
    });
  }
  const moved: { part: HTMLElement; x: number; y: number }[] = [];
  for (const { part, across } of parts) {
    if (!shown(part)) continue;
    const was = before.get(part)!;
    const now = stands(part, across);
    const x = was.words - now.words;
    const y = across ? 0 : was.box.top - now.box.top;
    if (Math.abs(x) >= 0.5 || Math.abs(y) >= 0.5) moved.push({ part, x, y });
  }
  // What is carried across a region may reach past its edge as it goes, a
  // column wider than it was past the edge it grows towards: the region
  // scrolls no way across for it, or its bar across came for a part of the
  // glide and took the bar's height from a view kept at its foot.
  for (const scroller of scrollers) {
    if (shown(scroller)) set(scroller, "overflow-x", "hidden");
  }
  // Each region that scrolls reaches where it stood as well as where it now
  // stands, what it holds where it is.
  for (const { scroller, left, right, margins, paddings } of widened) {
    set(scroller, "margin-left", `${margins[0] - left}px`);
    set(scroller, "padding-left", `${paddings[0] + left}px`);
    set(scroller, "margin-right", `${margins[1] - right}px`);
    set(scroller, "padding-right", `${paddings[1] + right}px`);
  }
  // Each is moved on top of how it stands of itself, and what else of it
  // eases goes on easing.
  const own = moved.map(({ part }) => {
    const style = getComputedStyle(part);
    return { transform: style.transform === "none" ? "" : ` ${style.transform}`, transition: style.transition };
  });
  moved.forEach(({ part, x, y }, at) => {
    set(part, "transition", `${own[at]!.transition}, transform 0s`);
    set(part, "transform", `translate(${x}px, ${y}px)${own[at]!.transform}`);
  });
  void desk.offsetWidth;
  moved.forEach(({ part }, at) => {
    part.style.transition = `${own[at]!.transition}, transform ${glide}ms var(--glide-easing)`;
    part.style.transform = `translate(0px, 0px)${own[at]!.transform}`;
  });
  if (carrying) {
    window.clearTimeout(carrying.timer);
    carrying.timer = window.setTimeout(stopCarrying, glide + 50);
  }
}

/** What is carried back where the page has it, carried no further: as a
 *  glide ends, or as the desk is laid out otherwise under it. */
export function stopCarrying(): void {
  const under = carrying;
  if (!under) return;
  window.clearTimeout(under.timer);
  under.waiting?.disconnect();
  carrying = null;
  for (const [element, name, value] of under.styles.reverse()) {
    if (value) element.style.setProperty(name, value);
    else element.style.removeProperty(name);
  }
}
