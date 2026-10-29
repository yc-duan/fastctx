// One login of an agent assisting others (`world-gui.md`, 账号、机器、共享与
// 协助): how much of its own five-hour window it lends at most and how many
// turns it runs at once, for the whole login; whom it assists now, each until when and, for Claude Code,
// up to how much a day and a week, changed or ended in place; then whom it
// assists next, the network's accounts that match listed under the name as
// it is typed. The hub ends an assistance by itself at its deadline; the
// login's machine holds each account to its allowances
// (`world-agent-logins.md`, 用量).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccountChange, AssistSeen, HubAccounts, Login } from "../world/accounts";
import type { LimitWindow } from "../world/agents";
import { wholeDollars } from "../world/money";
import type { AccessWords } from "./account-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { NameSuggest } from "./name-suggest";
import { WhenPicker } from "./when";
import { windowOf } from "./usage/model";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

/** What an assistance allows: until when (none: no end), what may be spent
 *  of the login a day and a week, in dollars (none: no limit), the share
 *  of the login's own five-hour window, in percent, past which it assists
 *  no one (none: no limit), and the most turns the login runs at once
 *  (none: as the vendor's rate limits allow). */
export interface Terms {
  until: Date | null;
  daily: number | null;
  weekly: number | null;
  fiveHour: number | null;
  turns: number | null;
}

/** One account the login assists, on its terms. */
export interface Assistance {
  to: string;
  until: string | null;
  daily: number | null;
  weekly: number | null;
  fiveHour: number | null;
  turns: number | null;
}

/** Lets the login assist `to` on `terms`, or no longer when there are
 *  none; answers the assistances as they are then. */
export type Assist = (to: string, terms: Terms | null) => Promise<Assistance[]>;

/** What the sheet opens on. */
export interface AssistOpening {
  title: string;
  assistances: Assistance[];
  assist: Assist;
  /** The assistance opened to be changed at once. */
  editing?: string | null;
  /** Whether amounts can be set: Claude Code keeps what it costs. */
  costs: boolean;
  /** The accounts listed as one is typed. */
  names: string[];
  /** The login's five-hour window as last read, none when it never was;
   *  absent where the page that opens the sheet does not read it. */
  window?: LimitWindow | null;
}

const dollars = (micro: number | undefined) => (micro ? micro / 1_000_000 : null);

/** The accounts `agent`'s login `home` on `machine` assists, among a hub's
 *  assistances; `home` is none for the agent's default login there. */
export function assistancesIn(assists: AssistSeen[], machine: string, agent: string, home: string | null): Assistance[] {
  return assists
    .filter((assist) => assist.machine === machine && assist.agent === agent && (assist.home ?? null) === home)
    .map((assist) => ({ to: assist.account, until: assist.until ?? null, daily: dollars(assist.daily), weekly: dollars(assist.weekly), fiveHour: assist.five_hour ?? null, turns: assist.turns ?? null }));
}

/** The terms an assistance the hub keeps was given on. */
export function termsOf(assist: AssistSeen): Terms {
  return { until: assist.until ? new Date(assist.until) : null, daily: dollars(assist.daily), weekly: dollars(assist.weekly), fiveHour: assist.five_hour ?? null, turns: assist.turns ?? null };
}

/** The five-hour window among a login's windows. */
export function fiveHourWindow(windows: LimitWindow[]): LimitWindow | null {
  return windowOf(windows, 300);
}

/** The accounts a login may have assisted, as its hub tells them: every
 *  one it knows of but the login's own. */
export function namesOn(hub: HubAccounts | null | undefined, login: Login): string[] {
  return (hub?.accounts ?? []).map((account) => account.name).filter((name) => name !== login.account);
}

