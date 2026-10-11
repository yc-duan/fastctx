// The conversation's model and reasoning effort (`world-gui.md`, 对话): the
// chip under the input says them; pressed, a small panel rises over it, on a
// phone from the bottom. The model's name opens the host's list of models;
// the effort is the hand-drawn slider, a step for each the model offers;
// where the model offers a speed, a switch for it; and a way back to the
// default. What is chosen goes to the host at once and holds from the next
// turn. The chip names the speed only when it is not the standard one.

import { EffortSlider, effortLabel } from "./effort-slider";
import { Popover } from "./popover";
import { glyph } from "./glyphs";
import { escape } from "./prose";
import type { ModelOffer, Offers } from "../world/control";

export interface ModelPanelWords {
  model: string;
  effort: string;
  speed: string;
  backToDefault: string;
  noModels: string;
  /** The chip's words before the host has said its model. */
  chip: string;
  /** Said of a model whose window this conversation has outgrown. */
  tooLong: string;
}

export const zhModelPanel: ModelPanelWords = { model: "模型", effort: "推理强度", speed: "速度", backToDefault: "回到默认", noModels: "没有可选的模型", chip: "模型", tooLong: "这段对话已超过它的上下文" };
export const enModelPanel: ModelPanelWords = { model: "Model", effort: "Reasoning", speed: "Speed", backToDefault: "Back to default", noModels: "No models available", chip: "Model", tooLong: "This conversation is longer than its context" };

export interface ModelChoice {
  /** The model and effort from the next turn on; null keeps what is. */
  model(model: string | null, effort: string | null): Promise<void>;
  /** The speed tier from the next turn on: a tier's id, or "default". */
  tier(id: string): Promise<void>;
}

export class ModelPanel {
  readonly element: HTMLElement;
  private readonly slider: EffortSlider;
  private readonly popover: Popover;
  private offered: Offers | null = null;
  private listing = false;
  private busy = false;

