// One login of an agent assisting others (`world-gui.md`, 账号、机器、共享与
// 协助): whom it assists now, each until when and, for Claude Code, up to
// how much a week, changed or ended in place; then whom it assists next,
// the network's accounts that match listed under the name as it is typed.
// The hub ends an assistance by itself at its deadline; the login's
// machine holds each account to its weekly amount (`world-agent-logins.md`,
// 用量).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccountChange, AssistSeen, HubAccounts, Login } from "../world/accounts";
import { wholeDollars } from "../world/money";
import type { AccessWords } from "./account-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { NameSuggest } from "./name-suggest";
import { WhenPicker } from "./when";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

/** One account the login assists, until when (none: no end), and what it
 *  may spend of the login a week, in dollars (none: no limit). */
export interface Assistance {
  to: string;
  until: string | null;
  weekly: number | null;
}

/** Lets the login assist `to` until `until` and up to `weekly` dollars a
 *  week, or no longer; answers the assistance as it is then. */
export type Assist = (to: string, until: Date | null, weekly: number | null, assisting: boolean) => Promise<Assistance[]>;

/** The accounts `agent`'s login `home` on `machine` assists, among a hub's
 *  assistances; `home` is none for the agent's default login there. */
export function assistancesIn(assists: AssistSeen[], machine: string, agent: string, home: string | null): Assistance[] {
  return assists
    .filter((assist) => assist.machine === machine && assist.agent === agent && (assist.home ?? null) === home)
    .map((assist) => ({ to: assist.account, until: assist.until ?? null, weekly: assist.weekly ? assist.weekly / 1_000_000 : null }));
}

/** The accounts a login may have assisted, as its hub tells them: every
 *  one it knows of but the login's own. */
export function namesOn(hub: HubAccounts | null | undefined, login: Login): string[] {
  return (hub?.accounts ?? []).map((account) => account.name).filter((name) => name !== login.account);
}

/** The change that has the login assist `to` as asked, or no longer. */
export function assistChange(machine: string, agent: string, home: string | null, to: string, until: Date | null, weekly: number | null, assisting: boolean): AccountChange {
  return {
    change: "assist",
    machine,
    to,
    agent,
    ...(home ? { home } : {}),
    assisting,
    ...(assisting && until ? { until: until.toISOString() } : {}),
    ...(assisting && weekly ? { weekly: Math.round(weekly * 1_000_000) } : {}),
  };
}

/** Whose assistance is being changed, and the picker its deadline is
 *  changed in. */
type Editing = { to: string; picker: WhenPicker; weekly: string } | null;

export class AssistSheet {
  private readonly sheet = new Sheet("device-sheet assist-sheet");
  private title = "";
  private assistances: Assistance[] = [];
  private assist: Assist = async () => [];
  /** Whether a weekly amount can be set: Claude Code keeps what it costs. */
  private weekly = false;
  /** The deadline for the next one assisted. */
  private fresh: WhenPicker;
  private editing: Editing = null;
  /** The accounts that may be assisted, to list as a name is typed. */
  private names: string[] = [];
  private readonly suggest: NameSuggest;

