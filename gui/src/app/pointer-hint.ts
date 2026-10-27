// What a pointer shows on resting on a mark with a `title`: the same solid
// bubble a long press raises on a touch screen, drawn by the interface in
// the theme's colours rather than the system's box (`world-gui.md`, 界面的结构).
// It comes soon after the pointer rests; once one is up, moving to the next
// mark changes it at once. The system's box never shows: a mark's `title`
// is held aside while the pointer is on it and given back as it leaves, and
// a page that writes a new one meanwhile changes the bubble instead.
//
// The bubble is a popover, so it stands over a modal sheet as well.

import { leave } from "./leave";

/** How long a pointer rests on a mark before its hint comes. */
const REST_MS = 200;
/** After a hint went, the next one comes at once within this long. */
const WARM_MS = 400;
/** Taller marks (a row, a card) take the hint under the pointer, not under
 *  the whole mark. */
const TALL = 64;

export function installPointerHints(): void {
  /** The mark the pointer is on, its `title` held aside, and that text. */
  let mark: HTMLElement | null = null;
  let held = "";
  let bubble: HTMLElement | null = null;
  let timer = 0;
  let warmUntil = 0;
  /** Pressed: the hint stays away until the pointer moves to another mark. */
  let pressed = false;
  const pointer = { x: 0, y: 0 };

  // A page may write the mark's `title` while the pointer rests on it.
  const watch = new MutationObserver(() => {
    if (!mark) return;
    const written = mark.getAttribute("title");
    if (written === null) return;
    held = written.trim();
    mark.removeAttribute("title");
    if (bubble) held ? show() : hide();
  });

  /** `fade`: the pointer left every mark, and the hint goes softly; a
   *  press, a key or a scroll takes it away at once. */
  const hide = (fade = false) => {
    clearTimeout(timer);
    if (!bubble) return;
    const going = bubble;
    bubble = null;
    warmUntil = performance.now() + WARM_MS;
    if (fade) leave(going, () => going.remove());
    else going.remove();
  };

  const show = () => {
    if (!mark || !held || pressed || !mark.isConnected) {
      hide();
      return;
    }
    if (!bubble) {
      bubble = document.createElement("div");
      bubble.className = "pointer-hint";
      bubble.setAttribute("role", "tooltip");
      bubble.setAttribute("popover", "manual");
      document.body.append(bubble);
      (bubble as HTMLElement & { showPopover?: () => void }).showPopover?.();
    }
    bubble.textContent = held;
    const box = mark.getBoundingClientRect();
    const width = bubble.offsetWidth;
    const height = bubble.offsetHeight;
    const left = Math.min(Math.max(8, pointer.x - width / 2), innerWidth - width - 8);
    const under = box.height > TALL ? pointer.y + 20 : box.bottom + 6;
    const over = (box.height > TALL ? pointer.y - 12 : box.top - 6) - height;
    bubble.style.left = `${left}px`;
    bubble.style.top = `${under + height <= innerHeight - 8 || over < 8 ? under : over}px`;
  };

  /** Gives the mark its `title` back as the pointer leaves it. */
  const giveBack = () => {
    watch.disconnect();
    if (mark && held && !mark.hasAttribute("title")) mark.setAttribute("title", held);
    mark = null;
    held = "";
  };

  /** The pointer is now on `next`, or on no mark. */
  const onto = (next: HTMLElement | null) => {
    if (next === mark) return;
    giveBack();
    pressed = false;
    clearTimeout(timer);
    if (!next) {
      hide(true);
      return;
    }
    mark = next;
    held = next.getAttribute("title")?.trim() ?? "";
    next.removeAttribute("title");
    watch.observe(next, { attributes: true, attributeFilter: ["title"] });
    if (bubble || performance.now() < warmUntil) show();
    else timer = window.setTimeout(show, REST_MS);
  };

  document.addEventListener(
    "pointerover",
    (event) => {
      if (event.pointerType === "touch") return;
      pointer.x = event.clientX;
      pointer.y = event.clientY;
      const target = event.target as Element;
      const nearest = target.closest?.<HTMLElement>("[title]") ?? null;
      // Within the mark held, a mark inside it takes over; one around it
      // does not.
      if (mark?.contains(target)) onto(nearest && mark.contains(nearest) ? nearest : mark);
      else onto(nearest);
    },
    { capture: true, passive: true },
  );
  document.addEventListener(
    "pointermove",
    (event) => {
      if (event.pointerType === "touch") return;
      pointer.x = event.clientX;
      pointer.y = event.clientY;
      // A page drawn anew under a resting pointer takes its mark away.
      if (mark && !mark.isConnected) onto(null);
    },
    { capture: true, passive: true },
  );
  document.addEventListener(
    "pointerout",
    (event) => {
      if (event.pointerType !== "touch" && !event.relatedTarget) onto(null);
    },
    { capture: true, passive: true },
  );
  document.addEventListener(
    "pointerdown",
    () => {
      pressed = true;
      hide();
    },
    { capture: true, passive: true },
  );
  document.addEventListener("keydown", () => hide(), { capture: true });
  document.addEventListener("scroll", () => hide(), { capture: true, passive: true });
  addEventListener("blur", () => onto(null));
}
