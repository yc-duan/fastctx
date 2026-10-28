// A switch's chosen side moves to the one pressed: its wash slides there
// from where it was, instead of one going out and the other coming on. The
// switches are drawn by the pages that hold them, often anew on a press, so
// this follows every one from outside: before the press is handled it notes
// where the chosen side stood, and once the page has drawn the switch again
// it slides a wash of the same colour from there to the side now chosen,
// then leaves the chosen side its own. Where motion is off, nothing slides.

import { reducedMotion } from "../motion";

/** The switches whose chosen side slides: two or a few choices side by
 *  side, the chosen one washed. */
const TRACKS =
  ".segmented, .model-speeds, .picker-kinds, .ml-views, .map-roles, .mp-ranges, .logins-tabs, .data-tabs, .mp-tabs, .ledger-switch";

/** How a switch marks its chosen side. */
const CHOSEN = ":scope > .chosen, :scope > .current, :scope > [aria-selected='true'], :scope > [aria-checked='true'], :scope > [aria-pressed='true']";

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Follows every switch on the page, for as long as the page lives. */
export function followSwitches(): void {
  document.addEventListener(
    "click",
    (event) => {
      if (reducedMotion()) return;
      const target = event.target as Element | null;
      const track = target?.closest<HTMLElement>(TRACKS);
      if (!track) return;
      const before = chosenIn(track);
      // The side already chosen pressed again moves nothing.
      if (!before || before.contains(target)) return;
      const look = getComputedStyle(before);
      if (!painted(look.backgroundColor)) return;
      const from = boxIn(before, track);
      const where = track.getBoundingClientRect();
      const centre = { x: where.left + where.width / 2, y: where.top + where.height / 2 };
      const wash = { colour: look.backgroundColor, radius: look.borderRadius };
      requestAnimationFrame(() => {
        // The page may have drawn the switch anew where it stood.
        const now = document.elementsFromPoint(centre.x, centre.y).find((element): element is HTMLElement => element instanceof HTMLElement && element.matches(TRACKS));
        const after = now ? chosenIn(now) : null;
        // Not chosen yet, or chosen where it was: nothing to slide.
        if (!now || !after || after === before) return;
        slide(now, from, boxIn(after, now), wash);
      });
    },
    true,
  );
}

function chosenIn(track: HTMLElement): HTMLElement | null {
  return track.querySelector<HTMLElement>(CHOSEN);
}

function painted(colour: string): boolean {
  return colour !== "transparent" && !/rgba\([^)]*,\s*0\)$/.test(colour);
}

/** Where `element` is inside `track`'s padding box. */
function boxIn(element: HTMLElement, track: HTMLElement): Box {
  const outer = track.getBoundingClientRect();
  const inner = element.getBoundingClientRect();
  return { x: inner.left - outer.left - track.clientLeft, y: inner.top - outer.top - track.clientTop, w: inner.width, h: inner.height };
}

/** A wash from `from` to `to` in `track`, the chosen side's own wash held
 *  back meanwhile. */
function slide(track: HTMLElement, from: Box, to: Box, wash: { colour: string; radius: string }): void {
  if (Math.abs(from.x - to.x) < 1 && Math.abs(from.y - to.y) < 1) return;
  track.querySelector(":scope > .seg-thumb")?.remove();
  if (getComputedStyle(track).position === "static") track.style.position = "relative";
  const thumb = document.createElement("span");
  thumb.className = "seg-thumb";
  thumb.setAttribute("aria-hidden", "true");
  thumb.style.background = wash.colour;
  thumb.style.borderRadius = wash.radius;
  place(thumb, from);
  track.prepend(thumb);
  track.classList.add("seg-sliding");
  // Laid out where it was, then sent to where it goes.
  void thumb.offsetWidth;
  place(thumb, to);
  let done = false;
  const end = () => {
    if (done) return;
    done = true;
    thumb.remove();
    if (!track.querySelector(":scope > .seg-thumb")) track.classList.remove("seg-sliding");
  };
  thumb.addEventListener("transitionend", (event) => {
    if (event.propertyName === "--thumb-x" || event.propertyName === "--thumb-y") end();
  });
  // However the transition ends, or does not start, the wash goes back.
  const token = getComputedStyle(thumb).getPropertyValue("--fold").trim();
  const duration = parseFloat(token) * (token.endsWith("ms") ? 1 : 1000) || 0;
  window.setTimeout(end, duration + 80);
}

function place(thumb: HTMLElement, box: Box): void {
  thumb.style.setProperty("--thumb-x", `${box.x}px`);
  thumb.style.setProperty("--thumb-y", `${box.y}px`);
  thumb.style.setProperty("--thumb-w", `${box.w}px`);
  thumb.style.setProperty("--thumb-h", `${box.h}px`);
}