/** The change that has the login assist `to` on `terms`, or no longer. */
export function assistChange(machine: string, agent: string, home: string | null, to: string, terms: Terms | null): AccountChange {
  return {
    change: "assist",
    machine,
    to,
    agent,
    ...(home ? { home } : {}),
    assisting: !!terms,
    ...(terms?.until ? { until: terms.until.toISOString() } : {}),
    ...(terms?.weekly ? { weekly: Math.round(terms.weekly * 1_000_000) } : {}),
    ...(terms?.daily ? { daily: Math.round(terms.daily * 1_000_000) } : {}),
    ...(terms?.fiveHour ? { five_hour: terms.fiveHour } : {}),
    ...(terms?.turns ? { turns: terms.turns } : {}),
  };
}

/** What is being changed: one assistance, with the picker its deadline is
 *  changed in and its amounts as they were, or the login's five-hour share,
 *  or its turns at once. */
type Editing =
  | { kind: "one"; to: string; picker: WhenPicker; daily: string; weekly: string }
  | { kind: "window"; share: string }
  | { kind: "turns"; turns: string }
  | null;

export class AssistSheet {
  private readonly sheet = new Sheet("device-sheet assist-sheet");
  private opening: AssistOpening = { title: "", assistances: [], assist: async () => [], costs: false, names: [] };
  private assistances: Assistance[] = [];
  /** The share of its five-hour window the login lends at most, which every
   *  assistance it gives carries; kept here for the next one while it
   *  assists no one. */
  private fiveHour: number | null = null;
  /** The most turns the login runs at once, kept the same way. */
  private turns: number | null = null;
  /** The deadline for the next one assisted. */
  private fresh: WhenPicker;
  private editing: Editing = null;
  private readonly suggest: NameSuggest;

