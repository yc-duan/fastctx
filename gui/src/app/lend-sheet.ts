// Lending one login of an agent (`world-gui.md`, 账号、机器、共享与借出):
// whom it is lent to now, each until when, the deadline changed or the
// loan taken back in place; then whom to lend it to next, and until when.
// The hub ends a loan by itself at its deadline.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccessWords } from "./account-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { WhenPicker } from "./when";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

/** One loan of the login: to whom, and until when (none: no end). */
export interface Loan {
  to: string;
  until: string | null;
}

/** Lends the login to `to` until `until`, or takes it back; answers the
 *  loans as they are then. */
export type Lend = (to: string, until: Date | null, lent: boolean) => Promise<Loan[]>;

/** Whose deadline is being changed, and the picker it is changed in. */
type Editing = { to: string; picker: WhenPicker } | null;

export class LendSheet {
  private readonly sheet = new Sheet("device-sheet lend-sheet");
  private title = "";
  private loans: Loan[] = [];
  private lend: Lend = async () => [];
  /** The deadline for the next loan. */
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

  /** Opens the sheet titled `title` on the login's `loans` now; `editing`
   *  opens one loan's deadline to be changed at once. */
  open(title: string, loans: Loan[], lend: Lend, editing: string | null = null): void {
    this.title = title;
    this.loans = loans;
    this.lend = lend;
    this.fresh = new WhenPicker(this.words.when, null);
    this.editing = null;
    this.sheet.body.innerHTML = "";
    if (editing && loans.some((loan) => loan.to === editing)) this.edit(editing);
    else this.draw();
    this.sheet.show();
    const opened = this.editing as Editing;
    if (opened) opened.picker.reveal();
    else this.sheet.body.querySelector<HTMLInputElement>("[name=to]")?.focus();
  }

  private edit(to: string | null): void {
    const loan = this.loans.find((loan) => loan.to === to);
    this.editing = loan ? { to: loan.to, picker: new WhenPicker(this.words.when, loan.until ? new Date(loan.until) : null) } : null;
    redrawKeepingFocus(this.sheet.body, () => this.draw());
    this.editing?.picker.reveal();
  }

  private draw(): void {
    const { words, sheet, editing } = this;
    const now = new Date();
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=to]")?.value ?? "";
    const rows = this.loans
      .map((loan) => {
        const until = loan.until ? new Date(loan.until) : null;
        const open = editing?.to === loan.to;
        return `<li class="lend-row${open ? " editing" : ""}">
          <span class="give-who">${glyph("person", 14)}<span>${escape(loan.to)}</span></span>
          <button class="lend-until${until ? "" : " none"}" type="button" data-change="${escape(loan.to)}" title="${escape(words.changeUntil)}">${glyph("history", 14)}<span>${escape(until ? words.moment.until(until, now) : words.when.none)}</span></button>
          <button class="text-button quiet-button" type="button" data-take-back="${escape(loan.to)}">${escape(words.takeBack)}</button>
          ${open ? `<div class="lend-edit"><div class="lend-edit-when"></div><div class="lend-edit-acts"><button class="text-button quiet-button" type="button" data-cancel>${escape(words.cancel)}</button><button class="sheet-go" type="button" data-save="1">${escape(words.save)}</button></div></div>` : ""}
        </li>`;
      })
      .join("");
    sheet.body.innerHTML = `
      ${sheet.head(this.title, words.close)}
      <p class="confirm-what">${escape(words.lendWhat)}</p>
      <p class="field-hint give-sees">${glyph("shield", 14)}<span>${escape(words.lendSees)}</span></p>
      <section class="give-list">
        <h3 class="field-label">${escape(words.lentTo)}</h3>
        ${rows ? `<ul class="lend-rows">${rows}</ul>` : `<p class="field-hint">${escape(words.nobodyYet)}</p>`}
      </section>
      ${
        // While one loan's deadline is changed, the next loan waits.
        editing
          ? `<input type="hidden" name="to" value="${escape(typed)}">`
          : `<div class="lend-new">
        <label class="field"><span class="field-label">${escape(words.lendWho)}</span>
          <input name="to" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="${escape(words.toHint)}" value="${escape(typed)}"></label>
        <div class="field lend-when"><span class="field-label">${escape(words.lendUntil)}</span></div>
      </div>`
      }
      <p class="sheet-note" role="alert" hidden></p>
      ${editing ? "" : `<footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.lendGo)}</button></footer>`}`;
    if (editing) sheet.body.querySelector(".lend-edit-when")?.append(editing.picker.element);
    else {
      sheet.body.querySelector(".lend-when")!.append(this.fresh.element);
      this.fresh.reveal();
    }
  }

  private async change(to: string, until: Date | null, lent: boolean): Promise<void> {
    const { sheet, words } = this;
    if (sheet.busy) return;
    if (lent && until && until <= new Date()) return this.say(words.untilPassed);
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    sheet.body.querySelector<HTMLElement>(".sheet-note")!.hidden = true;
    try {
      this.loans = await this.lend(to, until, lent);
      sheet.busy = false;
      const wasNew = lent && this.editing?.to !== to;
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
