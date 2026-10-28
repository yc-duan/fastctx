// The conversation's measure in the input's row (`world-gui.md`, 对话): how
// much it holds, how much of the last request's prompt the provider's cache
// served, and, where the host says how long that cache stays, how long it
// stays from the last answer. A figure that changes is written again by a
// sweep from left to right: a caret runs ahead, the two marks behind it
// flicker, and each settles as the caret leaves it. Where there is room for
// no figure, as on a phone, a small dial stands for them: the cache's time
// draining, else the share of the context used. Pressed, it opens what the
// conversation has used.

import { reducedMotion } from "../motion";
import { glyph } from "./glyphs";
import { escape } from "./prose";
import { contextOf, hitOf, warmUntil, type ModelRequest } from "./requests";

export interface BadgeWords {
  title: string;
  warm: string;
  cold: string;
  left(ms: number): string;
  ttl(ms: number): string;
  contextTitle(used: number, window: number | null): string;
  hitTitle(hit: string): string;
  warmTitle(left: string, ttl: string): string;
  coldTitle: string;
}

/** A count of tokens, short: 820, 61.4k, 148k, 1.02M. */
export function tokensShort(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 100_000) return `${Math.round(n / 1000)}k`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(Math.round(n));
}

/** A share to a hundredth of a percent, as a cache's hits are told apart. */
export function percentFine(share: number): string {
  return `${(share * 100).toFixed(2)}%`;
}

/** A small ring with a wedge of `share` filled, clockwise from the top. */
export function ring(share: number): string {
  const r = 5;
  const angle = share * 2 * Math.PI;
  const wedge =
    share <= 0
      ? ""
      : share >= 0.999
        ? `<circle class="used" cx="9" cy="9" r="${r}"/>`
        : `<path class="used" d="M9 9V${9 - r}A${r} ${r} 0 ${share > 0.5 ? 1 : 0} 1 ${(9 + r * Math.sin(angle)).toFixed(2)} ${(9 - r * Math.cos(angle)).toFixed(2)}Z"/>`;
  return `<svg class="context-ring" width="18" height="18" viewBox="0 0 18 18" aria-hidden="true"><circle cx="9" cy="9" r="7.4"/>${wedge}</svg>`;
}

/** How long a sweep takes over each mark. */
const STEP_MS = 130;

type Figure = "context" | "hit" | "warm";

export class UsageBadge {
  private readonly figures = new Map<Figure, HTMLElement>();
  private readonly shown = new Map<Figure, string>();
  private readonly sweeps = new Map<HTMLElement, number>();
  private ticking = 0;
  /** What it was last drawn from, for the clock to move on alone. */
  private last: { context: { used: number; window: number | null } | null; requests: readonly ModelRequest[] } = { context: null, requests: [] };

  constructor(
    private readonly button: HTMLElement,
    private readonly words: BadgeWords,
  ) {
    button.innerHTML = (["context", "hit", "warm"] as Figure[])
      .map((figure) => `<span class="talk-gauge" data-figure="${figure}" hidden>${glyph(figure === "context" ? "context" : figure === "hit" ? "cache" : "clock", 15)}<span class="talk-gauge-value"></span></span>`)
      .join("") + `<span class="talk-gauge-dial" aria-hidden="true"></span><span class="talk-gauge-bare" hidden><span class="talk-gauge-word">${escape(words.title)}</span>${glyph("context", 17)}</span>`;
    for (const element of button.querySelectorAll<HTMLElement>("[data-figure]")) this.figures.set(element.dataset.figure as Figure, element);
  }

  /** Whether it has anything to show. */
  get shows(): boolean {
    return [...this.figures.values()].some((element) => !element.hidden);
  }

