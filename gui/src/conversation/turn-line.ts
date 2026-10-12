// The turn's line (`world-gui.md`, 对话): at the foot of a conversation
// while its model works, a rule in pencil across the column, a drop of ink
// and a few words in italic with how long the turn has run. It is there for
// the whole of a turn, whatever the turn is doing, and it is a boundary:
// above it what the session has taken in, below it words sent that it has
// not read yet.
//
// It comes as if drawn: the rule is pulled across from the left, the drop
// lands, the words are written in after it. It goes at once, when the turn
// ends or is stopped, with nothing to watch. Its motion is the Web
// Animations API's, which an element keeps when it is moved in the page,
// so the line is never drawn again for having moved below words sent.

/** What the line says. */
export type LineSays =
  /** The first words are on their way and nothing has come back yet. */
  | { kind: "starting" }
  /** The model is working: on `step`, or, where what is just above already
   *  says the step, only that it works; since `since`. */
  | { kind: "working"; step: string | null; since: number | null }
  /** The turn waits on the user; `said` where nothing above says so. */
  | { kind: "waiting"; said: boolean };

export interface LineWords {
  starting: string;
  working: string;
  waitingForYou: string;
  took(ms: number): string;
}

export class TurnLine {
  readonly element: HTMLElement;
  private readonly rule: HTMLElement;
  private readonly mark: HTMLElement;
  private readonly step: HTMLElement;
  private readonly time: HTMLElement;
  private since: number | null = null;
  private ticking = 0;

  constructor(private readonly words: LineWords) {
    this.element = document.createElement("div");
    this.element.className = "turn-status";
    this.element.setAttribute("role", "status");
    this.element.innerHTML = `<span class="turn-status-rule" aria-hidden="true"></span><span class="turn-status-mark" aria-hidden="true"><i></i></span><span class="turn-status-step"></span><span class="turn-status-time" aria-hidden="true"></span>`;
    this.rule = this.element.querySelector<HTMLElement>(".turn-status-rule")!;
    this.mark = this.element.querySelector<HTMLElement>(".turn-status-mark")!;
    this.step = this.element.querySelector<HTMLElement>(".turn-status-step")!;
    this.time = this.element.querySelector<HTMLElement>(".turn-status-time")!;
  }

  get shown(): boolean {
    return this.element.isConnected;
  }

  /** Says `says`; the words fade in where others stood. */
  say(says: LineSays): void {
    const { words } = this;
    const step =
      says.kind === "starting" ? words.starting : says.kind === "waiting" ? (says.said ? words.waitingForYou : "") : (says.step ?? words.working);
    this.element.classList.toggle("waiting", says.kind === "waiting");
    this.element.classList.toggle("starting", says.kind === "starting");
    if (this.step.textContent !== step) {
      const had = !!this.step.textContent;
      this.step.textContent = step;
      if (had && this.shown && !this.drawing()) fadeIn(this.step);
    }
    this.since = says.kind === "working" ? says.since : null;
    this.tick();
  }

  /** Puts the line in the page before `before` (at the end of `parent`
   *  where null). When it was not there it is drawn in, after `after`
   *  milliseconds, where `drawn`; otherwise it is simply there, as in a
   *  conversation opened while its model works. */
  place(parent: HTMLElement, before: Element | null, drawn: boolean, after = 0): void {
    const fresh = !this.element.isConnected;
    if (this.element.parentElement !== parent || this.element.nextElementSibling !== before) parent.insertBefore(this.element, before);
    if (!fresh) return;
    this.tick();
    if (drawn) this.draw(after);
  }

  /** Takes the line away at once. */
  remove(): void {
    if (!this.element.isConnected) return;
    for (const animation of this.motion) animation.cancel();
    this.motion = [];
    this.element.remove();
    clearTimeout(this.ticking);
  }

  /** The drawing under way, if it is. */
  private motion: Animation[] = [];

  /** Brings the clock up to date, once a second while it runs. */
  private tick(): void {
    clearTimeout(this.ticking);
    const said = this.since !== null ? this.words.took(Date.now() - this.since) : "";
    if (this.time.textContent !== said) this.time.textContent = said;
    if (this.since !== null && this.element.isConnected) this.ticking = window.setTimeout(() => this.tick(), 1000);
  }

  private drawing(): boolean {
    return this.motion.some((animation) => animation.playState === "running");
  }

  /** The rule pulled across from the left, the drop landing, the words
   *  written in after it; nothing moves where the system asks for less. */
  private draw(wait: number): void {
    const style = getComputedStyle(this.element);
    const draw = time(style, "--draw");
    const write = time(style, "--write");
    const after = time(style, "--write-after");
    const easing = style.getPropertyValue("--draw-easing").trim() || "ease-out";
    this.motion = [];
    if (draw > 0) {
      this.motion.push(this.rule.animate([{ clipPath: "inset(0 100% 0 0)" }, { clipPath: "inset(0 0 0 0)" }], { duration: draw, delay: wait, easing, fill: "backwards" }));
    }
    if (write > 0) {
      this.motion.push(
        this.mark.animate(
          [
            { opacity: 0, transform: "scale(0.35)" },
            { opacity: 1, transform: "scale(1.08)", offset: 0.6 },
            { opacity: 1, transform: "scale(1)" },
          ],
          { duration: write * 0.7, delay: wait + after * 0.5, easing: "ease-out", fill: "backwards" },
        ),
      );
      // The words come from behind a soft edge moving to the right, as if
      // written; at rest they carry no mask at all.
      const veil = (position: string): Keyframe => ({ maskImage: REVEAL, maskSize: "220% 100%", maskRepeat: "no-repeat", maskPosition: position });
      for (const words of [this.step, this.time]) {
        this.motion.push(words.animate([veil("100% 0"), veil("0% 0")], { duration: write, delay: wait + after, easing, fill: "backwards" }));
      }
    }
  }
}

/** What the words are revealed through: opaque, then a soft edge, then
 *  nothing; at 220% of their width, wholly opaque at one end and wholly
 *  clear at the other. */
const REVEAL = "linear-gradient(90deg, #000 46%, transparent 54%)";

/** A duration from a design token, in milliseconds. */
export function time(style: CSSStyleDeclaration, token: string): number {
  const value = style.getPropertyValue(token).trim();
  const number = parseFloat(value);
  if (!Number.isFinite(number)) throw new Error(`${token} must be a CSS time`);
  return value.endsWith("ms") ? number : number * 1000;
}

/** New words where others stood: they come up out of the paper. */
function fadeIn(element: HTMLElement): void {
  const duration = time(getComputedStyle(element), "--hover");
  if (duration > 0) element.animate([{ opacity: 0 }, { opacity: 1 }], { duration: duration * 1.4, easing: "ease-out" });
}
