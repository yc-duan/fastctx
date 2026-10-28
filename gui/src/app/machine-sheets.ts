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

/** The most jobs at once each account a machine is given to runs on it, and
 *  how to change that; only a machine's sheet has them. */
export interface JobCaps {
  of(account: string): number | null;
  /** Sets it, or takes it away with none; answers whom it is given to then. */
  set(account: string, jobs: number | null): Promise<string[]>;
}

/** Giving a machine, or one login of an agent on it, to other accounts on
 *  the hub: those it is given to, each with a way to take it back and, for
 *  a machine, the most jobs it runs there at once; then whom to give it to. */
export class GiveSheet {
  private readonly sheet = new Sheet("device-sheet give-sheet");
  private said: GiveWords | null = null;
  private given: string[] = [];
  private give: (to: string, given: boolean) => Promise<string[]> = async () => [];
  private caps: JobCaps | null = null;
  /** The account whose limit of jobs is being changed, as typed so far. */
  private capping: { account: string; jobs: string } | null = null;

  constructor(private readonly words: AccessWords) {
    this.sheet.body.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
      if (!button || button.disabled) return;
      if (button.dataset.takeBack) void this.change(button.dataset.takeBack, false);
      else if (button.dataset.cap) this.editCap(button.dataset.cap);
      else if (button.hasAttribute("data-cap-save")) this.saveCap();
      else if (button.hasAttribute("data-cap-cancel")) this.editCap(null);
    });
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      if (this.capping) return this.saveCap();
      const to = this.sheet.body.querySelector<HTMLInputElement>("[name=to]")!.value.trim();
      if (to) void this.change(to, true);
    });
  }

  /** Opens the sheet, saying `said`, given to `given` now; `give` gives it
   *  to an account or takes it back, and answers whom it is given to then;
   *  `caps`, for a machine, its limits of jobs at once. */
  open(said: GiveWords, given: string[], give: (to: string, given: boolean) => Promise<string[]>, caps: JobCaps | null = null): void {
    this.said = said;
    this.given = given;
    this.give = give;
    this.caps = caps;
    this.capping = null;
    this.sheet.body.innerHTML = "";
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLInputElement>("[name=to]")?.focus();
  }

  private editCap(account: string | null): void {
    const most = account ? (this.caps?.of(account) ?? null) : null;
    this.capping = account ? { account, jobs: most === null ? "" : String(most) } : null;
    redrawKeepingFocus(this.sheet.body, () => this.draw());
    const field = this.sheet.body.querySelector<HTMLInputElement>("[name=jobs]");
    field?.focus();
    field?.select();
  }

  private saveCap(): void {
    const { capping, caps } = this;
    if (!capping || !caps) return;
    const jobs = jobsTyped(this.sheet.body.querySelector<HTMLInputElement>("[name=jobs]")?.value ?? capping.jobs);
    if (jobs === undefined) {
      const note = this.sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      note.textContent = this.words.jobsBad;
      note.title = "";
      note.hidden = false;
      return;
    }
    void this.run(() => caps.set(capping.account, jobs), false);
  }

  /** An account's limit of jobs at once as a pill to change it, and when it
   *  is being changed, the field to type it in under the row. */
  private cap(account: string): { pill: string; editor: string } {
    const { words, caps, capping } = this;
    if (!caps) return { pill: "", editor: "" };
    const most = caps.of(account);
    const pill = `<button class="assist-until give-cap${most === null ? " none" : ""}" type="button" data-cap="${escape(account)}" title="${escape(words.jobsHint)}"><span>${escape(most === null ? words.jobsNone : words.jobsOf(most))}</span></button>`;
    const editor =
      capping?.account === account
        ? `<div class="give-cap-edit"><label class="field assist-amount" title="${escape(words.jobsHint)}"><span class="assist-amount-input"><input name="jobs" inputmode="numeric" autocomplete="off" spellcheck="false" aria-label="${escape(words.jobsLabel)}" placeholder="${escape(words.jobsNone)}" value="${escape(capping.jobs)}"><span class="assist-unit">${escape(words.jobsUnit)}</span></span></label><div class="assist-edit-acts"><button class="text-button quiet-button" type="button" data-cap-cancel>${escape(words.cancel)}</button><button class="sheet-go" type="button" data-cap-save>${escape(words.save)}</button></div></div>`
        : "";
    return { pill, editor };
  }

  private draw(): void {
    const { words, sheet, said } = this;
    if (!said) return;
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=to]")?.value ?? "";
    const rows = this.given
      .map((account) => {
        const { pill, editor } = this.cap(account);
        return `<li class="give-row${editor ? " editing" : ""}">
          <span class="give-who">${glyph("person", 14)}<span>${escape(account)}</span></span>
          ${pill}
          <button class="text-button quiet-button" type="button" data-take-back="${escape(account)}">${escape(words.takeBack)}</button>
          ${editor}
        </li>`;
      })
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
    await this.run(() => this.give(to, given), given);
  }

  /** Does `work` with the sheet held still, and draws whom it is given to
   *  as it answers; `fresh` when one was just given it, whose field empties. */
  private async run(work: () => Promise<string[]>, fresh: boolean): Promise<void> {
    const { sheet, words } = this;
    if (sheet.busy) return;
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    note.hidden = true;
    try {
      this.given = await work();
      sheet.busy = false;
      this.capping = null;
      redrawKeepingFocus(sheet.body, () => this.draw());
      if (fresh) {
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

/** A limit as typed: a whole number from 1, none when empty, and undefined
 *  otherwise. */
function jobsTyped(text: string): number | null | undefined {
  const plain = text.replace(/\s/g, "");
  if (!plain) return null;
  const jobs = Number(plain);
  return /^\d+$/.test(plain) && jobs >= 1 && jobs <= 10_000 ? jobs : undefined;
}
