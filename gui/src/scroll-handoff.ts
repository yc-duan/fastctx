// A box that scrolls inside a page that scrolls (a long output, an agent's
// own conversation, a plan): the wheel moves the box while the box can move
// that way, and at its edge the same turn of the wheel moves the page. The
// browser latches a wheel gesture to the box it began over, so without this
// a box at its end holds the page still for as long as the wheel turns.

import { reducedMotion } from "./motion";

const handed = new WeakSet<HTMLElement>();

/** Lets the wheel go on into the page at `box`'s edges. */
export function handOnAtEdge(box: HTMLElement): void {
  if (handed.has(box)) return;
  handed.add(box);
  box.addEventListener(
    "wheel",
    (event) => {
      if (event.defaultPrevented || event.ctrlKey || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? box.clientHeight : 1);
      if (!delta || canMove(box, delta)) return;
      const outer = movingWay(box.parentElement, delta);
      if (!outer) return;
      event.preventDefault();
      glide(outer, delta);
    },
    { passive: false },
  );
}

/** Whether `element` can still scroll some way in the direction of
 *  `delta`. */
function canMove(element: HTMLElement, delta: number): boolean {
  return delta < 0 ? element.scrollTop > 0.5 : element.scrollTop + element.clientHeight < element.scrollHeight - 0.5;
}

/** The nearest box from `from` outward that scrolls and can still move in
 *  the direction of `delta`. */
function movingWay(from: HTMLElement | null, delta: number): HTMLElement | null {
  for (let element = from; element; element = element.parentElement) {
    if (!/(auto|scroll)/.test(getComputedStyle(element).overflowY)) continue;
    if (canMove(element, delta)) return element;
  }
  return null;
}

/** Where each gliding box is headed and its frame. */
const glides = new WeakMap<HTMLElement, { target: number; frame: number }>();

/** Moves `element` by `delta` the way a wheel moves it: eased over a few
 *  frames, a turn that comes while it moves adding to where it is headed;
 *  at once where motion is off. */
function glide(element: HTMLElement, delta: number): void {
  const most = element.scrollHeight - element.clientHeight;
  if (reducedMotion()) {
    element.scrollTop = Math.max(0, Math.min(most, element.scrollTop + delta));
    return;
  }
  const going = glides.get(element);
  const target = Math.max(0, Math.min(most, (going ? going.target : element.scrollTop) + delta));
  if (going) {
    going.target = target;
    return;
  }
  const state = { target, frame: 0 };
  glides.set(element, state);
  const step = (): void => {
    const left = state.target - element.scrollTop;
    if (Math.abs(left) < 1) {
      element.scrollTop = state.target;
      glides.delete(element);
      return;
    }
    const before = element.scrollTop;
    element.scrollTop = before + (Math.abs(left) < 3 ? Math.sign(left) : left * 0.3);
    // Something else moved it, or it can go no further: stop here.
    if (Math.abs(element.scrollTop - before) < 0.5) {
      glides.delete(element);
      return;
    }
    state.frame = requestAnimationFrame(step);
  };
  state.frame = requestAnimationFrame(step);
}
