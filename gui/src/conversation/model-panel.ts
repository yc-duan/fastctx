// The conversation's model and reasoning effort (`world-gui.md`, 对话): the
// chip under the input says them; pressed, a small panel rises over it, on a
// phone from the bottom. The model's name opens the host's list of models;
// the effort is the hand-drawn slider, a step for each the model offers;
// where the model offers a speed, a switch for it, and where it offers more
// than one, a single choice among them and the standard one, since a turn
// runs at one speed; and a way back to the default. What is chosen shows at once, in the panel and on the chip, and
// goes to the host, which holds it from the next turn; choices made while
// one is on its way follow it in turn, the latest of them only. A choice the
// host refuses gives way to what the host has, and why is said under the
// input. The chip names the speed only when it is not the standard one.

import { EffortSlider, effortLabel } from "./effort-slider";
import { Popover } from "./popover";
import { glyph } from "./glyphs";
import { escape } from "./prose";
import type { ModelOffer, Offers } from "../world/control";

export interface ModelPanelWords {
  model: string;
  effort: string;
  speed: string;
  /** The speed of a turn that asks for none of the faster ones. */
  standard: string;
  backToDefault: string;
  noModels: string;
  /** The chip's words before the host has said its model. */
  chip: string;
  /** Said of a model whose window this conversation has outgrown. */
  tooLong: string;
}

export const zhModelPanel: ModelPanelWords = { model: "模型", effort: "推理强度", speed: "速度", standard: "标准", backToDefault: "回到默认", noModels: "没有可选的模型", chip: "模型", tooLong: "这段对话已超过它的上下文" };
export const enModelPanel: ModelPanelWords = { model: "Model", effort: "Reasoning", speed: "Speed", standard: "Standard", backToDefault: "Back to default", noModels: "No models available", chip: "Model", tooLong: "This conversation is longer than its context" };

export interface ModelChoice {
  /** The model and effort from the next turn on; null keeps what is. */
  model(model: string | null, effort: string | null): Promise<void>;
  /** The speed tier from the next turn on: a tier's id, or "default". */
  tier(id: string): Promise<void>;
}

/** A choice the host has not taken yet; null leaves that part as it is. */
interface Wanted {
  model: string | null;
  effort: string | null;
  tier: string | null;
}

