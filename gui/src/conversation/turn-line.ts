// The turn's line (`world-gui.md`, 对话): at the foot of a conversation
// while its model works, an ellipsis being dotted by hand, 「……」, the way
// a sentence that is not finished trails off on paper, and how long the
// turn has run. The pen taps the six dots onto a faint pencil
// underdrawing, the two marks a breath apart, each dot blooming a little as
// the ink soaks into the paper; they stand a moment, dry away together, and
// are written again, so the line never stands still while the model works.
// While the first words are on their way the dots are pencil, not ink;
// waiting on the user, they stand still in the accent.
//
// It is there for the whole of a turn, whatever the turn is doing, and it
// is a boundary: above it what the session has taken in, below it words
// sent that it has not read yet. It comes in softly and goes at once, when
// the turn ends or is stopped. The dots go on where the system asks for
// less motion: they are the one sign that the model is at work.

import { curve } from "../design/curve";
import { blot, seeded } from "../pen/shapes";
import { bezier } from "./flight";

/** What the line says. */
export type LineSays =
  /** The first words are on their way and nothing has come back yet. */
  | { kind: "starting" }
  /** The model is working, since `since`. */
  | { kind: "working"; since: number | null }
  /** The turn waits on the user; `said` where nothing above says so. */
  | { kind: "waiting"; said: boolean };

export interface LineWords {
  starting: string;
  working: string;
  waitingForYou: string;
  took(ms: number): string;
}

/** Where the six dots of 「……」 sit: two marks of three, a breath apart. */
const PLACES = [3, 9, 15, 23.5, 29.5, 35.5];
const DOTS = PLACES.map((x, index) => blot([x, 5.5], 1.75, seeded(`ellipsis:${index}`)));
/** The ink a dot spreads into the paper as it lands. */
const BLOOMS = PLACES.map((x, index) => blot([x, 5.5], 3.4, seeded(`ellipsis-bloom:${index}`)));
const ELLIPSIS = `<svg viewBox="0 0 39 11" aria-hidden="true"><path class="ellipsis-trace" d="${DOTS.join("")}"/>${DOTS.map((dot, index) => `<path class="ellipsis-bloom" d="${BLOOMS[index]}"/><path class="ellipsis-ink" d="${dot}"/>`).join("")}</svg>`;

/** One writing of the ellipsis, in milliseconds: when each dot lands, when
 *  the ink begins to dry, when it is gone, and the whole, after which the
 *  pen begins again. */
const LANDS = [0, 300, 600, 1080, 1380, 1680];
const DRIES = 3500;
const DRY = 700;
const WHOLE = 4500;

/** A stroke's value at a moment of one writing, in milliseconds. */
interface Frame {
  at: number;
  alpha: number;
  scale: number;
}

/** The ink of dot `index`: it lands and swells, settles, stands, and dries
 *  away; a little later from left to right, as ink laid later dries later. */
function inkFrames(index: number): Frame[] {
  const land = LANDS[index]!;
  const dries = DRIES + index * 40;
  return [
    { at: 0, alpha: 0, scale: 0.4 },
    { at: land, alpha: 0, scale: 0.4 },
    { at: land + 110, alpha: 1, scale: 1.18 },
    { at: land + 260, alpha: 1, scale: 1 },
    { at: dries, alpha: 1, scale: 1 },
    { at: dries + DRY, alpha: 0, scale: 1 },
    { at: WHOLE, alpha: 0, scale: 1 },
  ];
}

/** The ink dot `index` spreads into the paper as it lands. */
function bloomFrames(index: number): Frame[] {
  const land = LANDS[index]!;
  return [
    { at: 0, alpha: 0, scale: 0.5 },
    { at: land + 60, alpha: 0, scale: 0.5 },
    { at: land + 160, alpha: 0.32, scale: 0.8 },
    { at: land + 900, alpha: 0, scale: 1.25 },
    { at: WHOLE, alpha: 0, scale: 1.25 },
  ];
}

/** Where `frames` stand `ms` into a writing, each easing out into the next:
 *  an easing over the whole writing would bend its clock instead, and the
 *  dots would land early. */
function sample(frames: Frame[], ms: number, ease: (t: number) => number): { alpha: number; scale: number } {
  let from = frames[0]!;
  for (const to of frames) {
    if (to.at > ms) {
      const span = to.at - from.at;
      const progress = span > 0 ? ease((ms - from.at) / span) : 1;
      return { alpha: from.alpha + (to.alpha - from.alpha) * progress, scale: from.scale + (to.scale - from.scale) * progress };
    }
    from = to;
  }
  return { alpha: from.alpha, scale: from.scale };
}

/** How often the pen moves the dots: they are a slow writing, and each
 *  move restyles them. Moved by the animations' own clock instead, a page
 *  whose model works restyled them every frame of the screen, 60 to 144
 *  times a second, nearly half a processor's core for the dots alone. */
const PEN_STEP = 1000 / 30;

/** Every pen at work, moved together by one clock, held weakly: a line
 *  dropped with its page goes with it. The clock runs while one is held,
 *  and stops where the page is not shown, as its timers and frames do. */
const pens = new Set<WeakRef<TurnLine>>();
let moving = false;

function move(): void {
  requestAnimationFrame((now) => {
    for (const pen of pens) {
      const line = pen.deref();
      if (line) line.draw(now);
      else pens.delete(pen);
    }
    if (pens.size > 0) window.setTimeout(move, PEN_STEP);
    else moving = false;
  });
}

