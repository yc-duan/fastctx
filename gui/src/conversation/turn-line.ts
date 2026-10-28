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

export class TurnLine {
  readonly element: HTMLElement;
  private readonly step: HTMLElement;
  private readonly time: HTMLElement;
  private since: number | null = null;
  private ticking = 0;
  /** The pen's strokes, held still while the turn waits on the user. */
  private strokes: Animation[] = [];

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
   *  the one sign that the model works. */
  private write(): void {
    const inks = [...this.element.querySelectorAll<SVGPathElement>(".ellipsis-ink")];
    const blooms = [...this.element.querySelectorAll<SVGPathElement>(".ellipsis-bloom")];
    const at = (ms: number) => Math.min(1, ms / WHOLE);
    inks.forEach((ink, index) => {
      const land = LANDS[index]!;
      // A little later from left to right, as ink laid later dries later.
      const dries = DRIES + index * 40;
      // The pen moves the dots' own properties (`design/breath.css`), not
      // their opacity and transform, so no layer is made for them and the
      // words beside them stay sharp.
      const inked = ink.animate(
        eased([
          { offset: 0, "--ink-alpha": 0, "--ink-scale": 0.4 },
          { offset: at(land), "--ink-alpha": 0, "--ink-scale": 0.4 },
          { offset: at(land + 110), "--ink-alpha": 1, "--ink-scale": 1.18 },
          { offset: at(land + 260), "--ink-alpha": 1, "--ink-scale": 1 },
          { offset: at(dries), "--ink-alpha": 1, "--ink-scale": 1 },
          { offset: at(dries + DRY), "--ink-alpha": 0, "--ink-scale": 1 },
          { offset: 1, "--ink-alpha": 0, "--ink-scale": 1 },
        ]),
        { duration: WHOLE, iterations: Infinity },
      );
      const bloomed = blooms[index]!.animate(
        eased([
          { offset: 0, "--ink-alpha": 0, "--ink-scale": 0.5 },
          { offset: at(land + 60), "--ink-alpha": 0, "--ink-scale": 0.5 },
          { offset: at(land + 160), "--ink-alpha": 0.32, "--ink-scale": 0.8 },
          { offset: at(land + 900), "--ink-alpha": 0, "--ink-scale": 1.25 },
          { offset: 1, "--ink-alpha": 0, "--ink-scale": 1.25 },
        ]),
        { duration: WHOLE, iterations: Infinity },
      );
      this.strokes.push(inked, bloomed);
    });
  }

  /** Holds the pen with the ellipsis written whole, or lets it write on. */
  private hold(still: boolean): void {
    for (const stroke of this.strokes) {
      if (still && stroke.playState !== "paused") {
        stroke.pause();
        stroke.currentTime = DRIES - 1;
      } else if (!still && stroke.playState === "paused") stroke.play();
    }
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

/** Keyframes that each ease out into the next: an easing on the whole
 *  animation would bend its clock instead, and the dots would land early. */
function eased(frames: Keyframe[]): Keyframe[] {
  return frames.map((frame) => ({ ...frame, easing: curve("--ease-out") }));
}

/** A duration from a design token, in milliseconds. */
export function time(style: CSSStyleDeclaration, token: string): number {
  const value = style.getPropertyValue(token).trim();
  const number = parseFloat(value);
  if (!Number.isFinite(number)) throw new Error(`${token} must be a CSS time`);
  return value.endsWith("ms") ? number : number * 1000;
}