  constructor(private readonly words: AccessWords) {
    this.fresh = new WhenPicker(words.when, null);
    this.suggest = new NameSuggest(words.assistNames);
    this.sheet.body.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
      if (!button || button.disabled) return;
      if (button.dataset.takeBack) void this.change(button.dataset.takeBack, null, null, false);
      else if (button.dataset.change) this.edit(button.dataset.change);
      else if (button.dataset.save && this.editing) this.save();
      else if (button.hasAttribute("data-cancel")) this.edit(null);
    });
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      if (this.editing) return;
      const to = this.sheet.body.querySelector<HTMLInputElement>("[name=to]")!.value.trim();
      const weekly = amountOf(this.sheet.body.querySelector<HTMLInputElement>("[name=weekly]")?.value ?? "");
      if (weekly === undefined) return this.say(this.words.weeklyBad);
      if (to) void this.change(to, this.fresh.value, weekly, true);
    });
  }

  /** Opens the sheet titled `title` on whom the login assists now;
   *  `editing` opens one assistance to be changed at once; `weekly` is
   *  whether it can be held to an amount a week; `names` are the accounts
   *  listed as one is typed. */
  open(title: string, assistances: Assistance[], assist: Assist, editing: string | null = null, weekly = false, names: string[] = []): void {
    this.title = title;
    this.names = names;
    this.assistances = assistances;
    this.assist = assist;
    this.weekly = weekly;
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
    this.editing = found ? { to: found.to, picker: new WhenPicker(this.words.when, found.until ? new Date(found.until) : null), weekly: found.weekly ? String(found.weekly) : "" } : null;
    redrawKeepingFocus(this.sheet.body, () => this.draw());
    this.editing?.picker.reveal();
  }

  private save(): void {
    const editing = this.editing!;
    const typed = this.sheet.body.querySelector<HTMLInputElement>(".assist-edit [name=weekly]")?.value ?? editing.weekly;
    const weekly = this.weekly ? amountOf(typed) : null;
    if (weekly === undefined) return this.say(this.words.weeklyBad);
    void this.change(editing.to, editing.picker.value, weekly, true);
  }

  private draw(): void {
    const { words, sheet, editing } = this;
    const now = new Date();
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=to]")?.value ?? "";
    const typedWeekly = sheet.body.querySelector<HTMLInputElement>(".assist-new [name=weekly]")?.value ?? "";
    const rows = this.assistances
      .map((one) => {
        const until = one.until ? new Date(one.until) : null;
        const open = editing?.to === one.to;
        const weekly = this.weekly
          ? `<button class="assist-until${one.weekly ? "" : " none"}" type="button" data-change="${escape(one.to)}" title="${escape(words.weeklyHint)}"><span>${escape(one.weekly ? words.weeklyOf(wholeDollars(one.weekly)) : words.weeklyUnlimited)}</span></button>`
          : "";
        return `<li class="assist-row${open ? " editing" : ""}${this.weekly ? " weekly" : ""}">
          <span class="give-who">${glyph("person", 14)}<span>${escape(one.to)}</span></span>
          <button class="assist-until${until ? "" : " none"}" type="button" data-change="${escape(one.to)}" title="${escape(words.changeUntil)}">${glyph("history", 14)}<span>${escape(until ? words.moment.until(until, now) : words.when.none)}</span></button>
          ${weekly}
          <button class="text-button quiet-button" type="button" data-take-back="${escape(one.to)}">${escape(words.endAssist)}</button>
          ${open ? `<div class="assist-edit"><div class="assist-edit-when"></div>${this.weekly ? this.weeklyField(editing.weekly) : ""}<div class="assist-edit-acts"><button class="text-button quiet-button" type="button" data-cancel>${escape(words.cancel)}</button><button class="sheet-go" type="button" data-save="1">${escape(words.save)}</button></div></div>` : ""}
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
        // While one assistance is changed, the next one assisted waits.
        editing
          ? `<input type="hidden" name="to" value="${escape(typed)}">`
          : `<div class="assist-new${this.weekly ? " weekly" : ""}">
        <label class="field"><span class="field-label">${escape(words.assistWho)}</span>
          <input name="to" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="${escape(words.toHint)}" value="${escape(typed)}"></label>
        <div class="field assist-when"><span class="field-label">${escape(words.assistUntil)}</span></div>
        ${this.weekly ? this.weeklyField(typedWeekly) : ""}
      </div>`
      }
      <p class="sheet-note" role="alert" hidden></p>
      ${editing ? "" : `<footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.assistGo)}</button></footer>`}`;
    if (editing) sheet.body.querySelector(".assist-edit-when")?.append(editing.picker.element);
    else {
      sheet.body.querySelector(".assist-when")!.append(this.fresh.element);
      this.fresh.reveal();
      // Those it assists already are not listed again.
      const field = sheet.body.querySelector<HTMLInputElement>(".assist-new [name=to]");
      if (field) this.suggest.attach(field, this.names.filter((name) => !this.assistances.some((one) => one.to === name)));
    }
  }

  /** The weekly amount: dollars, empty for no limit. */
  private weeklyField(value: string): string {
    const { words } = this;
    return `<label class="field assist-weekly" title="${escape(words.weeklyHint)}"><span class="field-label">${escape(words.weekly)}</span>
      <span class="assist-weekly-input"><span class="assist-dollar">$</span><input name="weekly" inputmode="decimal" autocomplete="off" spellcheck="false" placeholder="${escape(words.weeklyNone)}" value="${escape(value)}"></span></label>`;
  }

  private async change(to: string, until: Date | null, weekly: number | null, assisting: boolean): Promise<void> {
    const { sheet, words } = this;
    if (sheet.busy) return;
    if (assisting && until && until <= new Date()) return this.say(words.untilPassed);
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    sheet.body.querySelector<HTMLElement>(".sheet-note")!.hidden = true;
    try {
      this.assistances = await this.assist(to, until, weekly, assisting);
      sheet.busy = false;
      const wasNew = assisting && this.editing?.to !== to;
      this.editing = null;
      if (wasNew) this.fresh = new WhenPicker(words.when, null);
      redrawKeepingFocus(sheet.body, () => this.draw());
      if (wasNew) {
        const field = sheet.body.querySelector<HTMLInputElement>("[name=to]")!;
        field.value = "";
        const amount = sheet.body.querySelector<HTMLInputElement>(".assist-new [name=weekly]");
        if (amount) amount.value = "";
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

/** A weekly amount as typed: dollars to the cent, none when empty, and
 *  undefined when it is no amount above zero. */
function amountOf(text: string): number | null | undefined {
  const plain = text.replace(/[$,，\s]/g, "").replace(/。/g, ".");
  if (!plain) return null;
  if (!/^\d+(\.\d+)?$/.test(plain)) return undefined;
  const amount = Math.round(Number(plain) * 100) / 100;
  return amount > 0 && Number.isFinite(amount) ? amount : undefined;
}
