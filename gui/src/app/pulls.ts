// On a phone what stands beyond the screen's sides is pulled in by the
// finger (`world-gui.md`, 界面的结构): a swipe to the right anywhere on the
// page draws the sidebar's drawer out of the left side, a swipe to the left
// draws out what the page keeps at its right, a conversation's side; the
// opposite swipe on either sends it back. It moves with the finger: let go
// past a third of its width, or flicked, it goes the rest of the way, else
// back where it was. A pull starts anywhere on the page, not at the screen's
// edge, which is the phone's own back gesture's. What moves sideways itself
// keeps the finger: a field, a slider, what scrolls sideways and still has
// somewhere to go that way, what takes its touches whole.

/** How far a finger goes before the movement is a pull. */
const SLOP = 10;
/** How many times further sideways than up or down a pull goes: less than
 *  this and the movement is the page's scroll. */
const LEAN = 1.5;
/** A flick this fast, in pixels a millisecond, decides it however short. */
const FLICK = 0.5;
/** What has fewer pixels than this left to scroll has nowhere to go. */
const ROOM = 4;

/** What a pull never starts on: fields, what drags sideways itself, and
 *  what lies over the page with gestures of its own. */
const OWN = "input, textarea, select, [contenteditable], [role='slider'], dialog, [role='dialog'], [role='menu'], .as-sheet, .no-pull";

/** Something a sideways swipe pulls in and sends back. */
export interface Pulled {
  /** The side of the screen it comes from. */
  from: "left" | "right";
  /** Whether it can be pulled now: the page is laid out as a phone's and
   *  has it. */
  ready(): boolean;
  isOpen(): boolean;
  /** Whether a swipe that began on `target` is on it, to send it back. */
  holds(target: Element): boolean;
  /** How far it travels from shut to open, in pixels. */
  width(): number;
  /** The finger takes it: from here it stands where `at` puts it, and one
   *  taken shut is drawn with nothing of it showing yet. */
  take(opening: boolean): void;
  /** How much of it shows, from 0 to 1. */
  at(share: number): void;
  /** The finger let go: it goes the rest of the way, open or shut, by its
   *  own movement. */
  leave(open: boolean): void;
}

const all: Pulled[] = [];
let installed = false;

/** Lets swipes pull `one`; what is returned stops them. */
export function pulled(one: Pulled): () => void {
  all.push(one);
  if (!installed) {
    installed = true;
    install();
  }
  return () => {
    const at = all.indexOf(one);
    if (at >= 0) all.splice(at, 1);
  };
}

/** Whether something under the finger keeps a movement to the right, or to
 *  the left, for itself. */
function keeps(target: Element, right: boolean): boolean {
  for (let one: Element | null = target; one && one !== document.documentElement; one = one.parentElement) {
    const style = getComputedStyle(one);
    if (style.touchAction === "none") return true;
    if (style.overflowX !== "auto" && style.overflowX !== "scroll") continue;
    if ((right ? one.scrollLeft : one.scrollWidth - one.clientWidth - one.scrollLeft) >= ROOM) return true;
  }
  return false;
}

/** What a swipe that began on `target` and goes to the right, or to the
 *  left, pulls: what is open over the page, sent back the way it came, else
 *  what comes from the side the finger leaves. */
function chosen(target: Element, right: boolean): { one: Pulled; opening: boolean } | null {
  const ready = all.filter((one) => one.ready());
  const open = ready.filter((one) => one.isOpen());
  if (open.length) {
    const one = open.find((one) => one.holds(target));
    return one && (one.from === "left") !== right ? { one, opening: false } : null;
  }
  const one = ready.find((one) => (one.from === "left") === right);
  return one ? { one, opening: true } : null;
}

function install(): void {
  let swipe: { target: Element; x: number; y: number; one: Pulled | null; opening: boolean; share: number; samples: [number, number][] } | null = null;

  const settle = (cancelled: boolean) => {
    const done = swipe;
    swipe = null;
    if (!done?.one) return;
    const [firstAt, firstX] = done.samples[0]!;
    const [lastAt, lastX] = done.samples[done.samples.length - 1]!;
    // Towards open is positive, whichever side it comes from.
    const speed = lastAt > firstAt ? ((lastX - firstX) / (lastAt - firstAt)) * (done.one.from === "left" ? 1 : -1) : 0;
    const open = cancelled ? !done.opening : speed > FLICK ? true : speed < -FLICK ? false : done.share > (done.opening ? 1 / 3 : 2 / 3);
    done.one.leave(open);
  };

  document.addEventListener(
    "touchstart",
    (event) => {
      // A second finger ends what the first was pulling.
      if (swipe?.one) settle(true);
      swipe = null;
      if (event.touches.length !== 1 || !all.some((one) => one.ready())) return;
      const target = event.target;
      if (!(target instanceof Element) || target.closest(OWN)) return;
      const touch = event.touches[0]!;
      swipe = { target, x: touch.clientX, y: touch.clientY, one: null, opening: false, share: 0, samples: [[event.timeStamp, touch.clientX]] };
    },
    { passive: true },
  );

  document.addEventListener(
    "touchmove",
    (event) => {
      if (!swipe) return;
      const touch = event.touches[0]!;
      if (!swipe.one) {
        const dx = touch.clientX - swipe.x;
        const dy = touch.clientY - swipe.y;
        if (Math.abs(dx) < SLOP && Math.abs(dy) < SLOP) return;
        // Up and down is the page's scroll, and so is a movement the page
        // has already taken for one.
        const right = dx > 0;
        const found = event.cancelable && Math.abs(dx) > Math.abs(dy) * LEAN && !keeps(swipe.target, right) ? chosen(swipe.target, right) : null;
        if (!found) {
          swipe = null;
          return;
        }
        swipe.one = found.one;
        swipe.opening = found.opening;
        swipe.share = found.opening ? 0 : 1;
        swipe.x = touch.clientX;
        found.one.take(found.opening);
      }
      if (event.cancelable) event.preventDefault();
      const one = swipe.one;
      const moved = ((touch.clientX - swipe.x) * (one.from === "left" ? 1 : -1)) / (one.width() || 1);
      swipe.share = Math.min(1, Math.max(0, swipe.opening ? moved : 1 + moved));
      swipe.samples.push([event.timeStamp, touch.clientX]);
      if (swipe.samples.length > 6) swipe.samples.shift();
      one.at(swipe.share);
    },
    { passive: false },
  );

  document.addEventListener("touchend", () => settle(false), { passive: true });
  document.addEventListener("touchcancel", () => settle(true), { passive: true });
}
