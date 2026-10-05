// Bringing a device into the World and sending one away
// (`world-network.md`): an invitation, a one-use code shown as text and as
// a QR code for a phone to scan; revoking a member's key; this machine
// leaving. Revoking and leaving cannot be taken back, so each says what it
// does and waits for a second press before it does it.

import qrcode from "qrcode-generator";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { Confirm, Sheet } from "./confirm";
import type { DeviceWords } from "./words";

export interface DevicesOptions {
  words: DeviceWords;
  invite(name: string | null, hours: number, device: boolean): Promise<string>;
  revoke(name: string, reason: string): Promise<void>;
  leave(): Promise<void>;
  /** A moment as the interface says it. */
  when(at: Date): string;
  /** Something changed in the World: ask again. */
  changed(): void;
}

const validity = [1, 24, 72];

/** A QR code as an SVG of dark squares on light, whatever the theme:
 *  scanners read dark on light, and the code is never drawn by a pen. */
export function qrSvg(text: string, label: string): string {
  const code = qrcode(0, "L");
  code.addData(text, "Byte");
  code.make();
  const count = code.getModuleCount();
  const quiet = 4;
  const size = count + quiet * 2;
  let path = "";
  for (let row = 0; row < count; row += 1) {
    for (let col = 0; col < count; col += 1) {
      if (code.isDark(row, col)) path += `M${col + quiet} ${row + quiet}h1v1h-1z`;
    }
  }
  return `<svg class="qr" viewBox="0 0 ${size} ${size}" shape-rendering="crispEdges" role="img" aria-label="${escape(label)}"><rect width="${size}" height="${size}" fill="#fff"/><path d="${path}" fill="#111"/></svg>`;
}

export class Devices {
  private readonly invitation = new Sheet("device-sheet");
  private readonly confirm: Confirm;
  private hours = 24;
  /** Whether the invitation is for a phone rather than a computer. */
  private device = false;

