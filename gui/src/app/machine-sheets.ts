// What the owner of a machine sets on it (`world-network.md`, 共享机器与智能体
// 协助): whom the machine is shared with, working on it and every agent
// there; whom one login of an agent on it assists, that login alone;
// each said plainly with what the owner gives and sees. Each change is the
// account's own, made at once; the hub refuses what it may not do, and the
// sheet stays open with why.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccessWords } from "./account-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

/** IPv4 or bracketed IPv6, with a port. */
export const socketAddress = /^(?:\d{1,3}(?:\.\d{1,3}){3}|\[[0-9a-fA-F:.]+\]):\d{1,5}$/;

/** What a sheet that gives something to other accounts says. */
export interface GiveWords {
  title: string;
  what: string;
  sees: string;
  given: string;
  go: string;
  /** A line under it all, when there is one to say. */
  after?: string;
}

/** Giving a machine, or one login of an agent on it, to other accounts on
 *  the hub: those it is given to, each with a way to take it back, then
 *  whom to give it to. */
export class GiveSheet {
  private readonly sheet = new Sheet("device-sheet give-sheet");
  private said: GiveWords | null = null;
  private given: string[] = [];
  private give: (to: string, given: boolean) => Promise<string[]> = async () => [];

  constructor(private readonly words: AccessWords) {
    this.sheet.body.addEventListener("click", (event) => {
      const back = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-take-back]");
      if (back && !back.disabled) void this.change(back.dataset.takeBack!, false);
    });
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      const to = this.sheet.body.querySelector<HTMLInputElement>("[name=to]")!.value.trim();
      if (to) void this.change(to, true);
    });
  }

  /** Opens the sheet, saying `said`, given to `given` now; `give` gives it
   *  to an account or takes it back, and answers whom it is given to then. */
  open(said: GiveWords, given: string[], give: (to: string, given: boolean) => Promise<string[]>): void {
    this.said = said;
    this.given = given;
    this.give = give;
    this.sheet.body.innerHTML = "";
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLInputElement>("[name=to]")?.focus();
  }

  private draw(): void {
    const { words, sheet, said } = this;
    if (!said) return;
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=to]")?.value ?? "";
    const rows = this.given
      .map(
        (account) => `<li class="give-row">
          <span class="give-who">${glyph("person", 14)}<span>${escape(account)}</span></span>
          <button class="text-button quiet-button" type="button" data-take-back="${escape(account)}">${escape(words.takeBack)}</button>
        </li>`,
      )
      .join("");
    sheet.body.innerHTML = `
      ${sheet.head(said.title, words.close)}
      <p class="confirm-what">${escape(said.what)}</p>
      <p class="field-hint give-sees">${glyph("shield", 14)}<span>${escape(said.sees)}</span></p>
      <section class="give-list">
        <h3 class="field-label">${escape(said.given)}</h3>
        ${rows ? `<ul class="give-rows">${rows}</ul>` : `<p class="field-hint">${escape(words.nobodyYet)}</p>`}
      </section>
      <div class="give-new">
        <label class="field"><span class="field-label">${escape(words.to)}</span>
          <input name="to" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="${escape(words.toHint)}" value="${escape(typed)}"></label>
      </div>
      ${said.after ? `<p class="field-hint give-after">${escape(said.after)}</p>` : ""}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(said.go)}</button></footer>`;
  }

  private async change(to: string, given: boolean): Promise<void> {
    const { sheet, words } = this;
    if (sheet.busy) return;
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    note.hidden = true;
    try {
      this.given = await this.give(to, given);
      sheet.busy = false;
      redrawKeepingFocus(sheet.body, () => this.draw());
      if (given) {
        const field = sheet.body.querySelector<HTMLInputElement>("[name=to]")!;
        field.value = "";
        field.focus();
      }
    } catch (error) {
      sheet.busy = false;
      for (const control of controls) control.disabled = false;
      const shown = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      shown.textContent = words.failed(error instanceof Error ? error.message : String(error));
      shown.title = error instanceof Error ? error.message : String(error);
      shown.hidden = false;
    }
  }
}
