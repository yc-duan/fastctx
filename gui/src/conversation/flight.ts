// Words sent rise from where they were typed into their place in the
// conversation (`world-gui.md`, 界面的结构): the very words leave the input,
// gather their ground as they go up, and settle where the conversation
// keeps them. What flies is a copy over the page, since the conversation's
// own region clips what lies outside it; the words themselves wait unseen
// in their place, which it follows to the end however the conversation
// moves meanwhile. It goes up a little sooner than it goes across, so it
// travels on a curve. Where the system asks for less motion the words are
// simply there.

import { curve } from "../design/curve";
import { time } from "./turn-line";

/** Where words leave from: the top left of the first line of the input they
 *  were typed in, the colour they were typed in, and a way to keep the
 *  input quiet (no placeholder over the words) until they have gone. */
export interface Departure {
  left: number;
  top: number;
  color: string;
  hush?: () => () => void;
}

/** Where words typed in `input` leave from; its placeholder is kept out of
 *  the way while they go. */
export function departureOf(input: HTMLTextAreaElement): Departure | null {
  if (!input.isConnected || input.closest("[hidden]")) return null;
  const box = input.getBoundingClientRect();
  const style = getComputedStyle(input);
  return {
    left: box.left + parseFloat(style.paddingLeft),
    top: box.top + parseFloat(style.paddingTop),
    color: style.color,
    hush: () => {
      input.classList.add("hushed");
      return () => input.classList.remove("hushed");
    },
  };
}

/** Sends the bubble `find` returns from where it was typed. `find` is asked
 *  each frame: the bubble may be drawn again on its way. */
export function fly(find: () => HTMLElement | null, from: Departure, done: () => void = () => {}): void {
  const bubble = find();
  if (!bubble) return;
  const style = getComputedStyle(bubble);
  const duration = time(style, "--send");
  if (duration <= 0) return;
  const rise = bezier(style.getPropertyValue("--send-easing"));
  const drift = bezier(style.getPropertyValue("--send-drift"));
  const landing = bubble.getBoundingClientRect();
  // Where its words begin inside it, so they leave from where the typed
  // words stood.
  const inset = { left: parseFloat(style.paddingLeft), top: parseFloat(style.paddingTop) };
  const start = { x: from.left - inset.left, y: from.top - inset.top };
  const ghost = bubble.cloneNode(true) as HTMLElement;
  ghost.classList.add("bubble-ghost");
  ghost.setAttribute("aria-hidden", "true");
  Object.assign(ghost.style, {
    position: "fixed",
    left: "0",
    top: "0",
    width: `${landing.width}px`,
    maxWidth: "none",
    margin: "0",
  });
  // Where it is moves registered properties (`turns.css`), so the words in
  // flight stay smooth on Windows.
  const place = (x: number, y: number) => {
    ghost.style.setProperty("--fly-x", `${x}px`);
    ghost.style.setProperty("--fly-y", `${y}px`);
  };
  place(start.x, start.y);
  document.body.append(ghost);
  bubble.style.visibility = "hidden";
  const hushed = from.hush?.() ?? (() => {});
  // Its ground and its ink come as it rises.
  ghost.animate(
    [
      { backgroundColor: "transparent", color: from.color, boxShadow: "none" },
      { backgroundColor: style.backgroundColor, color: style.color },
    ],
    { duration: duration * 0.25, easing: curve("--ease-out"), fill: "backwards" },
  );
  const began = performance.now();
  const hidden = new Set<HTMLElement>([bubble]);
  const frame = (now: number) => {
    const target = find();
    const progress = Math.min(1, (now - began) / duration);
    if (!target || !target.isConnected || progress >= 1) {
      ghost.remove();
      hushed();
      for (const element of hidden) element.style.visibility = "";
      if (target?.isConnected) settle(target);
      done();
      return;
    }
    if (!hidden.has(target)) {
      target.style.visibility = "hidden";
      hidden.add(target);
    }
    const to = target.getBoundingClientRect();
    const x = start.x + (to.left - start.x) * drift(progress);
    const y = start.y + (to.top - start.y) * rise(progress);
    place(x, y);
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}

/** What came with the words (pictures above them, a note beneath) comes
 *  up quietly once they have landed. */
function settle(bubble: HTMLElement): void {
  const block = bubble.closest(".block");
  if (!block) return;
  for (const other of block.children) {
    if (other === bubble || !(other instanceof HTMLElement)) continue;
    other.classList.add("bubble-landing");
    other.addEventListener("animationend", () => other.classList.remove("bubble-landing"), { once: true });
  }
}

/** A CSS `cubic-bezier()` as a function of progress. */
export function bezier(easing: string): (t: number) => number {
  const numbers = /cubic-bezier\(([^)]+)\)/.exec(easing)?.[1]?.split(",").map(Number);
  if (!numbers || numbers.length !== 4 || numbers.some((n) => !Number.isFinite(n))) throw new Error(`not a cubic-bezier easing: ${easing}`);
  const [x1, y1, x2, y2] = numbers as [number, number, number, number];
  const at = (a: number, b: number, t: number) => 3 * a * t * (1 - t) ** 2 + 3 * b * t * t * (1 - t) + t ** 3;
  const slope = (a: number, b: number, t: number) => 3 * a * (1 - t) ** 2 + 6 * (b - a) * t * (1 - t) + 3 * (1 - b) * t * t;
  return (x) => {
    if (x <= 0) return 0;
    if (x >= 1) return 1;
    // Newton's method on x(t) = x, then y(t).
    let t = x;
    for (let step = 0; step < 8; step += 1) {
      const error = at(x1, x2, t) - x;
      const d = slope(x1, x2, t);
      if (Math.abs(error) < 1e-5 || d === 0) break;
      t = Math.min(1, Math.max(0, t - error / d));
    }
    return at(y1, y2, t);
  };
}
