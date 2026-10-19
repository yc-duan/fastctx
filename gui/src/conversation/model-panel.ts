// The conversation's model and reasoning effort (`world-gui.md`, 对话): the
// chip under the input says them; pressed, a small panel rises over it, on a
// phone from the bottom. At its foot, nearest the chip, the model's name;
// above it, past a ruled line, the effort as the hand-drawn slider, a step
// for each the model offers, and where the model offers a speed, a switch
// for it, or where it offers more than one, a single choice among them and
// the standard one, since a turn runs at one speed. The name pressed, the
// host's models take the controls' place above it, the chosen one washed,
// and the panel grows or shrinks upward to fit, so it is never taller than
// what it holds and the name under the pointer never moves. What is chosen shows at once, in
// the panel and on the chip, and goes to the host, which holds it from the
// next turn; choices made while one is on its way follow it in turn, the
// latest of them only. A choice the host refuses gives way to what the host
// has, and why is said under the input. The chip names the speed only when
// it is not the standard one.

import { EffortSlider, effortLabel } from "./effort-slider";
import { Popover } from "./popover";
import { glyph } from "./glyphs";
import { escape } from "./prose";
import { handOnAtEdge } from "../scroll-handoff";
import { resizeUpward } from "../app/resize-upward";
import type { ModelOffer, Offers } from "../world/control";

export interface ModelPanelWords {
  model: string;
  effort: string;
  speed: string;
  /** The speed of a turn that asks for none of the faster ones. */
  standard: string;
  noModels: string;
  /** The chip's words before the host has said its model. */
  chip: string;
  /** Said of a model whose window this conversation has outgrown. */
  tooLong: string;
}

