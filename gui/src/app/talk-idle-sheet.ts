// Compacting one conversation idle a while (Issue #57), as the user sets it
// for that conversation alone: whether, after how long and from how much
// context, each part following its agent's setting until it is set here.
// Each change is kept as it is made, as an application's settings are.

import { escape } from "../conversation/prose";
import { IDLE_MINUTES, IDLE_TOKENS_MOST, type IdleChoice, type IdleCompaction } from "../world/logins";
import type { ResidentNow } from "../world/world";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import type { TalkMenuWords } from "./talk-menu-words";

export interface IdleSheetOpening {
  r: ResidentNow;
  /** Its agent's setting for the account it works for, once read; none
   *  where it could not be. */
  login: Promise<IdleCompaction | null>;
  keep(choice: IdleChoice): void;
}

export class TalkIdleSheet {
  private readonly sheet = new Sheet("device-sheet talk-idle-sheet");
  private opening: IdleSheetOpening | null = null;
  private choice: IdleChoice = {};
  private login: IdleCompaction | null | undefined;

  constructor(private readonly words: TalkMenuWords) {
    this.sheet.body.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
      if (!button || button.disabled) return;
      const on = button.dataset.idleOn;
      if (on !== undefined) this.keep({ ...this.without("on"), ...(on === "follow" ? {} : { on: on === "on" }) });
      else if (button.hasAttribute("data-idle-reset")) this.keep({});
      else if (button.hasAttribute("data-done")) this.sheet.close();
    });
    this.sheet.body.addEventListener("change", (event) => {
      const field = event.target as HTMLInputElement;
      if (field.name === "after" || field.name === "min") this.typed(field);
    });
    this.sheet.body.addEventListener("keydown", (event) => {
      const field = event.target as HTMLInputElement;
      if (event.key === "Enter" && (field.name === "after" || field.name === "min")) field.blur();
    });
  }

  open(opening: IdleSheetOpening): void {
    this.opening = opening;
    this.choice = { ...(opening.r.about.idle_compaction ?? {}) };
    this.login = undefined;
    this.sheet.body.innerHTML = "";
    this.draw();
    this.sheet.show();
    void opening.login.then(
      (login) => {
        if (this.opening !== opening) return;
        this.login = login;
        redrawKeepingFocus(this.sheet.body, () => this.draw());
      },
      () => {
        if (this.opening !== opening) return;
        this.login = null;
        redrawKeepingFocus(this.sheet.body, () => this.draw());
      },
    );
  }

  private without(part: keyof IdleChoice): IdleChoice {
    const { [part]: _, ...rest } = this.choice;
    return rest;
  }

  private draw(): void {
    const { words, sheet, opening, choice, login } = this;
    if (!opening) return;
    const each = words.idleTokensEach;
    // What its agent's setting says, beside each part.
    const follows = (value: string | null) =>
      login === undefined ? "" : login === null ? words.idleFollowUnknown : words.idleFollows(value ?? "");
    const onNow = choice.on === undefined ? "follow" : choice.on ? "on" : "off";
    const pick = (value: string, label: string) =>
      `<button class="choice${onNow === value ? " chosen" : ""}" type="button" aria-pressed="${onNow === value}" data-idle-on="${value}">${escape(label)}</button>`;
    const number = (name: string, value: number | undefined, follow: number | undefined, least: number, most: number, unit: string) =>
      `<span class="li-value"><input class="li-number" name="${name}" type="number" inputmode="numeric" min="${least}" max="${most}" step="1" value="${value ?? ""}" placeholder="${follow ?? ""}" aria-label="${escape(unit)}"><span class="li-unit">${escape(unit)}</span></span>`;
    const row = (name: string, said: string, end: string) =>
      `<div class="li-row"><span class="li-what"><span class="li-name">${escape(name)}</span>${said ? `<span class="li-said">${escape(said)}</span>` : ""}</span>${end}</div>`;
    const loginOn = login ? (login.on ? words.idleOn : words.idleOff) : null;
    const loginAfter = login ? `${login.after_minutes} ${words.idleMinutes}` : null;
    const loginMin = login ? `${tokensIn(login.min_tokens, each)} ${words.idleTokens}` : null;
    const set = choice.on !== undefined || choice.after_minutes !== undefined || choice.min_tokens !== undefined;
    // A number being typed, not kept yet, stays as typed when the sheet is
    // drawn again, as when its agent's setting comes.
    const typing = document.activeElement instanceof HTMLInputElement && sheet.body.contains(document.activeElement) ? document.activeElement : null;
    const typed = typing ? { name: typing.name, value: typing.value } : null;
    sheet.body.innerHTML = `
      ${sheet.head(words.idle, words.idleClose)}
      <p class="li-intro">${escape(words.idleWhat)}</p>
      <div class="li-rows">
        ${row(words.idle, follows(loginOn), `<span class="ti-choices">${pick("follow", words.idleFollowShort)}${pick("on", words.idleOn)}${pick("off", words.idleOff)}</span>`)}
        ${row(words.idleAfter, follows(loginAfter), number("after", choice.after_minutes, login?.after_minutes, IDLE_MINUTES.least, IDLE_MINUTES.most, words.idleMinutes))}
        ${row(words.idleMin, follows(loginMin), number("min", choice.min_tokens === undefined ? undefined : tokensIn(choice.min_tokens, each), login ? tokensIn(login.min_tokens, each) : undefined, 0, IDLE_TOKENS_MOST / each, words.idleTokens))}
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot">
        ${set ? `<button class="text-button quiet-button" type="button" data-idle-reset>${escape(words.idleReset)}</button>` : "<span></span>"}
        <button class="sheet-go" type="button" data-done>${escape(words.idleDone)}</button>
      </footer>`;
    if (typed) {
      const field = sheet.body.querySelector<HTMLInputElement>(`[name="${typed.name}"]`);
      if (field) field.value = typed.value;
    }
  }

  /** A number typed: none follows its agent; one within its bounds is kept,
   *  one outside them said. */
  private typed(field: HTMLInputElement): void {
    const each = this.words.idleTokensEach;
    const part = field.name === "after" ? "after_minutes" : "min_tokens";
    if (field.value.trim() === "") {
      this.keep(this.without(part));
      return;
    }
    const [least, most] = part === "after_minutes" ? [IDLE_MINUTES.least, IDLE_MINUTES.most] : [0, IDLE_TOKENS_MOST / each];
    const value = Number(field.value);
    if (!Number.isInteger(value) || value < least || value > most) {
      const note = this.sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      note.textContent = this.words.idleRange(least, most);
      note.hidden = false;
      return;
    }
    this.keep({ ...this.choice, [part]: part === "after_minutes" ? value : value * each });
  }

  private keep(choice: IdleChoice): void {
    if (!this.opening) return;
    this.choice = choice;
    this.opening.keep(choice);
    redrawKeepingFocus(this.sheet.body, () => this.draw());
  }
}

/** Tokens in the sheet's unit, to one decimal at most. */
function tokensIn(tokens: number, each: number): number {
  return Number((tokens / each).toFixed(1));
}