  constructor(
    private readonly chip: HTMLButtonElement,
    private readonly words: ModelPanelWords,
    private readonly english: boolean,
    private readonly choose: ModelChoice,
  ) {
    this.element = document.createElement("div");
    this.element.className = "model-panel";
    this.element.hidden = true;
    this.element.setAttribute("role", "dialog");
    this.element.setAttribute("aria-label", words.model);
    this.slider = new EffortSlider(words.effort, (effort) => void this.change(() => this.choose.model(this.current()?.id ?? null, effort)));
    this.element.addEventListener("click", (event) => this.click(event));
    this.popover = new Popover(this.element, chip, () => (this.listing = false));
    chip.addEventListener("click", () => (this.popover.open ? this.hide() : this.show()));
    chip.setAttribute("aria-haspopup", "dialog");
    this.element.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      this.hide();
      chip.focus();
    });
  }

  get open(): boolean {
    return this.popover.open;
  }

  /** What the host offers now: the chip and, when open, the panel follow. */
  update(offered: Offers | null): void {
    this.offered = offered;
    this.drawChip();
    if (this.popover.open) this.draw();
  }

  show(listing = false): void {
    this.listing = listing;
    this.popover.show();
    this.draw();
    this.element.querySelector<HTMLElement>("button, [tabindex]")?.focus({ preventScroll: true });
  }

  hide(): void {
    this.popover.hide();
  }

  private current(): ModelOffer | undefined {
    const offered = this.offered;
    return offered?.models.find((model) => model.id === offered.model);
  }

  private drawChip(): void {
    const offered = this.offered;
    const shown = !!offered && offered.controls.includes("model");
    this.chip.hidden = !shown;
    if (!shown) return;
    const model = this.current();
    const tier = offered.service_tier && offered.service_tier !== "default" ? (model?.service_tiers.find((t) => t.id === offered.service_tier)?.name ?? offered.service_tier) : null;
    const effort = offered.effort ? effortLabel(offered.effort, this.english) : null;
    this.chip.innerHTML = `<span>${escape(model?.name ?? offered.model ?? this.words.chip)}</span>${effort ? `<span class="chip-effort" title="${escape(offered.effort ?? "")}">${escape(effort)}</span>` : ""}${tier ? `<span class="chip-effort chip-tier">${escape(tier)}</span>` : ""}${glyph("chevron", 14)}`;
  }

  private draw(): void {
    const { words } = this;
    const offered = this.offered;
    const model = this.current();
    const models = offered?.models ?? [];
    const used = offered?.usage?.context?.used ?? null;
    const list = this.listing
      ? `<ul class="model-list" role="listbox">${models.length ? models
          .map((one) => {
            // A conversation already longer than a model's window would be
            // refused on it: marked, not hidden, so the choice stays the user's.
            const tooLong = used !== null && one.context_window != null && used > one.context_window;
            const title = tooLong ? words.tooLong : (one.description ?? "");
            return `<li><button type="button" role="option" data-model="${escape(one.id)}" aria-selected="${one.id === model?.id}"${tooLong ? ` class="model-too-long"` : ""}${title ? ` title="${escape(title)}"` : ""}><span>${escape(one.name ?? one.id)}</span>${one.id === model?.id ? glyph("check", 14) : tooLong ? `<span class="model-mark" aria-hidden="true">${glyph("info", 13)}</span>` : ""}</button></li>`;
          })
          .join("") : `<li class="model-none">${escape(words.noModels)}</li>`}</ul>`
      : "";
    const tiers = model?.service_tiers ?? [];
    const speed = tiers.length
      ? `<div class="model-row">${tiers
          .map((tier) => {
            const on = offered?.service_tier === tier.id;
            return `<button class="model-switch" type="button" role="switch" data-tier="${escape(tier.id)}" aria-checked="${on}" title="${escape(tier.description)}"><span>${escape(tier.name)}</span><span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>`;
          })
          .join("")}</div>`
      : "";
    this.element.innerHTML = `
      <button class="model-name" type="button" data-list aria-expanded="${this.listing}"><span class="model-label">${escape(words.model)}</span><span class="model-value">${escape(model?.name ?? offered?.model ?? "")}</span>${glyph("chevron", 14)}</button>
      ${list}
      ${model && model.efforts.length > 1 ? `<div class="model-effort"><span class="model-label">${escape(words.effort)}</span><div class="model-slider"></div></div>` : ""}
      ${speed}
      <button class="text-button quiet-button model-default" type="button" data-default>${glyph("regenerate", 14)}<span>${escape(words.backToDefault)}</span></button>`;
    const holder = this.element.querySelector<HTMLElement>(".model-slider");
    if (holder && model) {
      this.slider.set(model.efforts.map((value) => ({ value, label: effortLabel(value, this.english) })), offered?.effort ?? model.default_effort);
      holder.append(this.slider.element);
    }
    this.element.classList.toggle("busy", this.busy);
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("button");
    if (!target || this.busy) return;
    if (target.dataset.list !== undefined) {
      this.listing = !this.listing;
      this.draw();
      return;
    }
    if (target.dataset.model) {
      const chosen = this.offered?.models.find((one) => one.id === target.dataset.model);
      if (!chosen) return;
      // A model that has the effort chosen keeps it; one that has not takes
      // its own default.
      const effort = this.offered?.effort && chosen.efforts.includes(this.offered.effort) ? this.offered.effort : chosen.default_effort;
      this.listing = false;
      void this.change(() => this.choose.model(chosen.id, effort));
      return;
    }
    if (target.dataset.tier) {
      const on = target.getAttribute("aria-checked") === "true";
      void this.change(() => this.choose.tier(on ? "default" : target.dataset.tier!));
      return;
    }
    if (target.dataset.default !== undefined) {
      const fallback = this.offered?.models.find((one) => one.default) ?? this.offered?.models[0];
      if (!fallback) return;
      void this.change(async () => {
        await this.choose.model(fallback.id, fallback.default_effort);
        if (this.offered?.service_tier && this.offered.service_tier !== "default") await this.choose.tier("default");
      });
    }
  }

  private async change(act: () => Promise<void>): Promise<void> {
    this.busy = true;
    this.element.classList.add("busy");
    try {
      await act();
    } finally {
      this.busy = false;
      this.element.classList.remove("busy");
    }
  }
}
