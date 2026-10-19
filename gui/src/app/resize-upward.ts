// A panel risen over the input whose content changes grows or shrinks
// upward to fit: its foot, nearest the input and where the hand is, stays,
// and its top edge glides to where it now belongs. A panel that keeps its
// size (`steady.ts`) keeps the new one; any other is its content's size
// again once it is there.

import { reducedMotion } from "../motion";
import type { Steady } from "./steady";

/** Makes `change` to what `panel` holds, then carries its height over. */
export function resizeUpward(panel: HTMLElement, steady: Steady | null, change: () => void): void {
  const from = panel.getBoundingClientRect().height;
  panel.classList.remove("resizing-upward");
  steady?.free();
  panel.style.height = "";
  change();
  steady?.hold();
  const to = panel.getBoundingClientRect().height;
  if (reducedMotion() || panel.hidden || !panel.isConnected || Math.abs(to - from) < 1) return;
  const kept = panel.style.height;
  panel.style.height = `${from}px`;
  void panel.offsetHeight;
  panel.classList.add("resizing-upward");
  panel.style.height = `${to}px`;
  const done = (event: TransitionEvent): void => {
    if (event.target !== panel || event.propertyName !== "height") return;
    panel.removeEventListener("transitionend", done);
    panel.classList.remove("resizing-upward");
    panel.style.height = kept;
  };
  panel.addEventListener("transitionend", done);
}
