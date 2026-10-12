// On a touch screen there is no pointer to rest on a mark, so what a pointer
// shows on hover is shown on a long press instead: holding a finger on
// anything with a `title` raises it in a small solid bubble above the
// finger, and the tap that would have followed does nothing
// (`world-gui.md`, 界面的结构: 指针停住才出现的说明，换成长按出现).

import { safeTop } from "../system-bars";
import { layerOpened } from "./back";

/** How long a finger rests before it is a long press, as Android counts it. */
const LONG_PRESS_MS = 500;
/** How far a finger may drift and still be resting, in CSS pixels. */
const SLOP = 10;

export function installHints(): void {
  if (!matchMedia("(hover: none)").matches) return;
  let timer = 0;
  let start: { x: number; y: number } | null = null;
  let bubble: HTMLElement | null = null;
  let swallow = false;

  const hide = () => {
    bubble?.remove();
    bubble = null;
  };
  const cancel = () => {
    clearTimeout(timer);
    start = null;
  };

  document.addEventListener(
    "pointerdown",
    (event) => {
      hide();
      swallow = false;
      if (event.pointerType === "mouse") return;
      // A place that answers a long press itself (a menu, a drag) keeps it.
      if ((event.target as HTMLElement | null)?.closest("[data-own-press]")) return;
      const target = (event.target as HTMLElement | null)?.closest<HTMLElement>("[title]");
      const text = target?.getAttribute("title")?.trim();
      if (!target || !text) return;
      start = { x: event.clientX, y: event.clientY };
      timer = window.setTimeout(() => {
        start = null;
        swallow = true;
        bubble = document.createElement("div");
        bubble.className = "touch-hint";
        bubble.setAttribute("role", "tooltip");
        bubble.textContent = text;
        document.body.append(bubble);
        const box = target.getBoundingClientRect();
        const width = bubble.offsetWidth;
        const left = Math.min(Math.max(8, box.left + box.width / 2 - width / 2), innerWidth - width - 8);
        const above = box.top - bubble.offsetHeight - 8;
        bubble.style.left = `${left}px`;
        // Not under the status bar, which a phone draws over the page.
        bubble.style.top = `${above >= safeTop() + 8 ? above : box.bottom + 8}px`;
        const shown = bubble;
        layerOpened({ isOpen: () => shown.isConnected, close: hide });
        navigator.vibrate?.(8);
      }, LONG_PRESS_MS);
    },
    { capture: true, passive: true },
  );
  document.addEventListener(
    "pointermove",
    (event) => {
      if (start && Math.hypot(event.clientX - start.x, event.clientY - start.y) > SLOP) cancel();
    },
    { capture: true, passive: true },
  );
  document.addEventListener("pointerup", cancel, { capture: true, passive: true });
  document.addEventListener("pointercancel", cancel, { capture: true, passive: true });
  // The tap a long press ends in, and the system's own long-press menu, give
  // way to the hint.
  document.addEventListener(
    "click",
    (event) => {
      if (!swallow) return;
      swallow = false;
      event.preventDefault();
      event.stopPropagation();
    },
    { capture: true },
  );
  document.addEventListener(
    "contextmenu",
    (event) => {
      if (bubble || swallow) event.preventDefault();
    },
    { capture: true },
  );
  document.addEventListener("scroll", hide, { capture: true, passive: true });
}