  constructor(private readonly words: AccessWords) {
    this.fresh = new WhenPicker(words.when, null);
    this.suggest = new NameSuggest(words.assistNames);
    this.sheet.body.tabIndex = -1;
    this.sheet.body.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
      if (!button || button.disabled) return;
      if (button.dataset.takeBack) void this.change(button.dataset.takeBack, null);
      else if (button.dataset.change) this.edit(button.dataset.change);
      else if (button.hasAttribute("data-window")) this.editWindow();
      else if (button.hasAttribute("data-turns")) this.editTurns();
      else if (button.dataset.save) this.save();
      else if (button.hasAttribute("data-cancel")) this.edit(null);
    });
    // The Fable limit under the weekly field follows what is typed there.
    this.sheet.body.addEventListener("input", (event) => {
      const input = event.target;
      if (!(input instanceof HTMLInputElement) || input.name !== "weekly") return;
      const line = input.closest("label")?.querySelector<HTMLElement>("[data-fable]");
      if (line) line.textContent = this.fableOf(input.value);
    });
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      if (this.editing) return this.save();
      const to = this.sheet.body.querySelector<HTMLInputElement>("[name=to]")!.value.trim();
      const amounts = this.amounts(".assist-new");
      if (amounts && to) void this.change(to, { until: this.fresh.value, ...amounts, fiveHour: this.fiveHour, turns: this.turns });
    });
  }

  /** Opens the sheet on whom the login assists now. */
  open(opening: AssistOpening): void {
    this.opening = opening;
    this.assistances = opening.assistances;
    this.fiveHour = shareOf(opening.assistances);
    this.turns = turnsOf(opening.assistances);
    this.fresh = new WhenPicker(this.words.when, null);
    this.editing = null;
    this.sheet.body.innerHTML = "";
    const editing = opening.editing;
    if (editing && opening.assistances.some((one) => one.to === editing)) this.edit(editing);
    else this.draw();
    this.sheet.show();
    const opened = this.editing as Editing;
    if (opened?.kind === "one") {
      opened.picker.reveal();
      // Nothing is typed first: the sheet itself holds the focus, and no
      // button wears a ring it was never pressed for.
      this.sheet.body.focus({ preventScroll: true });
    } else this.sheet.body.querySelector<HTMLInputElement>("[name=to]")?.focus();
  }

  private edit(to: string | null): void {
    const found = this.assistances.find((one) => one.to === to);
    this.editing = found
      ? { kind: "one", to: found.to, picker: new WhenPicker(this.words.when, found.until ? new Date(found.until) : null), daily: found.daily ? String(found.daily) : "", weekly: found.weekly ? String(found.weekly) : "" }
      : null;
    redrawKeepingFocus(this.sheet.body, () => this.draw());
    if (this.editing?.kind === "one") this.editing.picker.reveal();
  }

  private editWindow(): void {
    this.editing = { kind: "window", share: this.fiveHour ? String(this.fiveHour) : "" };
    redrawKeepingFocus(this.sheet.body, () => this.draw());
    const field = this.sheet.body.querySelector<HTMLInputElement>("[name=five-hour]");
    field?.focus();
    field?.select();
  }

  private editTurns(): void {
    this.editing = { kind: "turns", turns: this.turns ? String(this.turns) : "" };
    redrawKeepingFocus(this.sheet.body, () => this.draw());
    const field = this.sheet.body.querySelector<HTMLInputElement>("[name=turns]");
    field?.focus();
    field?.select();
  }

  private save(): void {
    const editing = this.editing;
    if (editing?.kind === "window") {
      const share = shareTyped(this.sheet.body.querySelector<HTMLInputElement>("[name=five-hour]")?.value ?? editing.share);
      if (share === undefined) return this.say(this.words.fiveHourBad);
      return void this.setLogin({ fiveHour: share });
    }
    if (editing?.kind === "turns") {
      const turns = turnsTyped(this.sheet.body.querySelector<HTMLInputElement>("[name=turns]")?.value ?? editing.turns);
      if (turns === undefined) return this.say(this.words.turnsBad);
      return void this.setLogin({ turns });
    }
    if (editing?.kind !== "one") return;
    const amounts = this.amounts(".assist-edit");
    if (amounts) void this.change(editing.to, { until: editing.picker.value, ...amounts, fiveHour: this.fiveHour, turns: this.turns });
  }

  /** The amounts typed under `within`, or none when one is not an amount,
   *  said beside the sheet's foot. */
  private amounts(within: string): { daily: number | null; weekly: number | null } | null {
    if (!this.opening.costs) return { daily: null, weekly: null };
    const typed = (name: string) => this.sheet.body.querySelector<HTMLInputElement>(`${within} [name=${name}]`)?.value ?? "";
    const daily = amountOf(typed("daily"));
    const weekly = amountOf(typed("weekly"));
    if (daily === undefined || weekly === undefined) {
      this.say(daily === undefined ? this.words.dailyBad : this.words.weeklyBad);
      return null;
    }
    return { daily, weekly };
  }

  private draw(): void {
    const { words, sheet, editing, opening } = this;
    const now = new Date();
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=to]")?.value ?? "";
    const typedDaily = sheet.body.querySelector<HTMLInputElement>(".assist-new [name=daily]")?.value ?? "";
    const typedWeekly = sheet.body.querySelector<HTMLInputElement>(".assist-new [name=weekly]")?.value ?? "";
    const rows = this.assistances
      .map((one) => {
        const until = one.until ? new Date(one.until) : null;
        const open = editing?.kind === "one" && editing.to === one.to;
        return `<li class="assist-row${open ? " editing" : ""}${opening.costs ? " costs" : ""}">
          <span class="give-who">${glyph("person", 14)}<span>${escape(one.to)}</span></span>
          <button class="assist-until${until ? "" : " none"}" type="button" data-change="${escape(one.to)}" title="${escape(words.changeUntil)}">${glyph("history", 14)}<span>${escape(until ? words.moment.until(until, now) : words.when.none)}</span></button>
          ${opening.costs ? this.amountsPill(one) : ""}
          <button class="text-button quiet-button" type="button" data-take-back="${escape(one.to)}">${escape(words.endAssist)}</button>
          ${
            open
              ? `<div class="assist-edit"><div class="assist-edit-when"></div>${opening.costs ? this.amountFields(editing.daily, editing.weekly) : ""}${this.acts()}</div>`
              : ""
          }
        </li>`;
      })
      .join("");
    sheet.body.innerHTML = `
      ${sheet.head(opening.title, words.close)}
      <p class="confirm-what">${escape(words.assistWhat)}</p>
      <p class="field-hint give-sees">${glyph("shield", 14)}<span>${escape(words.assistSees)}</span></p>
      ${this.windowLine(now)}
      ${this.turnsLine()}
      <section class="give-list">
        <h3 class="field-label">${escape(words.assisting)}</h3>
        ${rows ? `<ul class="assist-rows">${rows}</ul>` : `<p class="field-hint">${escape(words.nobodyYet)}</p>`}
      </section>
      ${
        // While anything is changed, the next one assisted waits.
        editing
          ? `<input type="hidden" name="to" value="${escape(typed)}">`
          : `<div class="assist-new">
        <label class="field"><span class="field-label">${escape(words.assistWho)}</span>
          <input name="to" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="${escape(words.toHint)}" value="${escape(typed)}"></label>
        <div class="field assist-when"><span class="field-label">${escape(words.assistUntil)}</span></div>
        ${opening.costs ? this.amountFields(typedDaily, typedWeekly) : ""}
      </div>`
      }
      <p class="sheet-note" role="alert" hidden></p>
      ${editing ? "" : `<footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.assistGo)}</button></footer>`}`;
    if (editing?.kind === "one") sheet.body.querySelector(".assist-edit-when")?.append(editing.picker.element);
    else if (!editing) {
      sheet.body.querySelector(".assist-when")!.append(this.fresh.element);
      this.fresh.reveal();
      // Those it assists already are not listed again.
      const field = sheet.body.querySelector<HTMLInputElement>(".assist-new [name=to]");
      if (field) this.suggest.attach(field, opening.names.filter((name) => !this.assistances.some((one) => one.to === name)));
    }
  }

  /** The login's five-hour window as a bar, its share marked on it, and
   *  the share to press and change. */
  private windowLine(now: Date): string {
    const { words, editing, fiveHour } = this;
    const window = this.opening.window ?? null;
    const drawn = this.opening.window !== undefined;
    const used = window && !(window.resets_at && new Date(window.resets_at) <= now) ? window.used_tenths / 10 : null;
    const read = !window
      ? words.fiveHourUnread
      : used === null
        ? words.fiveHourReset
        : words.fiveHourRead(Math.round(used), window.resets_at ? words.moment.short(new Date(window.resets_at), now) : null);
    const over = used !== null && fiveHour !== null && used >= fiveHour;
    const open = editing?.kind === "window";
    return `<section class="assist-window-line${open ? " editing" : ""}">
      <span class="field-label">${escape(words.fiveHour)}</span>
      ${
        drawn
          ? `<span class="assist-window${window ? "" : " unread"}${over ? " over" : ""}" title="${escape(read)}">
        <span class="assist-window-used" style="width:${Math.min(100, used ?? 0).toFixed(1)}%"></span>
        ${fiveHour !== null ? `<span class="assist-window-share" style="left:${fiveHour}%"></span>` : ""}
      </span>`
          : "<span></span>"
      }
      <button class="assist-until${fiveHour === null ? " none" : ""}" type="button" data-window title="${escape(words.fiveHourHint)}"><span>${escape(fiveHour === null ? words.fiveHourNone : words.fiveHourOf(fiveHour))}</span></button>
      ${
        open
          ? `<div class="assist-edit"><label class="field assist-amount" title="${escape(words.fiveHourHint)}">
          <span class="assist-amount-input"><input name="five-hour" inputmode="numeric" autocomplete="off" spellcheck="false" aria-label="${escape(words.fiveHour)}" placeholder="${escape(words.fiveHourNone)}" value="${escape(editing.share)}"><span class="assist-unit">%</span></span></label>${this.acts()}</div>`
          : ""
      }
    </section>`;
  }

  /** The most turns the login runs at once, pressed to change. */
  private turnsLine(): string {
    const { words, editing, turns } = this;
    const open = editing?.kind === "turns";
    return `<section class="assist-window-line assist-turns-line${open ? " editing" : ""}">
      <span class="field-label">${escape(words.turns)}</span>
      <span></span>
      <button class="assist-until${turns === null ? " none" : ""}" type="button" data-turns title="${escape(words.turnsHint)}"><span>${escape(turns === null ? words.turnsNone : words.turnsOf(turns))}</span></button>
      ${
        open
          ? `<div class="assist-edit"><label class="field assist-amount" title="${escape(words.turnsHint)}">
          <span class="assist-amount-input"><input name="turns" inputmode="numeric" autocomplete="off" spellcheck="false" aria-label="${escape(words.turns)}" placeholder="${escape(words.turnsNone)}" value="${escape(editing.turns)}"><span class="assist-unit">${escape(words.turnsUnit)}</span></span></label>${this.acts()}</div>`
          : ""
      }
    </section>`;
  }

  /** What one may spend a day and a week, pressed to change. */
  private amountsPill(one: Assistance): string {
    const { words } = this;
    const said = [one.daily ? words.dailyOf(wholeDollars(one.daily)) : "", one.weekly ? words.weeklyOf(wholeDollars(one.weekly)) : ""].filter(Boolean);
    const inside = said.length ? said.map((part) => `<span>${escape(part)}</span>`).join("") : `<span>${escape(words.noAllowance)}</span>`;
    return `<button class="assist-until assist-amounts-pill${said.length ? "" : " none"}" type="button" data-change="${escape(one.to)}" title="${escape(`${words.dailyHint}\n${words.weeklyHint}`)}">${inside}</button>`;
  }

  /** The daily and weekly amounts side by side: dollars, empty for no
   *  limit. */
  private amountFields(daily: string, weekly: string): string {
    const field = (name: string, label: string, hint: string, value: string, under = "") =>
      `<label class="field assist-amount" title="${escape(hint)}"><span class="field-label">${escape(label)}</span>
      <span class="assist-amount-input"><span class="assist-unit">$</span><input name="${name}" inputmode="decimal" autocomplete="off" spellcheck="false" placeholder="${escape(this.words.weeklyNone)}" value="${escape(value)}"></span>${under}</label>`;
    const { words } = this;
    // Kept in its place while empty, so typing the first digit moves nothing.
    const fable = `<span class="field-hint assist-fable" data-fable title="${escape(words.fableHint)}">${escape(this.fableOf(weekly))}</span>`;
    return `<div class="assist-amounts">${field("daily", words.daily, words.dailyHint, daily)}${field("weekly", words.weekly, words.weeklyHint, weekly, fable)}</div>`;
  }

  /** The Fable limit a weekly amount as typed brings: half of it, to the
   *  cent; nothing while it is no amount. */
  private fableOf(weekly: string): string {
    const amount = amountOf(weekly);
    if (typeof amount !== "number") return "";
    const half = Math.round(amount * 50) / 100;
    return this.words.fableOf(`$${Number.isInteger(half) ? half : half.toFixed(2)}`);
  }

  private acts(): string {
    const { words } = this;
    return `<div class="assist-edit-acts"><button class="text-button quiet-button" type="button" data-cancel>${escape(words.cancel)}</button><button class="sheet-go" type="button" data-save="1">${escape(words.save)}</button></div>`;
  }

  /** Has the login assist `to` on `terms`, or no longer. */
  private async change(to: string, terms: Terms | null): Promise<void> {
    if (terms?.until && terms.until <= new Date()) return this.say(this.words.untilPassed);
    const wasNew = !!terms && !(this.editing?.kind === "one" && this.editing.to === to);
    await this.run(() => this.opening.assist(to, terms), wasNew);
  }

  /** Sets what holds for the whole login, the share of its five-hour
   *  window it lends at most or the turns it runs at once, on every
   *  assistance it gives; while it gives none, for the next one. */
  private async setLogin(change: { fiveHour?: number | null; turns?: number | null }): Promise<void> {
    const given = this.assistances;
    if (!given.length) {
      if (change.fiveHour !== undefined) this.fiveHour = change.fiveHour;
      if (change.turns !== undefined) this.turns = change.turns;
      this.edit(null);
      return;
    }
    await this.run(async () => {
      let now = given;
      for (const one of given) {
        const terms = {
          until: one.until ? new Date(one.until) : null,
          daily: one.daily,
          weekly: one.weekly,
          fiveHour: change.fiveHour !== undefined ? change.fiveHour : one.fiveHour,
          turns: change.turns !== undefined ? change.turns : one.turns,
        };
        now = await this.opening.assist(one.to, terms);
      }
      return now;
    }, false);
  }

  /** Does `work` with the sheet held still, and draws what it answers. */
  private async run(work: () => Promise<Assistance[]>, wasNew: boolean): Promise<void> {
    const { sheet, words } = this;
    if (sheet.busy) return;
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    sheet.body.querySelector<HTMLElement>(".sheet-note")!.hidden = true;
    try {
      this.assistances = await work();
      this.fiveHour = shareOf(this.assistances) ?? (this.assistances.length ? null : this.fiveHour);
      this.turns = turnsOf(this.assistances) ?? (this.assistances.length ? null : this.turns);
      sheet.busy = false;
      this.editing = null;
      if (wasNew) this.fresh = new WhenPicker(words.when, null);
      redrawKeepingFocus(sheet.body, () => this.draw());
      if (wasNew) {
        const field = sheet.body.querySelector<HTMLInputElement>("[name=to]")!;
        field.value = "";
        for (const amount of sheet.body.querySelectorAll<HTMLInputElement>(".assist-new .assist-amount input")) amount.value = "";
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

/** The share the login's assistances carry; they carry one alike. */
function shareOf(assistances: Assistance[]): number | null {
  return assistances.find((one) => one.fiveHour)?.fiveHour ?? null;
}

/** The turns at once the login's assistances carry; they carry one alike. */
function turnsOf(assistances: Assistance[]): number | null {
  return assistances.find((one) => one.turns)?.turns ?? null;
}

/** Turns as typed: a whole number from 1, none when empty, and undefined
 *  otherwise. */
function turnsTyped(text: string): number | null | undefined {
  const plain = text.replace(/\s/g, "");
  if (!plain) return null;
  const turns = Number(plain);
  return /^\d+$/.test(plain) && turns >= 1 && turns <= 1000 ? turns : undefined;
}

/** An amount as typed: dollars to the cent, none when empty, and undefined
 *  when it is no amount above zero. */
function amountOf(text: string): number | null | undefined {
  const plain = text.replace(/[$,，\s]/g, "").replace(/。/g, ".");
  if (!plain) return null;
  if (!/^\d+(\.\d+)?$/.test(plain)) return undefined;
  const amount = Math.round(Number(plain) * 100) / 100;
  return amount > 0 && Number.isFinite(amount) ? amount : undefined;
}

/** A share as typed: a whole percent from 1 to 100, none when empty, and
 *  undefined otherwise. */
function shareTyped(text: string): number | null | undefined {
  const plain = text.replace(/[%％\s]/g, "");
  if (!plain) return null;
  const share = Number(plain);
  return /^\d+$/.test(plain) && share >= 1 && share <= 100 ? share : undefined;
}
