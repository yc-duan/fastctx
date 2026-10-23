// The sentinels' settings, on their feature page (`world-senses.md`): the
// machine they live on, which must stay on; the model they judge with, its
// kind of interface, its address and name, and its key; and, for the few
// that watch something only on the open web, the search they use. A key is
// typed here and handed to the machine they live on alone; only whether it
// is set is ever shown again.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { ModelProtocol, SentinelSeam, SentinelSettings, SentinelSettingsView } from "../world/sentinels";
import type { SentinelWords } from "./features-words";
import { detailOf } from "./problem";

export interface SentinelFormOptions {
  words: SentinelWords;
  seam: SentinelSeam;
  login: { hub: string; account: string };
  /** The machines of the account the sentinels may live on, and what
   *  kind of machine each is. */
  machines(): { name: string; form: "desktop" | "laptop" | "server" }[];
  /** The settings saved, with a home: the feature is on. */
  saved(view: SentinelSettingsView): void;
}

export class SentinelForm {
  readonly element: HTMLElement;
  private view: SentinelSettingsView | null = null;
  private draft: SentinelSettings = { home: null, model: null, search: null };
  /** Keys typed and not yet handed over. */
  private keys: { model: string; search: string } = { model: "", search: "" };
  private problem: { text: string; detail: string } | null = null;
  private busy = false;

