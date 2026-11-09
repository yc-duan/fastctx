// History's tab of the conversations' settings (`world-history.md`,
// `world-gui.md` 接入与工作台). Off, it is the film, what history gives and
// the way to turn it on; on, a still of the film beside what turning it on
// opened, and, last and light, the way to turn it off. It has no settings
// of its own. The switch is the account's, or, for a computer in no network,
// the computer's.

import type { GlyphName } from "../conversation/glyphs";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { historyFilm } from "../film/history";
import type { Built, Shape } from "../film/reel";
import { readHistorySwitch, writeHistorySwitch } from "../world/history-switch";
import type { FeaturesWords } from "./features-words";

/** What the features page lends a feature drawn on it. */
export interface FeatureFrame {
  body: HTMLElement;
  words: FeaturesWords;
  film(holder: Element | null, make: (shape: Shape) => Built, label: string, still?: boolean): void;
  dropFilms(): void;
  celebrate(name: string, gained: string, make: (shape: Shape) => Built): void;
  onTop(gained: string, solo?: boolean): string;
  offWay(name: string): string;
  askOff(name: string, what: string, off: () => Promise<void>, done: () => void): void;
  gives(list: [GlyphName, string][]): string;
}

/** Where the switch is kept: an account at a hub; `null` for this
 *  computer in no network; `"none"` where there is no account to keep it. */
export type SwitchPlace = { hub: string; account: string } | null | "none";

export class HistoryFeature {
  /** The switch as read; `null` until it is. */
  private on: boolean | null = null;
  private trouble: string | null = null;
  private busy = false;
  private readonly make: (shape: Shape) => Built;

  constructor(
    private readonly frame: FeatureFrame,
    private readonly place: SwitchPlace,
  ) {
    const words = frame.words.history;
    this.make = (shape) => historyFilm(words.film, shape);
    frame.body.addEventListener("click", (event) => this.click(event));
    this.draw();
    void this.read();
  }

  /** The film over the page, as "watch again" plays it. */
  get film(): (shape: Shape) => Built {
    return this.make;
  }

  private async read(): Promise<void> {
    if (this.place === "none") return;
    this.trouble = null;
    try {
      this.on = await readHistorySwitch(this.place?.hub ?? null, this.place?.account ?? null);
    } catch {
      this.trouble = this.frame.words.history.unread;
    }
    this.draw();
  }

  private async write(on: boolean): Promise<void> {
    if (this.place === "none") return;
    await writeHistorySwitch(this.place?.hub ?? null, this.place?.account ?? null, on);
    this.on = on;
  }

  private click(event: Event): void {
    const target = event.target as HTMLElement;
    const { words } = this.frame;
    if (target.closest("[data-act='retry']")) void this.read();
    else if (target.closest("[data-act='turn']") && !this.busy) {
      this.busy = true;
      this.draw();
      this.write(true).then(
        () => {
          this.busy = false;
          this.draw();
          this.frame.celebrate(words.history.name, words.history.after, this.make);
        },
        (error: unknown) => {
          this.busy = false;
          this.trouble = words.failed(String(error));
          this.draw();
        },
      );
    } else if (target.closest("[data-act='turn-off']")) {
      this.frame.askOff(words.history.name, words.history.offWhat, () => this.write(false), () => this.draw());
    }
  }

  private draw(): void {
    const { body, words } = this.frame;
    const said = words.history;
    const on = this.on === true;
    // The film keeps playing while the page around it is drawn again.
    const playing = !on ? body.querySelector(".feature-intro .film") : null;
    if (!playing) this.frame.dropFilms();
    body.classList.toggle("features-film", !on);
    body.classList.toggle("features-on", on);
    if (on) {
      body.innerHTML = this.frame.onTop(said.after, true) + this.frame.offWay(said.name);
      this.frame.film(body.querySelector(".feature-still"), this.make, said.name, true);
      return;
    }
    const trouble = this.trouble
      ? `<p class="feature-note feature-trouble">${escape(this.trouble)}${this.on === null ? `<button class="text-button quiet-button" type="button" data-act="retry">${glyph("regenerate", 14)}<span>${escape(words.retry)}</span></button>` : ""}</p>`
      : "";
    const turn =
      this.place === "none"
        ? `<p class="feature-note">${escape(words.noAccount)}</p>`
        : this.on === null
          ? trouble || `<p class="feature-note">${escape(words.reading)}</p>`
          : `<button class="sheet-go feature-turn" type="button" data-act="turn"${this.busy ? " disabled" : ""}>${escape(words.turnOn)}</button>${trouble}`;
    body.innerHTML = `<div class="feature-intro with-gives"><div class="feature-film"></div>${this.frame.gives(said.gives)}${turn}</div>`;
    if (playing) body.querySelector(".feature-film")?.replaceChildren(playing);
    else this.frame.film(body.querySelector(".feature-film"), this.make, said.name);
  }
}