  constructor(private readonly options: DevicesOptions) {
    this.confirm = new Confirm(options.words);
    this.invitation.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const kind = target.closest<HTMLButtonElement>("[data-device]");
      if (kind) {
        // The sheet is drawn again: what the invitation is for changes what
        // it says.
        const name = this.invitation.body.querySelector<HTMLInputElement>("[name=name]")?.value ?? "";
        this.device = kind.dataset.device === "true";
        this.invite();
        const input = this.invitation.body.querySelector<HTMLInputElement>("[name=name]");
        if (input) input.value = name;
        return;
      }
      const hours = target.closest<HTMLButtonElement>("[data-hours]");
      if (hours) {
        this.hours = Number(hours.dataset.hours);
        for (const button of this.invitation.body.querySelectorAll<HTMLButtonElement>("[data-hours]")) {
          const chosen = Number(button.dataset.hours) === this.hours;
          button.classList.toggle("chosen", chosen);
          button.setAttribute("aria-checked", String(chosen));
        }
      }
      if (target.closest(".copy-code")) {
        const code = this.invitation.body.querySelector<HTMLElement>(".invite-code")?.textContent ?? "";
        void navigator.clipboard.writeText(code).then(() => {
          const copy = this.invitation.body.querySelector<HTMLElement>(".copy-code span");
          if (copy) copy.textContent = options.words.copied;
        });
      }
      if (target.closest(".make-again")) this.invite();
    });
    this.invitation.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.make();
    });
  }

  /** The sheet that makes an invitation. */
  invite(): void {
    const { words } = this.options;
    this.invitation.body.innerHTML = `
      ${this.invitation.head(words.inviteTitle, words.close)}
      <div class="field"><span class="field-label">${escape(words.inviteWhat)}</span>
        <div class="choice-options" role="radiogroup" aria-label="${escape(words.inviteWhat)}">${([false, true] as const)
          .map(
            (device) =>
              `<button type="button" class="choice${device === this.device ? " chosen" : ""}" role="radio" aria-checked="${device === this.device}" data-device="${device}">${escape(device ? words.aPhone : words.aComputer)}</button>`,
          )
          .join("")}</div>
        <span class="field-hint">${escape(this.device ? words.phoneHint : words.computerHint)}</span></div>
      <label class="field"><span class="field-label">${escape(words.inviteName)}</span>
        <input name="name" autocomplete="off" spellcheck="false" class="mono" pattern="[a-z][a-z0-9-]{0,31}">
        <span class="field-hint">${escape(words.inviteNameHint)}</span></label>
      <div class="field"><span class="field-label">${escape(words.valid)}</span>
        <div class="choice-options" role="radiogroup" aria-label="${escape(words.valid)}">${validity
          .map(
            (hours) =>
              `<button type="button" class="choice${hours === this.hours ? " chosen" : ""}" role="radio" aria-checked="${hours === this.hours}" data-hours="${hours}">${escape(words.hours(hours))}</button>`,
          )
          .join("")}</div></div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.make)}</button></footer>`;
    this.invitation.show();
    this.invitation.body.querySelector<HTMLInputElement>("[name=name]")?.focus();
  }

  private async make(): Promise<void> {
    const { words } = this.options;
    if (this.invitation.busy) return;
    const name = this.invitation.body.querySelector<HTMLInputElement>("[name=name]")?.value.trim() || null;
    const go = this.invitation.body.querySelector<HTMLButtonElement>(".sheet-go");
    const note = this.invitation.body.querySelector<HTMLElement>(".sheet-note");
    if (!go || !note) return;
    this.invitation.busy = true;
    go.disabled = true;
    go.textContent = words.making;
    try {
      const code = await this.options.invite(name, this.hours, this.device);
      const until = this.options.when(new Date(Date.now() + this.hours * 3_600_000));
      this.invitation.body.innerHTML = `
        ${this.invitation.head(words.inviteTitle, words.close)}
        <div class="invitation">
          <div class="qr-frame">${qrSvg(code, words.qr)}</div>
          <div class="invitation-text">
            <p class="field-label">${escape(words.code)}</p>
            <p class="invite-code mono">${escape(code)}</p>
            <p class="field-hint">${escape(words.until(until))}</p>
            <button class="text-button copy-code" type="button">${glyph("copy", 16)}<span>${escape(words.copy)}</span></button>
          </div>
        </div>
        <section class="how-to"><h3 class="field-label">${escape(words.howTo)}</h3><ol>${(this.device ? words.phoneSteps : words.howToSteps).map((step) => `<li>${escape(step)}</li>`).join("")}</ol></section>
        <footer class="sheet-foot"><span></span><button class="text-button make-again" type="button">${escape(words.again)}</button></footer>`;
    } catch (error) {
      note.textContent = words.failed(String(error));
      note.hidden = false;
      go.disabled = false;
      go.textContent = words.make;
    } finally {
      this.invitation.busy = false;
    }
  }

  /** Asks once more before revoking `name`. */
  revoke(name: string): void {
    const { words } = this.options;
    this.ask(
      words.revokeTitle(name),
      words.revokeWhat(name),
      `<label class="field"><span class="field-label">${escape(words.reason)}</span><input name="reason" autocomplete="off" value="${escape(words.reasonDefault)}"></label>`,
      words.revokeGo,
      () => this.options.revoke(name, this.confirm.body.querySelector<HTMLInputElement>("[name=reason]")?.value.trim() || words.reasonDefault),
    );
  }

  /** Asks once more before this machine leaves the World. */
  leave(): void {
    const { words } = this.options;
    this.ask(words.leaveTitle, words.leaveWhat, "", words.leaveGo, () => this.options.leave());
  }

  private ask(title: string, what: string, fields: string, go: string, act: () => Promise<void>): void {
    this.confirm.ask({ title, what, fields, go, act, done: () => this.options.changed() });
  }
}