  constructor(private readonly options: SentinelFormOptions) {
    this.element = document.createElement("div");
    this.element.className = "sf-form";
    this.element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const home = target.closest<HTMLElement>("[data-home]")?.dataset.home;
      if (home) {
        this.draft = { ...this.draft, home };
        return this.draw();
      }
      const protocol = target.closest<HTMLElement>("[data-protocol]")?.dataset.protocol as ModelProtocol | undefined;
      if (protocol) {
        this.draft = { ...this.draft, model: { ...(this.draft.model ?? { endpoint: "", model: "" }), protocol } };
        return this.draw();
      }
      if (target.closest("[data-act='save']")) void this.save();
    });
    this.element.addEventListener("input", (event) => {
      const input = event.target as HTMLInputElement;
      const field = input.dataset.field;
      if (!field) return;
      const value = input.value;
      if (field === "endpoint" || field === "model") {
        const model = this.draft.model ?? { protocol: "open_ai" as const, endpoint: "", model: "" };
        this.draft = { ...this.draft, model: { ...model, [field]: value } };
      } else if (field === "search-endpoint") {
        this.draft = { ...this.draft, search: value.trim() ? { provider: "tavily", endpoint: value } : null };
      } else if (field === "model-key") this.keys.model = value;
      else if (field === "search-key") this.keys.search = value;
      this.drawSave();
    });
  }

  show(view: SentinelSettingsView): void {
    this.view = view;
    this.draft = structuredClone(view.settings);
    this.keys = { model: "", search: "" };
    this.draw();
  }

  /** Nothing saved yet: the form opens on the first machine. */
  start(): void {
    this.view = null;
    this.draft = { home: this.options.machines()[0]?.name ?? null, model: { protocol: "open_ai", endpoint: "", model: "" }, search: null };
    this.draw();
  }

  private draw(): void {
    const { words } = this.options;
    const draft = this.draft;
    const listed = this.options.machines();
    const machines = [...(draft.home && !listed.some((one) => one.name === draft.home) ? [{ name: draft.home, form: "server" as const }] : []), ...listed];
    const homeSaid = this.view && this.view.settings.home === draft.home ? this.homeSaid(this.view) : "";
    const chip = (attr: string, value: string, label: string, chosen: boolean, icon = "") =>
      `<button class="feature-chip${chosen ? " chosen" : ""}" type="button" ${attr}="${escape(value)}" aria-pressed="${chosen}">${icon}<span>${escape(label)}</span></button>`;
    const model = draft.model;
    const keySaid = (set: boolean | undefined) =>
      set === undefined ? "" : `<span class="sf-key-said">${set ? `${glyph("check", 14)}<span>${escape(words.keySet)}</span>` : escape(words.keyMissing)}</span>`;
    const keys = this.view?.keys ?? undefined;
    this.element.innerHTML = `
      <section class="feature-section"><h2 class="feature-heading">${escape(words.home)}</h2>
        <div class="feature-chips">${machines.map((one) => chip("data-home", one.name, one.name, one.name === draft.home, glyph(one.form, 14))).join("")}</div>
        ${homeSaid ? `<p class="feature-quiet sf-home-said">${homeSaid}</p>` : ""}
      </section>
      <section class="feature-section"><h2 class="feature-heading">${escape(words.model)}</h2>
        <div class="feature-chips">${(["open_ai", "anthropic"] as const).map((one) => chip("data-protocol", one, words.protocols[one], model?.protocol === one)).join("")}</div>
        <div class="sf-fields">
          <label class="feature-field"><span>${escape(words.endpoint)}</span><input type="url" data-field="endpoint" value="${escape(model?.endpoint ?? "")}" placeholder="${escape(model?.protocol === "anthropic" ? "https://…/v1/messages" : "https://…/v1/chat/completions")}" spellcheck="false" autocomplete="off"></label>
          <label class="feature-field"><span>${escape(words.modelName)}</span><input type="text" data-field="model" value="${escape(model?.model ?? "")}" spellcheck="false" autocomplete="off"></label>
          <label class="feature-field" title="${escape(draft.home ? words.keyGoes(draft.home) : "")}"><span class="sf-key-label">${escape(words.key)}${keySaid(keys?.model)}</span><input type="password" data-field="model-key" placeholder="${escape(keys?.model ? words.keyReplace : "")}" autocomplete="off"></label>
        </div>
      </section>
      <section class="feature-section"><h2 class="feature-heading">${escape(words.search)}</h2>
        <div class="sf-fields">
          <label class="feature-field"><span>${escape(words.searchEndpoint)}</span><input type="url" data-field="search-endpoint" value="${escape(draft.search?.endpoint ?? "")}" placeholder="https://api.tavily.com/search" spellcheck="false" autocomplete="off"></label>
          <label class="feature-field" title="${escape(draft.home ? words.keyGoes(draft.home) : "")}"><span class="sf-key-label">${escape(words.key)}${keySaid(keys?.search)}</span><input type="password" data-field="search-key" placeholder="${escape(keys?.search ? words.keyReplace : "")}" autocomplete="off"></label>
        </div>
      </section>
      <div class="feature-save-bar"><span class="feature-problem"></span><button class="sheet-go feature-save" type="button" data-act="save">${escape(words.save)}</button></div>`;
    this.drawSave();
  }

  /** How the machine they live on stands, where it is not simply ready. */
  private homeSaid(view: SentinelSettingsView): string {
    const { words } = this.options;
    const status = view.home_status;
    if (status.kind === "ready" || status.kind === "unset") return "";
    if (status.kind === "unavailable") return `<span title="${escape(status.reason)}">${escape(words.homeStatus.unavailable)}</span>`;
    return escape(words.homeStatus[status.kind]);
  }

  /** Whether anything differs from what is saved. */
  private dirty(): boolean {
    const saved = this.view?.settings;
    return !saved || JSON.stringify(saved) !== JSON.stringify(this.draft) || this.keys.model.trim() !== "" || this.keys.search.trim() !== "";
  }

  /** Saving takes a machine, a model's address and its name. */
  private complete(): boolean {
    const model = this.draft.model;
    return !!this.draft.home && !!model && /^https?:\/\/\S+$/.test(model.endpoint.trim()) && model.model.trim() !== "";
  }

  private drawSave(): void {
    const bar = this.element.querySelector<HTMLElement>(".feature-save-bar");
    if (!bar) return;
    bar.classList.toggle("shown", this.dirty() || this.problem !== null);
    bar.querySelector<HTMLButtonElement>(".feature-save")!.disabled = this.busy || !this.dirty() || !this.complete();
    const problem = bar.querySelector<HTMLElement>(".feature-problem")!;
    problem.textContent = this.problem?.text ?? "";
    problem.title = this.problem?.detail ?? "";
  }

  private async save(): Promise<void> {
    if (this.busy || !this.complete()) return;
    const { seam, login, words } = this.options;
    this.busy = true;
    this.problem = null;
    this.drawSave();
    try {
      const settings: SentinelSettings = {
        ...this.draft,
        model: this.draft.model ? { ...this.draft.model, endpoint: this.draft.model.endpoint.trim(), model: this.draft.model.model.trim() } : null,
      };
      const reply = await seam.write(login.hub, login.account, this.view?.version ?? null, settings);
      if (reply.kind === "conflict") {
        this.problem = { text: words.changedElsewhere, detail: "" };
        this.busy = false;
        const fresh = await seam.read(login.hub, login.account);
        this.view = fresh;
        return this.drawSave();
      }
      // The keys go to the home once it is the one saved.
      if (this.keys.model.trim()) await seam.setKey(login.hub, login.account, "model", this.keys.model.trim());
      if (this.keys.search.trim()) await seam.setKey(login.hub, login.account, "search", this.keys.search.trim());
      const view = this.keys.model.trim() || this.keys.search.trim() ? await seam.read(login.hub, login.account) : reply.view;
      this.busy = false;
      this.show(view);
      this.options.saved(view);
    } catch (error) {
      this.busy = false;
      this.problem = { text: words.notSaved, detail: detailOf(error) };
      this.drawSave();
    }
  }
}
