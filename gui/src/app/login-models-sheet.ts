// The models of one Claude Code login (`world-gui.md`, 对话): those who hold
// its machine take rows of the World's own table out, offer them again, add
// model ids of their own, or go back to the table as it comes. What they
// keep is what everyone who uses the login picks from, those it assists
// too; its node refuses a change that would leave it none.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AgentModel } from "../world/agents";
import type { Login, MachineLogins, ModelChange } from "../world/logins";
import type { AgentLoginsWords } from "./agent-logins-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";

/** What the sheet opens on: the login, and how its models are changed,
 *  answering the machine's record as it is then. */
export interface ModelsOpening {
  title: string;
  login: Login;
  change(change: ModelChange): Promise<MachineLogins>;
}

export class LoginModelsSheet {
  private readonly sheet = new Sheet("device-sheet login-models-sheet");
  private opening: ModelsOpening | null = null;
  private login: Login | null = null;

  constructor(private readonly words: AgentLoginsWords) {
    this.sheet.body.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
      if (!button || button.disabled) return;
      if (button.dataset.remove) void this.change({ do: "remove", model: button.dataset.remove });
      else if (button.dataset.back) void this.change({ do: "add", model: button.dataset.back });
      else if (button.hasAttribute("data-reset")) void this.change({ do: "reset" });
      else if (button.hasAttribute("data-done")) this.sheet.close();
    });
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      const field = this.sheet.body.querySelector<HTMLInputElement>("[name=model]");
      const model = field?.value.trim() ?? "";
      if (model) void this.change({ do: "add", model }, true);
    });
  }

  open(opening: ModelsOpening): void {
    this.opening = opening;
    this.login = opening.login;
    this.sheet.body.innerHTML = "";
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLInputElement>("[name=model]")?.focus();
  }

  private draw(): void {
    const { words, sheet, opening, login } = this;
    if (!opening || !login) return;
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=model]")?.value ?? "";
    const models = login.models ?? [];
    const edits = login.model_edits ?? {};
    const added = new Set(edits.added ?? []);
    const removed = edits.removed ?? [];
    const last = models.length <= 1;
    const row = (model: AgentModel) => {
      const own = added.has(model.model);
      const named = model.display_name && model.display_name !== model.model;
      return `<li class="lm-row">
        <span class="lm-name">${escape(named ? model.display_name : model.model)}</span>
        ${named ? `<code class="lm-id">${escape(model.model)}</code>` : own ? `<span class="lm-own">${escape(words.modelsOwn)}</span>` : ""}
        <button class="icon-button lm-take" type="button" data-remove="${escape(model.model)}" ${last ? "disabled" : ""} title="${escape(last ? words.modelsLast : words.modelsRemove)}" aria-label="${escape(`${words.modelsRemove} ${model.model}`)}">${glyph("close", 16)}</button>
      </li>`;
    };
    const back = (id: string) => `<li class="lm-row lm-out">
        <code class="lm-id">${escape(id)}</code>
        <button class="text-button quiet-button" type="button" data-back="${escape(id)}">${escape(words.modelsBack)}</button>
      </li>`;
    const changed = removed.length > 0 || added.size > 0;
    sheet.body.innerHTML = `
      ${sheet.head(opening.title, words.modelsClose)}
      <p class="confirm-what">${escape(words.modelsWhat)}</p>
      <ul class="lm-rows" aria-label="${escape(words.modelsTitle)}">${models.map(row).join("")}</ul>
      <label class="field lm-add"><span class="field-label">${escape(words.modelsAdd)}</span>
        <span class="lm-add-line">
          <input name="model" autocomplete="off" spellcheck="false" autocapitalize="off" maxlength="128" placeholder="${escape(words.modelsAddHint)}" value="${escape(typed)}">
          <button class="text-button" type="submit">${escape(words.modelsAddGo)}</button>
        </span>
        <span class="field-hint">${escape(words.modelsAddWhy)}</span>
      </label>
      ${removed.length ? `<section class="lm-removed"><h3 class="field-label">${escape(words.modelsRemoved)}</h3><ul class="lm-rows">${removed.map(back).join("")}</ul></section>` : ""}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot">
        ${changed ? `<button class="text-button quiet-button" type="button" data-reset>${escape(words.modelsReset)}</button>` : "<span></span>"}
        <button class="sheet-go" type="button" data-done>${escape(words.modelsDone)}</button>
      </footer>`;
  }

  /** Makes `change` with the sheet held still, and draws the login as its
   *  machine answers it; `typed`: the field is cleared once it went in. */
  private async change(change: ModelChange, typed = false): Promise<void> {
    const { sheet, opening } = this;
    if (!opening || sheet.busy) return;
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    sheet.body.querySelector<HTMLElement>(".sheet-note")!.hidden = true;
    try {
      const record = await opening.change(change);
      const dir = opening.login.dir;
      const login = [...record.claude.listed, ...record.claude.candidates].find((one) => one.dir === dir) ?? null;
      sheet.busy = false;
      if (login) this.login = login;
      if (typed) {
        const field = sheet.body.querySelector<HTMLInputElement>("[name=model]");
        if (field) field.value = "";
      }
      redrawKeepingFocus(sheet.body, () => this.draw());
      if (typed) sheet.body.querySelector<HTMLInputElement>("[name=model]")?.focus();
    } catch (error) {
      sheet.busy = false;
      redrawKeepingFocus(sheet.body, () => this.draw());
      this.say(error);
    }
  }

  private say(error: unknown): void {
    const note = this.sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    const why = error instanceof Error ? error.message : String(error);
    note.textContent = this.words.modelsFailed(why);
    note.title = why;
    note.hidden = false;
  }
}
