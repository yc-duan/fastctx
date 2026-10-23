// The page that stands in for the interface until Fastctx has opened on this
// device: a quiet line while it opens, if that takes a moment; when it could
// not, what did not open in a sentence, the opening's own words when that
// sentence is pointed at, and the two things to do, repair and try again;
// while a repair goes, its layers one by one.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { Sketch } from "../pen/sketch";
import { RepairLayers } from "../app/repair-layers";
import type { RepairStep } from "../world/repair";
import type { StartupWords } from "./words";
import "../design/fonts.css";
import "../design/tokens.css";
import "../design/breath.css";
import "../design/base.css";
import "./startup.css";

export interface Failure {
  step: string;
  error: string;
}

export interface PageActs {
  retry: () => void;
  /** Absent where this device cannot repair itself, as on a phone. */
  repair: (() => void) | null;
}

export class StartupPage {
  readonly element: HTMLElement;
  /** The layers of the repair under way, kept so rows drawn stay put. */
  private layers: RepairLayers | null = null;

  constructor(
    private readonly words: StartupWords,
    private readonly acts: PageActs,
  ) {
    this.element = document.createElement("main");
    this.element.className = "su-page";
    this.element.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]")?.dataset.act;
      if (act === "retry") this.acts.retry();
      else if (act === "repair") this.acts.repair?.();
    });
  }

  /** While it opens: a line, without fuss. */
  opening(): void {
    this.element.dataset.state = "opening";
    this.element.innerHTML = `<p class="su-opening" role="status"><span class="su-dots" aria-hidden="true"><span></span><span></span><span></span></span><span>${escape(this.words.opening)}</span></p>`;
  }

  /** It did not open. */
  failed(failure: Failure | null): void {
    this.element.dataset.state = "failed";
    const said = (failure && this.words.steps[failure.step]) ?? this.words.stepOther;
    const acts = this.acts.repair
      ? `${this.button("repair", this.words.repair.repair, true)}${this.button("retry", this.words.retry, false)}`
      : this.button("retry", this.words.retry, true);
    this.frame(this.words.failed, `<p class="su-said"${failure ? ` title="${escape(failure.error)}"` : ""}>${escape(said)}</p><div class="su-acts">${acts}</div>`);
  }

  /** A repair under way, its layers as far as it has come. */
  repairing(steps: readonly RepairStep[]): void {
    this.layered("repairing", this.words.repair.repairing, steps, null, "");
  }

  /** Repaired: the opening is tried again by itself. */
  repaired(steps: readonly RepairStep[]): void {
    this.layered("repaired", this.words.reopening, steps, null, "");
  }

  /** The repair stopped short: what it went through, and its own words. */
  repairFailed(steps: readonly RepairStep[], error: string | null): void {
    const acts = `${this.button("repair", this.words.repair.repairAgain, true)}${this.button("retry", this.words.retry, false)}`;
    this.layered("broken", this.words.repair.repairFailed, steps, error, `<div class="su-acts">${acts}</div>`);
  }

  /** The page with a repair's layers: the frame drawn once for each state,
   *  the layers ticked in place as they come. */
  private layered(state: string, title: string, steps: readonly RepairStep[], error: string | null, after: string): void {
    if (this.element.dataset.state !== state || !this.layers) {
      this.element.dataset.state = state;
      this.frame(title, `<div class="su-layers" role="status"></div>${after}`);
      this.layers ??= new RepairLayers(this.words.repair);
      this.element.querySelector(".su-layers")!.append(this.layers.element);
    }
    const holder = this.element.querySelector<HTMLElement>(".su-layers")!;
    if (error) holder.title = error;
    else holder.removeAttribute("title");
    this.layers.update(steps);
  }

  private frame(title: string, body: string): void {
    this.element.innerHTML = `<div class="su-column"><h1 class="su-title">${escape(title)}</h1><div class="su-rule" aria-hidden="true"></div>${body}</div>`;
    // The stroke is drawn as long as the title once it is laid out.
    requestAnimationFrame(() => {
      const heading = this.element.querySelector<HTMLElement>(".su-title");
      const rule = this.element.querySelector<HTMLElement>(".su-rule");
      if (!heading || !rule) return;
      const range = document.createRange();
      range.selectNodeContents(heading);
      const width = Math.max(48, Math.round(range.getBoundingClientRect().width) + 6);
      rule.innerHTML = new Sketch(`startup-${title}`).rule([2, 4], [width - 2, 3.4], 1.3, "graphite").svg(width, 8, "su-rule-svg");
    });
  }

  private button(act: string, label: string, main: boolean): string {
    const mark = act === "repair" ? glyph("repair", 16) : "";
    return `<button class="${main ? "su-main" : "su-quiet"}" type="button" data-act="${act}">${mark}<span>${escape(label)}</span></button>`;
  }
}
