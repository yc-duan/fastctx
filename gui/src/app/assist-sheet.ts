// One login of an agent assisting others (`world-gui.md`, 账号、机器、共享与
// 协助): whom it assists now, each until when, the deadline changed or the
// assistance ended in place; then whom it assists next, and until when.
// The hub ends an assistance by itself at its deadline.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccessWords } from "./account-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { WhenPicker } from "./when";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

/** One account the login assists, and until when (none: no end). */
export interface Assistance {
  to: string;
  until: string | null;
}

/** Lets the login assist `to` until `until`, or no longer; answers the
 *  assistance as it is then. */
export type Assist = (to: string, until: Date | null, assisting: boolean) => Promise<Assistance[]>;

/** Whose deadline is being changed, and the picker it is changed in. */
type Editing = { to: string; picker: WhenPicker } | null;

export class AssistSheet {
  private readonly sheet = new Sheet("device-sheet assist-sheet");
  private title = "";
  private assistances: Assistance[] = [];
  private assist: Assist = async () => [];
  /** The deadline for the next one assisted. */
  private fresh: WhenPicker;
  private editing: Editing = null;

  constructor(private readonly words: AccessWords) {
    this.fresh = new WhenPicker(words.when, null);
    this.sheet.body.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
      if (!button || button.disabled) return;
      if (button.dataset.takeBack) void this.change(button.dataset.takeBack, null, false);
      else if (button.dataset.change) this.edit(button.dataset.change);
      else if (button.dataset.save && this.editing) void this.change(this.editing.to, this.editing.picker.value, true);
      else if (button.hasAttribute("data-cancel")) this.edit(null);
    });
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      if (this.editing) return;
      const to = this.sheet.body.querySelector<HTMLInputElement>("[name=to]")!.value.trim();
      if (to) void this.change(to, this.fresh.value, true);
    });
  }

  /** Opens the sheet titled `title` on whom the login assists now;
   *  `editing` opens one deadline to be changed at once. */
  open(title: string, assistances: Assistance[], assist: Assist, editing: string | null = null): void {
    this.title = title;
    this.assistances = assistances;
    this.assist = assist;
    this.fresh = new WhenPicker(this.words.when, null);
    this.editing = null;
    this.sheet.body.innerHTML = "";
    if (editing && assistances.some((one) => one.to === editing)) this.edit(editing);
    else this.draw();
    this.sheet.show();
    const opened = this.editing as Editing;
    if (opened) opened.picker.reveal();
    else this.sheet.body.querySelector<HTMLInputElement>("[name=to]")?.focus();
  }

  private edit(to: string | null): void {
    const found = this.assistances.find((one) => one.to === to);
    this.editing = found ? { to: found.to, picker: new WhenPicker(this.words.when, found.until ? new Date(found.until) : null) } : null;
    redrawKeepingFocus(this.sheet.body, () => this.draw());
    this.editing?.picker.reveal();
  }

  private draw(): void {
    const { words, sheet, editing } = this;
    const now = new Date();
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=to]")?.value ?? "";
    const rows = this.assistances
      .map((one) => {
        const until = one.until ? new Date(one.until) : null;
        const open = editing?.to === one.to;
        return `<li class="assist-row${open ? " editing" : ""}">
          <span class="give-who">${glyph("person", 14)}<span>${escape(one.to)}</span></span>
          <button class="assist-until${until ? "" : " none"}" type="button" data-change="${escape(one.to)}" title="${escape(words.changeUntil)}">${glyph("history", 14)}<span>${escape(until ? words.moment.until(until, now) : words.when.none)}</span></button>
          <button class="text-button quiet-button" type="button" data-take-back="${escape(one.to)}">${escape(words.endAssist)}</button>
          ${open ? `<div class="assist-edit"><div class="assist-edit-when"></div><div class="assist-edit-acts"><button class="text-button quiet-button" type="button" data-cancel>${escape(words.cancel)}</button><button class="sheet-go" type="button" data-save="1">${escape(words.save)}</button></div></div>` : ""}
        </li>`;
      })
      .join("");
    sheet.body.innerHTML = `
      ${sheet.head(this.title, words.close)}
      <p class="confirm-what">${escape(words.assistWhat)}</p>
      <p class="field-hint give-sees">${glyph("shield", 14)}<span>${escape(words.assistSees)}</span></p>
      <section class="give-list">
        <h3 class="field-label">${escape(words.assisting)}</h3>
        ${rows ? `<ul class="assist-rows">${rows}</ul>` : `<p class="field-hint">${escape(words.nobodyYet)}</p>`}
      </section>
      ${
        // While one deadline is changed, the next one assisted waits.
        editing
          ? `<input type="hidden" name="to" value="${escape(typed)}">`
          : `<div class="assist-new">
        <label class="field"><span class="field-label">${escape(words.assistWho)}</span>
          <input name="to" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="${escape(words.toHint)}" value="${escape(typed)}"></label>
        <div class="field assist-when"><span class="field-label">${escape(words.assistUntil)}</span></div>
      </div>`
      }
      <p class="sheet-note" role="alert" hidden></p>
      ${editing ? "" : `<footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.assistGo)}</button></footer>`}`;
    if (editing) sheet.body.querySelector(".assist-edit-when")?.append(editing.picker.element);
    else {
      sheet.body.querySelector(".assist-when")!.append(this.fresh.element);
      this.fresh.reveal();
    }
  }

  private async change(to: string, until: Date | null, assisting: boolean): Promise<void> {
    const { sheet, words } = this;
    if (sheet.busy) return;
    if (assisting && until && until <= new Date()) return this.say(words.untilPassed);
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    sheet.body.querySelector<HTMLElement>(".sheet-note")!.hidden = true;
    try {
      this.assistances = await this.assist(to, until, assisting);
      sheet.busy = false;
      const wasNew = assisting && this.editing?.to !== to;
      this.editing = null;
      if (wasNew) this.fresh = new WhenPicker(words.when, null);
      redrawKeepingFocus(sheet.body, () => this.draw());
      if (wasNew) {
        const field = sheet.body.querySelector<HTMLInputElement>("[name=to]")!;
        field.value = "";
        field.focus();
      }
    } catch (error) {
      sheet.busy = false;
      for (const control of controls) control.disabled = false;
      this.say(words.failed(error instanceof Error ? error.message : String(error)), error);
    }
  }

  private say(text: string, error?: unknown): void {
    const note = this.sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    note.textContent = text;
    note.title = error instanceof Error ? error.message : error ? String(error) : "";
    note.hidden = false;
  }
}