export class ModelPanel {
  readonly element: HTMLElement;
  private readonly slider: EffortSlider;
  private readonly popover: Popover;
  private offered: Offers | null = null;
  private listing = false;
  private wanted: Wanted | null = null;
  /** The panel as last drawn: drawn again only when it differs, so a
   *  slider held by the pointer stays where it is. */
  private drawn = "";
  /** The choices on their way to the host, one after another. */
  private sending: Promise<void> = Promise.resolve();
  private asked = 0;

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
    this.slider = new EffortSlider(words.effort, (effort) => this.want({ effort }));
    this.element.addEventListener("click", (event) => this.click(event));
    this.popover = new Popover(this.element, chip, () => {
      this.listing = false;
      this.element.style.height = "";
    });
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
    this.popover.show();
    this.fit(listing);
    this.element.querySelector<HTMLElement>("button, [tabindex]")?.focus({ preventScroll: true });
  }

  /** The panel's size, for as long as it is open: as tall as the taller of
   *  its two faces, the controls and the list of models, so turning from
   *  one to the other moves nothing. Both are laid out at once, before it
   *  is painted. */
  private fit(listing: boolean): void {
    this.element.style.height = "";
    let tallest = 0;
    for (const face of [!listing, listing]) {
      this.listing = face;
      this.draw();
      tallest = Math.max(tallest, this.element.getBoundingClientRect().height);
    }
    this.element.style.height = `${Math.ceil(tallest)}px`;
  }

  hide(): void {
    this.popover.hide();
  }

  /** What the host offers, with what the user chose and the host has not
   *  taken yet. */
  private shown(): Offers | null {
    const { offered, wanted } = this;
    if (!offered || !wanted) return offered;
    return { ...offered, model: wanted.model ?? offered.model, effort: wanted.effort ?? offered.effort, service_tier: wanted.tier ?? offered.service_tier };
  }

  private current(): ModelOffer | undefined {
    const offered = this.shown();
    return offered?.models.find((model) => model.id === offered.model);
  }

  private drawChip(): void {
    const offered = this.shown();
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
    const offered = this.shown();
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
    const chosen = tiers.find((tier) => tier.id === offered?.service_tier)?.id ?? "default";
    const speed =
      tiers.length === 1
        ? `<div class="model-row"><button class="model-switch" type="button" role="switch" data-tier="${escape(tiers[0].id)}" aria-checked="${chosen === tiers[0].id}" title="${escape(tiers[0].description)}"><span>${escape(tiers[0].name)}</span><span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button></div>`
        : tiers.length > 1
          ? `<div class="model-speed"><span class="model-label">${escape(words.speed)}</span><div class="segmented model-speeds" role="radiogroup" aria-label="${escape(words.speed)}">${[{ id: "default", name: words.standard, description: "" }, ...tiers]
              .map((tier) => `<button class="segment${tier.id === chosen ? " chosen" : ""}" type="button" role="radio" data-tier="${escape(tier.id)}" aria-checked="${tier.id === chosen}"${tier.description ? ` title="${escape(tier.description)}"` : ""}>${escape(tier.name)}</button>`)
              .join("")}</div></div>`
          : "";
    // The list of models takes the place of the controls under the name.
    const controls = `${model && model.efforts.length > 1 ? `<div class="model-effort"><span class="model-label">${escape(words.effort)}</span><div class="model-slider"></div></div>` : ""}
      ${speed}
      <button class="text-button quiet-button model-default" type="button" data-default>${glyph("regenerate", 14)}<span>${escape(words.backToDefault)}</span></button>`;
    const html = `
      <button class="model-name" type="button" data-list aria-expanded="${this.listing}"><span class="model-label">${escape(words.model)}</span><span class="model-value">${escape(model?.name ?? offered?.model ?? "")}</span>${glyph("chevron", 14)}</button>
      ${this.listing ? list : controls}`;
    if (html !== this.drawn || !this.element.firstElementChild) {
      this.element.innerHTML = html;
      this.drawn = html;
    }
    const holder = this.element.querySelector<HTMLElement>(".model-slider");
    if (holder && model) {
      this.slider.set(model.efforts.map((value) => ({ value, label: effortLabel(value, this.english) })), offered?.effort ?? model.default_effort);
      if (this.slider.element.parentElement !== holder) holder.append(this.slider.element);
    }
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("button");
    if (!target) return;
    if (target.dataset.list !== undefined) {
      this.listing = !this.listing;
      this.draw();
      return;
    }
    const offered = this.shown();
    if (target.dataset.model) {
      const chosen = offered?.models.find((one) => one.id === target.dataset.model);
      if (!chosen) return;
      // A model that has the effort chosen keeps it; one that has not takes
      // its own default.
      const effort = offered?.effort && chosen.efforts.includes(offered.effort) ? offered.effort : chosen.default_effort;
      this.listing = false;
      this.want({ model: chosen.id, effort });
      return;
    }
    if (target.dataset.tier) {
      const on = target.getAttribute("aria-checked") === "true";
      // A switch turns its speed off again; a choice among speeds stays.
      if (target.getAttribute("role") === "radio") {
        if (!on) this.want({ tier: target.dataset.tier });
      } else this.want({ tier: on ? "default" : target.dataset.tier });
      return;
    }
    if (target.dataset.default !== undefined) {
      const fallback = offered?.models.find((one) => one.default) ?? offered?.models[0];
      if (!fallback) return;
      this.want({ model: fallback.id, effort: fallback.default_effort, tier: offered?.service_tier && offered.service_tier !== "default" ? "default" : null });
    }
  }

  /** Shows a choice at once and sends it after those on their way. */
  private want(next: Partial<Wanted>): void {
    const current = this.current();
    this.wanted = { model: this.wanted?.model ?? current?.id ?? null, effort: this.wanted?.effort ?? null, tier: this.wanted?.tier ?? null, ...next };
    this.drawChip();
    if (this.popover.open) this.draw();
    const asked = ++this.asked;
    this.sending = this.sending.then(async () => {
      // A later choice carries this one along.
      if (asked !== this.asked) return;
      await this.send();
      if (asked !== this.asked) return;
      this.wanted = null;
      this.drawChip();
      if (this.popover.open) this.draw();
    });
  }

  /** Gives the host what is wanted and it does not have. Whether it took
   *  it is the host's to say: the page shows what the host has after. */
  private async send(): Promise<void> {
    const { wanted, offered } = this;
    if (!wanted) return;
    try {
      if ((wanted.model !== null && wanted.model !== offered?.model) || (wanted.effort !== null && wanted.effort !== offered?.effort)) {
        await this.choose.model(wanted.model ?? offered?.model ?? null, wanted.effort);
      }
      if (wanted.tier !== null && wanted.tier !== (offered?.service_tier ?? "default")) await this.choose.tier(wanted.tier);
    } catch (error) {
      console.error("choosing the model", error);
    }
  }
}