export class TurnLine {
  readonly element: HTMLElement;
  private readonly step: HTMLElement;
  private readonly time: HTMLElement;
  private since: number | null = null;
  private ticking = 0;
  /** The pen's strokes: each dot's ink and bloom, what each was last set
   *  to, and since when the pen writes. */
  private strokes: { element: SVGPathElement; frames: Frame[]; alpha: number; scale: number }[] = [];
  private began = 0;
  /** Held still while the turn waits on the user, the ellipsis written whole. */
  private still = false;
  private ease: (t: number) => number = (t) => t;

  constructor(private readonly words: LineWords) {
    this.element = document.createElement("div");
    this.element.className = "turn-status";
    this.element.setAttribute("role", "status");
    this.element.innerHTML = `<span class="turn-status-mark">${ELLIPSIS}</span><span class="turn-status-step"></span><span class="turn-status-time" aria-hidden="true"></span>`;
    this.step = this.element.querySelector<HTMLElement>(".turn-status-step")!;
    this.time = this.element.querySelector<HTMLElement>(".turn-status-time")!;
    this.write();
  }

  /** Sets the pen to writing the ellipsis, over and over: each dot lands
   *  at its moment and blooms, all six dry together, and the pen comes
   *  back. The dots go on where the system asks for less motion: they are
   *  the one sign that the model works. The pen moves the dots' own
   *  properties (`design/breath.css`), not their opacity and transform, so
   *  no layer is made for them and the words beside them stay sharp. */
  private write(): void {
    const inks = [...this.element.querySelectorAll<SVGPathElement>(".ellipsis-ink")];
    const blooms = [...this.element.querySelectorAll<SVGPathElement>(".ellipsis-bloom")];
    this.strokes = inks.flatMap((ink, index) => [
      { element: ink, frames: inkFrames(index), alpha: Number.NaN, scale: Number.NaN },
      { element: blooms[index]!, frames: bloomFrames(index), alpha: Number.NaN, scale: Number.NaN },
    ]);
    try {
      this.ease = bezier(curve("--ease-out"));
    } catch {
      // Without its token the pen moves at an even pace.
    }
    this.began = performance.now();
    pens.add(new WeakRef(this));
    if (!moving) {
      moving = true;
      move();
    }
  }

  /** Moves the dots to where the writing stands at `now`, setting only
   *  what changed. */
  draw(now: number): void {
    if (!this.element.isConnected) return;
    const ms = this.still ? DRIES - 1 : (now - this.began) % WHOLE;
    for (const stroke of this.strokes) {
      const { alpha, scale } = sample(stroke.frames, ms, this.ease);
      const a = Math.round(alpha * 1000) / 1000;
      const s = Math.round(scale * 1000) / 1000;
      if (a !== stroke.alpha) {
        stroke.alpha = a;
        stroke.element.style.setProperty("--ink-alpha", String(a));
      }
      if (s !== stroke.scale) {
        stroke.scale = s;
        stroke.element.style.setProperty("--ink-scale", String(s));
      }
    }
  }

  /** Holds the pen with the ellipsis written whole, or lets it write on. */
  private hold(still: boolean): void {
    if (still === this.still) return;
    this.still = still;
    // Written on, the pen picks up at the moment it was held at.
    if (!still) this.began = performance.now() - (DRIES - 1);
  }

  get shown(): boolean {
    return this.element.isConnected;
  }

  /** Says `says`. */
  say(says: LineSays): void {
    const { words } = this;
    const step = says.kind === "waiting" && says.said ? words.waitingForYou : "";
    const heard = says.kind === "starting" ? words.starting : says.kind === "waiting" ? words.waitingForYou : words.working;
    this.element.classList.toggle("waiting", says.kind === "waiting");
    this.element.classList.toggle("starting", says.kind === "starting");
    if (this.step.textContent !== step) this.step.textContent = step;
    // The dots say nothing to a screen reader; the state does.
    const mark = this.element.querySelector<HTMLElement>(".turn-status-mark")!;
    if (mark.getAttribute("aria-label") !== heard) mark.setAttribute("aria-label", heard);
    this.since = says.kind === "working" ? says.since : null;
    this.hold(says.kind === "waiting");
    this.tick();
  }

  /** Puts the line in the page before `before` (at the end of `parent`
   *  where null). When it was not there it comes in softly, after `after`
   *  milliseconds, where `drawn`; otherwise it is simply there, as in a
   *  conversation opened while its model works. */
  place(parent: HTMLElement, before: Element | null, drawn: boolean, after = 0): void {
    const fresh = !this.element.isConnected;
    if (this.element.parentElement !== parent || this.element.nextElementSibling !== before) parent.insertBefore(this.element, before);
    if (!fresh) return;
    this.tick();
    if (!drawn) return;
    const duration = time(getComputedStyle(this.element), "--line-in");
    if (duration > 0) this.element.animate([{ "--rise": 0 }, { "--rise": 1 }], { duration, delay: after, easing: curve("--ease-out"), fill: "backwards" });
  }

  /** Takes the line away at once. */
  remove(): void {
    if (!this.element.isConnected) return;
    for (const animation of this.element.getAnimations()) animation.cancel();
    this.element.remove();
    clearTimeout(this.ticking);
  }

  /** Brings the clock up to date, once a second while it runs. */
  private tick(): void {
    clearTimeout(this.ticking);
    const said = this.since !== null ? this.words.took(Date.now() - this.since) : "";
    if (this.time.textContent !== said) this.time.textContent = said;
    if (this.since !== null && this.element.isConnected) this.ticking = window.setTimeout(() => this.tick(), 1000);
  }
}

/** A duration from a design token, in milliseconds. */
export function time(style: CSSStyleDeclaration, token: string): number {
  const value = style.getPropertyValue(token).trim();
  const number = parseFloat(value);
  if (!Number.isFinite(number)) throw new Error(`${token} must be a CSS time`);
  return value.endsWith("ms") ? number : number * 1000;
}
