// Something on its way out (a sheet, a menu, a panel, a hint, a bar): it
// plays its leaving keyframes, bound to the registered properties as its
// coming in was (`design/breath.css`), and only then is put away. Asked back
// before that, it stays where it is; where motion is off, it goes at once.

import { reducedMotion } from "../motion";

const going = new WeakMap<Element, () => void>();

export interface Leaving {
  /** The class that plays the leaving: `leaving` unless said. */
  as?: string;
  /** The element whose keyframes say it has gone, where that is not the
   *  one leaving: a sheet's body, inside its dialog. */
  moving?: Element;
}

/** Plays `element`'s leaving, then `gone` puts it away. */
export function leave(element: HTMLElement, gone: () => void, options: Leaving = {}): void {
  const as = options.as ?? "leaving";
  const moving = options.moving ?? element;
  stay(element);
  if (reducedMotion() || !element.isConnected) {
    gone();
    return;
  }
  element.classList.add(as);
  const style = getComputedStyle(moving);
  if (!style.animationName || style.animationName === "none") {
    gone();
    element.classList.remove(as);
    return;
  }
  let timer = 0;
  const ended = (event: Event) => {
    if (event.target === moving) finish();
  };
  const settle = () => {
    moving.removeEventListener("animationend", ended);
    window.clearTimeout(timer);
    going.delete(element);
  };
  const finish = () => {
    settle();
    gone();
    element.classList.remove(as);
  };
  moving.addEventListener("animationend", ended);
  // Should the end not come (the element hidden by something else first),
  // it goes when its keyframes would have ended, a frame later.
  timer = window.setTimeout(finish, longest(style.animationDuration) + 32);
  going.set(element, () => {
    settle();
    element.classList.remove(as);
  });
}

/** Keeps `element` where it is, if it was on its way out. */
export function stay(element: HTMLElement): void {
  going.get(element)?.();
}

/** Shows `element`, or lets it go where it was shown: for a part drawn
 *  again and again as what it shows changes, called each time. */
export function showOrLeave(element: HTMLElement, shown: boolean, options: Leaving = {}): void {
  if (shown) {
    stay(element);
    element.hidden = false;
  } else if (!element.hidden && !leaving(element)) {
    leave(element, () => (element.hidden = true), options);
  }
}

/** Whether `element` is on its way out. */
export function leaving(element: HTMLElement): boolean {
  return going.has(element);
}

/** The longest of the times an `animation-duration` lists, in ms. */
function longest(durations: string): number {
  return Math.max(0, ...durations.split(",").map((one) => {
    const value = parseFloat(one);
    if (!Number.isFinite(value)) return 0;
    return one.trim().endsWith("ms") ? value : value * 1000;
  }));
}
