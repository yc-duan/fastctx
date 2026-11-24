// Compacting a conversation idle a while, for one login (Issue #57): the
// reader turns it on for themselves, apart from everyone else who uses the
// login, and says after how long and from how much context. Each change is
// kept as it is made, as an application's settings are.

import { escape } from "../conversation/prose";
import { IDLE_DEFAULT, IDLE_MINUTES, IDLE_TOKENS_MOST, type IdleCompaction, type Login, type MachineLogins } from "../world/logins";
import type { AgentLoginsWords } from "./agent-logins-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";

/** What the sheet opens on: the login, and how its setting is kept,
 *  answering the machine's record as it is then. */
export interface IdleOpening {
  title: string;
  agent: "claude" | "codex";
  login: Login;
  set(setting: IdleCompaction): Promise<MachineLogins>;
}

export class LoginIdleSheet {
  private readonly sheet = new Sheet("device-sheet login-idle-sheet");
  private opening: IdleOpening | null = null;
  private setting: IdleCompaction = IDLE_DEFAULT;

  constructor(private readonly words: AgentLoginsWords) {
    this.sheet.body.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
      if (!button || button.disabled) return;
      if (button.hasAttribute("data-idle-on")) void this.keep({ ...this.setting, on: !this.setting.on });
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

  open(opening: IdleOpening): void {
    this.opening = opening;
    this.setting = opening.login.idle_compaction ?? IDLE_DEFAULT;
    this.sheet.body.innerHTML = "";
    this.draw();
    this.sheet.show();
  }

  private draw(): void {
    const { words, sheet, opening, setting } = this;
    if (!opening) return;
    const off = setting.on ? "" : " disabled";
    const each = words.idleTokensEach;
    const row = (name: string, said: string, end: string, quiet: boolean) =>
      `<label class="li-row${quiet ? " off" : ""}"><span class="li-what"><span class="li-name">${escape(name)}</span>${said ? `<span class="li-said">${escape(said)}</span>` : ""}</span>${end}</label>`;
    sheet.body.innerHTML = `
      ${sheet.head(opening.title, words.modelsClose)}
      <div class="li-rows">
        <div class="li-row">
          <span class="li-what"><span class="li-name">${escape(words.idleOn)}</span><span class="li-said">${escape(words.idleOnSaid)}</span></span>
          <button class="li-switch" type="button" role="switch" aria-checked="${setting.on}" aria-label="${escape(words.idleOn)}" data-idle-on><span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>
        </div>
        ${row(
          words.idleAfter,
          opening.agent === "claude" ? words.idleAfterSaid : "",
          `<span class="li-value"><input class="li-number" name="after" type="number" inputmode="numeric" min="${IDLE_MINUTES.least}" max="${IDLE_MINUTES.most}" step="1" value="${setting.after_minutes}"${off}><span class="li-unit">${escape(words.idleMinutes)}</span></span>`,
          !setting.on,
        )}
        ${row(
          words.idleMin,
          words.idleMinSaid,
          `<span class="li-value"><input class="li-number" name="min" type="number" inputmode="numeric" min="0" max="${IDLE_TOKENS_MOST / each}" step="1" value="${Math.round(setting.min_tokens / each)}"${off}><span class="li-unit">${escape(words.idleTokens)}</span></span>`,
          !setting.on,
        )}
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="button" data-done>${escape(words.modelsDone)}</button></footer>`;
  }

  /** A number typed: kept when it is within its bounds, said otherwise. */
  private typed(field: HTMLInputElement): void {
    const each = this.words.idleTokensEach;
    const [least, most] = field.name === "after" ? [IDLE_MINUTES.least, IDLE_MINUTES.most] : [0, IDLE_TOKENS_MOST / each];
    const value = Number(field.value);
    if (!Number.isInteger(value) || value < least || value > most) {
      this.say(this.words.idleRange(least, most), null);
      return;
    }
    void this.keep(field.name === "after" ? { ...this.setting, after_minutes: value } : { ...this.setting, min_tokens: value * each });
  }

  /** Keeps `setting` with the sheet held still, and draws the login as its
   *  machine answers it. */
  private async keep(setting: IdleCompaction): Promise<void> {
    const { sheet, opening } = this;
    if (!opening || sheet.busy) return;
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    sheet.body.querySelector<HTMLElement>(".sheet-note")!.hidden = true;
    try {
      const record = await opening.set(setting);
      const login = record[opening.agent].listed.find((one) => one.dir === opening.login.dir);
      this.setting = login?.idle_compaction ?? setting;
      sheet.busy = false;
      redrawKeepingFocus(sheet.body, () => this.draw());
    } catch (error) {
      sheet.busy = false;
      redrawKeepingFocus(sheet.body, () => this.draw());
      const why = error instanceof Error ? error.message : String(error);
      this.say(this.words.idleFailed(why), why);
    }
  }

  private say(text: string, detail: string | null): void {
    const note = this.sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    note.textContent = text;
    note.title = detail ?? "";
    note.hidden = false;
  }
}
