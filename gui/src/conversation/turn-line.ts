// The turn's line (`world-gui.md`, 对话): at the foot of a conversation
// while its model works, a pen writing loops in clay ink, one lively word
// picked for the turn, and how long the turn has run. It is there for the
// whole of a turn, whatever the turn is doing, and it is a boundary: above
// it what the session has taken in, below it words sent that it has not
// read yet.
//
// It comes in softly and goes at once, when the turn ends or is stopped.
// The pen keeps writing where the system asks for less motion: it is the
// one sign that the model is at work.

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
  waitingForYou: string;
  /** The words the line picks one of for each turn. */
  lively: readonly string[];
  took(ms: number): string;
}

/** The pen's loops: a cursive double loop traced in pencil, the clay ink
 *  going over it and taken up again from where it began, round and round. */
const LOOPS = "M1.2 9.6c2.2.2 4.6-2.1 4.6-5.1 0-2.2-2.4-2.1-2 .6.5 3.4 3.3 4.6 5.7 4.5 2.4-.1 4.5-2.4 4.5-5.1 0-2.2-2.4-2-2 .7.5 3.2 3.4 4.3 6.6 3.6";
const PEN = `<svg viewBox="0 0 20 12" aria-hidden="true"><path class="pen-trace" d="${LOOPS}"/><path class="pen-ink" pathLength="100" d="${LOOPS}"/></svg>`;

export class TurnLine {
  readonly element: HTMLElement;
  private readonly step: HTMLElement;
  private readonly time: HTMLElement;
  private since: number | null = null;
  private ticking = 0;
  /** The word picked for the turn under way. */
  private word = "";

  constructor(private readonly words: LineWords) {
    this.element = document.createElement("div");
    this.element.className = "turn-status";
    this.element.setAttribute("role", "status");
    this.element.innerHTML = `<span class="turn-status-mark" aria-hidden="true">${PEN}</span><span class="turn-status-step"></span><span class="turn-status-time" aria-hidden="true"></span>`;
    this.step = this.element.querySelector<HTMLElement>(".turn-status-step")!;
    this.time = this.element.querySelector<HTMLElement>(".turn-status-time")!;
  }

  get shown(): boolean {
    return this.element.isConnected;
  }

  /** Says `says`. */
  say(says: LineSays): void {
    const { words } = this;
    if (!this.word) this.word = words.lively[Math.floor(Math.random() * words.lively.length)] ?? "";
    const step = says.kind === "starting" ? `${words.starting}…` : says.kind === "waiting" ? (says.said ? words.waitingForYou : "") : `${this.word}…`;
    this.element.classList.toggle("waiting", says.kind === "waiting");
    this.element.classList.toggle("starting", says.kind === "starting");
    if (this.step.textContent !== step) this.step.textContent = step;
    this.since = says.kind === "working" ? says.since : null;
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
    if (duration > 0) this.element.animate([{ opacity: 0 }, { opacity: 1 }], { duration, delay: after, easing: "ease-out", fill: "backwards" });
  }

  /** Takes the line away at once; the next turn picks its own word. */
  remove(): void {
    if (!this.element.isConnected) return;
    for (const animation of this.element.getAnimations()) animation.cancel();
    this.element.remove();
    this.word = "";
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
