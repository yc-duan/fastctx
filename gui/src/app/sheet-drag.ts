// On a phone a sheet risen from below goes back down the way it came: a
// finger that pulls it down from the top of what it holds carries it along,
// and let go past a quarter of its height, or flicked, it closes, with
// whatever opened over it (`back.ts`); short of that it settles back up
// (`world-gui.md`, 界面的结构). What scrolls inside it scrolls first: the
// pull begins only where its content is at its top.

import { closeLayerOf } from "./back";

/** The sheets a phone raises from below: the panel that moves, each. */
const SHEETS = [
  ".sheet[open] > .sheet-body",
  ".talk-menu.as-sheet",
  ".map-menu.as-sheet",
  ".map-card.as-sheet",
  ".map-log.as-sheet",
  ".file-menu.as-sheet",
].join(", ");

/** What a pull never starts on: fields, and what drags sideways itself. */
const OWN = "input, textarea, select, [contenteditable], [role='slider'], .no-pull";

/** How far a finger goes before the pull is the sheet's, not a tap's. */
const SLOP = 8;
/** A flick this fast, in pixels a millisecond, closes it however short. */
const FLICK = 0.6;

const narrow = matchMedia("(max-width: 760px)");

export function installSheetDrag(): void {
  let pull: { sheet: HTMLElement; scroller: HTMLElement | null; x: number; y: number; dy: number; engaged: boolean; samples: [number, number][] } | null = null;

  document.addEventListener(
    "touchstart",
    (event) => {
      pull = null;
      if (event.touches.length !== 1) return;
      const target = event.target as HTMLElement;
      if (target.closest(OWN)) return;
      const sheet = target.closest<HTMLElement>(SHEETS);
      // What shows as a sheet is pulled as one, whatever width made it one;
      // a dialog is a sheet on a phone.
      if (!sheet || (!sheet.classList.contains("as-sheet") && !narrow.matches)) return;
      const touch = event.touches[0]!;
      pull = { sheet, scroller: scrollerOf(target, sheet), x: touch.clientX, y: touch.clientY, dy: 0, engaged: false, samples: [[event.timeStamp, touch.clientY]] };
    },
    { passive: true },
  );

  document.addEventListener(
    "touchmove",
    (event) => {
      if (!pull) return;
      const touch = event.touches[0]!;
      const dx = touch.clientX - pull.x;
      const dy = touch.clientY - pull.y;
      if (!pull.engaged) {
        if (Math.abs(dx) < SLOP && Math.abs(dy) < SLOP) return;
        // Sideways, upward, or with what it holds scrolled down: not a pull.
        if (Math.abs(dx) > Math.abs(dy) || dy < 0 || (pull.scroller && pull.scroller.scrollTop > 0)) {
          pull = null;
          return;
        }
        pull.engaged = true;
        pull.y = touch.clientY;
        hold(pull.sheet);
      }
      event.preventDefault();
      pull.dy = Math.max(0, touch.clientY - pull.y);
      pull.samples.push([event.timeStamp, touch.clientY]);
      if (pull.samples.length > 6) pull.samples.shift();
      pull.sheet.style.setProperty("--drag", `${pull.dy}px`);
    },
    { passive: false },
  );

  const end = (cancelled: boolean) => {
    const done = pull;
    pull = null;
    if (!done?.engaged) return;
    const [[firstAt, firstY] = [0, 0]] = done.samples;
    const [lastAt, lastY] = done.samples[done.samples.length - 1]!;
    const speed = lastAt > firstAt ? (lastY - firstY) / (lastAt - firstAt) : 0;
    const far = done.dy > Math.min(160, done.sheet.offsetHeight * 0.25);
    if (!cancelled && (far || speed > FLICK) && closeLayerOf(done.sheet)) {
      // It goes on down from where the finger left it, as it leaves.
      releaseWhenGone(done.sheet);
      return;
    }
    settle(done.sheet);
  };
  document.addEventListener("touchend", () => end(false), { passive: true });
  document.addEventListener("touchcancel", () => end(true), { passive: true });
}

/** The nearest scrolling box between `target` and its sheet. */
function scrollerOf(target: HTMLElement, sheet: HTMLElement): HTMLElement | null {
  for (let at: HTMLElement | null = target; at; at = at.parentElement) {
    if (at.scrollHeight > at.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(at).overflowY)) return at;
    if (at === sheet) break;
  }
  return null;
}

/** The sheet follows the finger: its own place, and the pull added. */
function hold(sheet: HTMLElement): void {
  sheet.classList.add("pulled");
  sheet.style.transition = "none";
  sheet.style.setProperty("--drag", "0px");
  sheet.style.translate = "var(--slide) calc(var(--lift) + var(--drag))";
}

/** Short of closing, it rises back to its place. */
function settle(sheet: HTMLElement): void {
  sheet.style.transition = "--drag var(--settle) var(--ease-out)";
  sheet.style.setProperty("--drag", "0px");
  window.setTimeout(() => release(sheet), 420);
}

/** Once it is gone from the screen, it is as it was before the pull, to
 *  open in its place next time. */
function releaseWhenGone(sheet: HTMLElement): void {
  const until = performance.now() + 1500;
  const dialog = sheet.closest("dialog");
  const look = () => {
    const gone = !sheet.isConnected || sheet.getClientRects().length === 0 || (dialog !== null && !dialog.open);
    if (gone || performance.now() > until) release(sheet);
    else requestAnimationFrame(look);
  };
  requestAnimationFrame(look);
}

function release(sheet: HTMLElement): void {
  sheet.classList.remove("pulled");
  sheet.style.removeProperty("transition");
  sheet.style.removeProperty("translate");
  sheet.style.removeProperty("--drag");
}
