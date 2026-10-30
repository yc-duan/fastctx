// Where a fork came from, shown as it opens (`world-gui.md`, 分叉): the
// turn it is made from is washed a moment in clay, and a copy of the card of
// where it came from lifts off that turn and is carried over to the head of
// the fork's place, following the place as it opens, over one glide. The
// card itself waits unseen until the copy lands on it. Its place, size and
// lift move by layout, not by a transform, so its words stay sharp on
// Windows as they go.

import { bezier } from "../conversation/flight";
import { msOf, reducedMotion } from "../motion";

/** Carries the card from `from` to where `card` stands. */
export function fly(from: Element | null, card: HTMLElement): void {
  if (!from?.isConnected || reducedMotion()) return;
  const style = getComputedStyle(document.documentElement);
  const ms = msOf(style.getPropertyValue("--glide"));
  if (ms <= 0) return;
  const ease = bezier(style.getPropertyValue("--glide-easing"));
  from.classList.remove("fk-from-turn");
  void (from as HTMLElement).offsetWidth;
  from.classList.add("fk-from-turn");
  from.addEventListener("animationend", () => from.classList.remove("fk-from-turn"), { once: true });
  const start = seen(from);
  const ghost = card.cloneNode(true) as HTMLElement;
  ghost.classList.add("fk-ghost");
  ghost.removeAttribute("title");
  ghost.setAttribute("aria-hidden", "true");
  ghost.tabIndex = -1;
  card.classList.add("fk-waiting");
  document.body.append(ghost);
  // It lifts off where the turn's words are seen, no wider than a card is.
  const startWidth = Math.min(start.width, 420);
  const began = performance.now();
  const step = (now: number) => {
    const t = Math.min(1, (now - began) / ms);
    const at = ease(t);
    const end = card.getBoundingClientRect();
    ghost.style.left = `${start.left + (end.left - start.left) * at}px`;
    ghost.style.top = `${start.top + (end.top - start.top) * at}px`;
    ghost.style.width = `${startWidth + (end.width - startWidth) * at}px`;
    ghost.style.setProperty("--fk-lift", String(Math.sin(Math.PI * t)));
    if (t < 1 && card.isConnected) requestAnimationFrame(step);
    else {
      ghost.remove();
      card.classList.remove("fk-waiting");
    }
  };
  requestAnimationFrame(step);
}

/** The part of `element` in view: cut to what scrolls it, and to the window. */
function seen(element: Element): DOMRect {
  const box = element.getBoundingClientRect();
  let top = Math.max(box.top, 0);
  let bottom = Math.min(box.bottom, innerHeight);
  for (let at = element.parentElement; at; at = at.parentElement) {
    if (!/(auto|scroll)/.test(getComputedStyle(at).overflowY)) continue;
    const clip = at.getBoundingClientRect();
    top = Math.max(top, clip.top);
    bottom = Math.min(bottom, clip.bottom);
    break;
  }
  return new DOMRect(box.left, top, box.width, Math.max(0, bottom - top));
}
