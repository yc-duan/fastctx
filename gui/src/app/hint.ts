// On a touch screen there is no pointer to rest on a mark, so what a pointer
// shows on hover is shown on a long press instead: holding a finger on
// anything with a `title` raises it in a small solid bubble above the
// finger, when the phone's own long press comes (`press.ts`;
// `world-gui.md`, 界面的结构: 指针停住才出现的说明，换成长按出现).

import { safeTop } from "../system-bars";
import { layerOpened } from "./back";
import { inputOf, pressFelt } from "./press";

export function installHints(): void {
  let bubble: HTMLElement | null = null;
  const hide = () => {
    bubble?.remove();
    bubble = null;
  };

  document.addEventListener("pointerdown", hide, { capture: true, passive: true });
  document.addEventListener("scroll", hide, { capture: true, passive: true });
  // After the places that answer a long press themselves, which have taken
  // the event by then.
  document.addEventListener("contextmenu", (event) => {
    if (event.defaultPrevented || inputOf(event) !== "touch") return;
    const at = event.target as HTMLElement | null;
    if (!at || at.closest("[data-own-press], input, textarea, [contenteditable]")) return;
    const target = at.closest<HTMLElement>("[title]");
    const text = target?.getAttribute("title")?.trim();
    if (!target || !text) return;
    event.preventDefault();
    pressFelt();
    hide();
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
  });
}