export const zhModelPanel: ModelPanelWords = { model: "模型", effort: "推理强度", speed: "速度", standard: "标准", noModels: "没有可选的模型", chip: "模型", tooLong: "这段对话已超过它的上下文" };
export const enModelPanel: ModelPanelWords = { model: "Model", effort: "Reasoning", speed: "Speed", standard: "Standard", noModels: "No models available", chip: "Model", tooLong: "This conversation is longer than its context" };

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
  /** The models are shown in the controls' place. */
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
    // It takes the focus itself on opening, so no control in it is ringed
    // as if the keyboard had walked there.
    this.element.tabIndex = -1;
    this.slider = new EffortSlider(words.effort, (effort) => this.want({ effort }));
    this.element.addEventListener("click", (event) => this.click(event));
    this.popover = new Popover(this.element, chip, () => {
      this.listing = false;
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

  /** Opens the panel; `listing`, at the models. A model with nothing to
   *  set but itself opens at them too. */
  show(listing = false): void {
    this.popover.show();
    this.listing = listing || !this.controlled();
    this.draw();
    this.element.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
    this.element.focus({ preventScroll: true });
  }

  /** Whether the model chosen has anything to set besides itself. */
  private controlled(): boolean {
    const model = this.current();
    return !!model && (model.efforts.length > 1 || model.service_tiers.length > 0);
  }

  /** Turns between the controls and the models, the panel growing or
   *  shrinking upward to fit what it holds now. */
  private turn(listing: boolean): void {
    resizeUpward(this.element, null, () => {
      this.listing = listing;
      this.draw();
    });
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
    const list = `<ul class="model-list" role="listbox" aria-label="${escape(words.model)}">${
      models.length
        ? models
            .map((one) => {
              // A conversation already longer than a model's window would be
              // refused on it: marked, not hidden, so the choice stays the user's.
              const tooLong = used !== null && one.context_window != null && used > one.context_window;
              const title = tooLong ? words.tooLong : (one.description ?? "");
              return `<li><button type="button" class="model-option${tooLong ? " model-too-long" : ""}" role="option" data-model="${escape(one.id)}" aria-selected="${one.id === model?.id}"${title ? ` title="${escape(title)}"` : ""}><span class="model-option-name">${escape(one.name ?? one.id)}</span>${tooLong ? `<span class="model-mark" aria-hidden="true">${glyph("info", 13)}</span>` : ""}</button></li>`;
            })
            .join("")
        : `<li class="model-none">${escape(words.noModels)}</li>`
    }</ul>`;
    const tiers = model?.service_tiers ?? [];
    const chosen = tiers.find((tier) => tier.id === offered?.service_tier)?.id ?? "default";
    const speed =
      tiers.length === 1
        ? `<button class="model-line model-switch" type="button" role="switch" data-tier="${escape(tiers[0].id)}" aria-checked="${chosen === tiers[0].id}" title="${escape(tiers[0].description)}"><span>${escape(tiers[0].name)}</span><span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>`
        : tiers.length > 1
          ? `<div class="model-line"><span class="model-label">${escape(words.speed)}</span><div class="segmented model-speeds" role="radiogroup" aria-label="${escape(words.speed)}">${[{ id: "default", name: words.standard, description: "" }, ...tiers]
              .map((tier) => `<button class="segment${tier.id === chosen ? " chosen" : ""}" type="button" role="radio" data-tier="${escape(tier.id)}" aria-checked="${tier.id === chosen}"${tier.description ? ` title="${escape(tier.description)}"` : ""}>${escape(tier.name)}</button>`)
              .join("")}</div></div>`
          : "";
    const effort = model && model.efforts.length > 1 ? `<div class="model-effort"><span class="model-label">${escape(words.effort)}</span><div class="model-slider"></div></div>` : "";
    const controls = effort || speed ? `<div class="model-controls">${effort}${speed}</div>` : "";
    // The model's name at the foot; above it, past a ruled line, the
    // controls or, the name pressed, the models.
    const head = `<button class="model-head" type="button" data-list aria-expanded="${this.listing}"><span class="model-label">${escape(words.model)}</span><span class="model-value">${escape(model?.name ?? offered?.model ?? "")}</span>${glyph("chevron", 14)}</button>`;
    const above = this.listing ? list : controls;
    const html = `${above}${above ? `<div class="model-rule" aria-hidden="true"></div>` : ""}${head}`;
    if (html !== this.drawn || !this.element.firstElementChild) {
      this.element.innerHTML = html;
      this.drawn = html;
      const models = this.element.querySelector<HTMLElement>(".model-list");
      if (models) handOnAtEdge(models);
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
      // A model with nothing else to set has only its models to show.
      if (this.listing && !this.controlled()) return;
      this.turn(!this.listing);
      return;
    }
    const offered = this.shown();
    if (target.dataset.model) {
      const chosen = offered?.models.find((one) => one.id === target.dataset.model);
      if (!chosen) return;
      // A model that has the effort chosen keeps it; one that has not takes
      // its own default.
      const effort = offered?.effort && chosen.efforts.includes(offered.effort) ? offered.effort : chosen.default_effort;
      this.want({ model: chosen.id, effort }, false);
      // Chosen, the models give way to what it can set, if anything.
      this.turn(!this.controlled());
      return;
    }
    if (target.dataset.tier) {
      const on = target.getAttribute("aria-checked") === "true";
      // A switch turns its speed off again; a choice among speeds stays.
      if (target.getAttribute("role") === "radio") {
        if (!on) this.want({ tier: target.dataset.tier });
      } else this.want({ tier: on ? "default" : target.dataset.tier });
    }
  }

  /** Shows a choice at once and sends it after those on their way; `draw`
   *  false leaves the panel to whoever draws it next. */
  private want(next: Partial<Wanted>, draw = true): void {
    const current = this.current();
    this.wanted = { model: this.wanted?.model ?? current?.id ?? null, effort: this.wanted?.effort ?? null, tier: this.wanted?.tier ?? null, ...next };
    this.drawChip();
    if (draw && this.popover.open) this.draw();
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