  /** Draws what the conversation holds now and what its requests cost. */
  draw(context: { used: number; window: number | null } | null, requests: readonly ModelRequest[]): void {
    this.last = { context, requests };
    const { words } = this;
    const latest = requests[requests.length - 1] ?? null;
    const used = context?.used ?? (latest ? contextOf(latest) : null);
    const window = context?.window ?? null;
    const hit = latest && contextOf(latest) ? percentFine(hitOf(latest)) : null;
    const warm = warmUntil(requests);
    const now = Date.now();
    const left = warm ? warm.until - now : null;
    this.set("context", used !== null && used > 0 ? tokensShort(used) : null, used !== null ? words.contextTitle(used, window) : "");
    this.set("hit", hit, hit ? words.hitTitle(hit) : "");
    this.set("warm", warm ? (left! > 0 ? words.left(left!) : words.cold) : null, warm ? (left! > 0 ? words.warmTitle(words.left(left!), words.ttl(warm.ttl)) : words.coldTitle) : "", left !== null && left <= 0);
    // Context nearly full, and a cache about to go, are said in the
    // caution colour.
    this.figures.get("context")!.classList.toggle("caution", !!(used && window && used / window >= 0.85));
    this.figures.get("warm")!.classList.toggle("caution", !!(warm && left! > 0 && left! < warm.ttl / 10));
    // The dial for a narrow row: the cache's time left of its whole, else
    // the share of the context used.
    const dial = this.button.querySelector<HTMLElement>(".talk-gauge-dial")!;
    const share = warm ? Math.max(0, left!) / warm.ttl : used && window ? Math.min(1, used / window) : null;
    dial.innerHTML = share === null ? "" : ring(share);
    dial.classList.toggle("caution", warm ? left! > 0 && left! < warm.ttl / 10 : !!(used && window && used / window >= 0.85));
    dial.classList.toggle("cold", !!warm && left! <= 0);
    // With nothing to measure, the button still opens the account's limits.
    this.button.querySelector<HTMLElement>(".talk-gauge-bare")!.hidden = this.shows;
    const label = [...this.figures.values()].filter((element) => !element.hidden).map((element) => element.title);
    this.button.setAttribute("aria-label", [words.title, ...label].join(" "));
    this.tick(left);
  }

  /** Stops the clock and any sweep, as the page goes. */
  dispose(): void {
    clearTimeout(this.ticking);
    for (const timer of this.sweeps.values()) clearTimeout(timer);
    this.sweeps.clear();
  }

  private set(figure: Figure, text: string | null, title: string, cold = false): void {
    const element = this.figures.get(figure)!;
    element.hidden = text === null;
    element.title = title;
    element.classList.toggle("cold", cold);
    if (text === null) return;
    const value = element.querySelector<HTMLElement>(".talk-gauge-value")!;
    const before = this.shown.get(figure);
    if (before === text) return;
    this.shown.set(figure, text);
    // The clock counting down moves quietly; a figure that changed for a
    // new request is written again.
    if (before === undefined || reducedMotion() || (figure === "warm" && !this.jumped(before, text))) {
      this.stopSweep(value);
      value.textContent = text;
      return;
    }
    this.sweep(value, text);
  }

  /** Whether the cache's time went up, as after a request, rather than
   *  down with the clock. */
  private jumped(before: string, after: string): boolean {
    const number = (text: string) => parseFloat(text.replace(/[^\d.]/g, "")) || 0;
    return after === this.words.cold || before === this.words.cold || number(after) > number(before);
  }

  /** Writes `text` again with the sweep. */
  private sweep(element: HTMLElement, text: string): void {
    this.stopSweep(element);
    const marks = [...text];
    let caret = 0;
    const step = () => {
      if (caret - 3 >= marks.length) {
        element.textContent = text;
        this.sweeps.delete(element);
        return;
      }
      element.innerHTML = marks
        .map((mark, index) => {
          const behind = caret - index;
          if (behind >= 3 || mark === " ") return escape(mark);
          if (behind === 0) return `<span class="talk-gauge-caret">▌</span>`;
          if (behind > 0) return `<span class="talk-gauge-noise">${escape(["·", "_", mark][Math.floor(Math.random() * 3)]!)}</span>`;
          // Not reached yet: a space as wide as a figure, so the badge keeps
          // its width while it is written.
          return " ";
        })
        .join("");
      caret += 1;
      this.sweeps.set(element, window.setTimeout(step, STEP_MS));
    };
    step();
  }

  private stopSweep(element: HTMLElement): void {
    const timer = this.sweeps.get(element);
    if (timer !== undefined) clearTimeout(timer);
    this.sweeps.delete(element);
  }

  /** Moves the cache's clock on: each minute, each second in its last one,
   *  and once more as it goes cold. */
  private tick(left: number | null): void {
    clearTimeout(this.ticking);
    if (left === null || left <= 0 || !this.button.isConnected) return;
    const next = left <= 60_000 ? left % 1000 || 1000 : left % 60_000 || 60_000;
    this.ticking = window.setTimeout(() => this.draw(this.last.context, this.last.requests), next + 20);
  }
}
