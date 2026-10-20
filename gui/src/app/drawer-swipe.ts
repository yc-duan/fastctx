// On a phone the sidebar's drawer moves with the finger (`world-gui.md`,
// 界面的结构): pushed left it goes back as far as it is pushed, and from the
// screen's left edge it is drawn out the same way; let go past a third of
// its width, or flicked, it goes the rest of the way, else back where it
// was. Where the phone's own back gesture holds the edge, that edge is the
// phone's.

/** How far a finger goes before the movement is the drawer's. */
const SLOP = 10;
/** How near the screen's left edge a finger starts to draw the drawer out. */
const EDGE = 20;
/** A flick this fast, in pixels a millisecond, finishes it however short. */
const FLICK = 0.5;

/** What a swipe never starts on: fields, and what moves sideways itself. */
const OWN = "input, textarea, select, [contenteditable], [role='slider'], pre, .no-pull";

export interface Drawer {
  frame: HTMLElement;
  scrim: HTMLElement;
  narrow(): boolean;
  isOpen(): boolean;
  open(): void;
  close(): void;
}

export function followDrawer(drawer: Drawer): void {
  let swipe: { opening: boolean; x: number; y: number; at: number; engaged: boolean; share: number; samples: [number, number][] } | null = null;

  document.addEventListener(
    "touchstart",
    (event) => {
      swipe = null;
      if (!drawer.narrow() || event.touches.length !== 1) return;
      const touch = event.touches[0]!;
      const target = event.target as HTMLElement;
      if (target.closest(OWN)) return;
      const open = drawer.isOpen();
      const within = drawer.frame.contains(target) || drawer.scrim.contains(target);
      if (open ? !within : touch.clientX > EDGE) return;
      swipe = { opening: !open, x: touch.clientX, y: touch.clientY, at: event.timeStamp, engaged: false, share: open ? 1 : 0, samples: [[event.timeStamp, touch.clientX]] };
    },
    { passive: true },
  );

  document.addEventListener(
    "touchmove",
    (event) => {
      if (!swipe) return;
      const touch = event.touches[0]!;
      const dx = touch.clientX - swipe.x;
      const dy = touch.clientY - swipe.y;
      if (!swipe.engaged) {
        if (Math.abs(dx) < SLOP && Math.abs(dy) < SLOP) return;
        // Up and down is the page's or the sidebar's scroll.
        if (Math.abs(dy) > Math.abs(dx) || (swipe.opening ? dx < 0 : dx > 0)) {
          swipe = null;
          return;
        }
        swipe.engaged = true;
        swipe.x = touch.clientX;
        drawer.frame.style.transition = "none";
        drawer.scrim.style.transition = "none";
      }
      event.preventDefault();
      const width = drawer.frame.offsetWidth || 1;
      const moved = touch.clientX - swipe.x;
      swipe.share = Math.min(1, Math.max(0, swipe.opening ? moved / width : 1 + moved / width));
      swipe.samples.push([event.timeStamp, touch.clientX]);
      if (swipe.samples.length > 6) swipe.samples.shift();
      drawer.frame.style.setProperty("--slide", `${((swipe.share - 1) * 102).toFixed(2)}%`);
      drawer.scrim.style.setProperty("--shown", swipe.share.toFixed(3));
      drawer.scrim.style.pointerEvents = "auto";
    },
    { passive: false },
  );

  const end = (cancelled: boolean) => {
    const done = swipe;
    swipe = null;
    if (!done?.engaged) return;
    const [firstAt, firstX] = done.samples[0]!;
    const [lastAt, lastX] = done.samples[done.samples.length - 1]!;
    const speed = lastAt > firstAt ? (lastX - firstX) / (lastAt - firstAt) : 0;
    const open = cancelled ? !done.opening : done.opening ? done.share > 1 / 3 || speed > FLICK : !(done.share < 2 / 3 || speed < -FLICK);
    // From where the finger left it, the rest of the way by the drawer's
    // own movement.
    for (const element of [drawer.frame, drawer.scrim]) element.style.removeProperty("transition");
    drawer.frame.style.removeProperty("--slide");
    drawer.scrim.style.removeProperty("--shown");
    drawer.scrim.style.removeProperty("pointer-events");
    if (open && !drawer.isOpen()) drawer.open();
    else if (!open && drawer.isOpen()) drawer.close();
  };
  document.addEventListener("touchend", () => end(false), { passive: true });
  document.addEventListener("touchcancel", () => end(true), { passive: true });
}
