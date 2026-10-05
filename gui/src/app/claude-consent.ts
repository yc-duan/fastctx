// The user's word on whether Claude on a machine may work without asking
// for approval. Claude Code runs the World's sessions in its bypass mode
// only once the user has accepted it on that machine; this sheet says what
// that means, as plainly as Claude Code's own dialog, and only the user's
// press writes it there.

import { escape } from "../conversation/prose";
import { Sheet } from "./confirm";
import type { ConsentWords } from "./words";

export type WriteConsent = (machine: string, accept: boolean) => Promise<{ accepted: boolean; changed: boolean }>;

export class ClaudeConsent {
  private readonly sheet = new Sheet("device-sheet confirm-sheet consent-sheet");

  constructor(
    private readonly words: ConsentWords,
    private readonly write: WriteConsent,
  ) {}

  /** Asks, and settles true once Claude Code on `machine` records the
   *  user's consent; false when the sheet is closed without it. `go` names
   *  the button, as what pressing it will do there. */
  allow(machine: string, go: string): Promise<boolean> {
    return this.ask(this.words.title(machine), this.words.what(machine), go, () => this.write(machine, true));
  }

  /** Asks before taking the consent back; true once it is withdrawn. */
  withdraw(machine: string): Promise<boolean> {
    return this.ask(this.words.withdrawTitle(machine), this.words.withdrawWhat, this.words.withdrawGo, () => this.write(machine, false));
  }

  private ask(title: string, what: string, go: string, act: () => Promise<unknown>): Promise<boolean> {
    const { words, sheet } = this;
    return new Promise((settle) => {
      let done = false;
      sheet.body.innerHTML = `
        ${sheet.head(title, words.close)}
        <p class="confirm-what">${escape(what)}</p>
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(go)}</button></footer>`;
      sheet.body.onsubmit = (event) => {
        event.preventDefault();
        if (sheet.busy) return;
        const button = sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
        const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
        sheet.busy = true;
        button.disabled = true;
        button.textContent = words.working;
        act()
          .then(() => {
            done = true;
            sheet.busy = false;
            sheet.close();
          })
          .catch((error: unknown) => {
            sheet.busy = false;
            button.disabled = false;
            button.textContent = go;
            note.textContent = words.failed(error instanceof Error ? error.message : String(error));
            note.hidden = false;
          });
      };
      sheet.dialog.addEventListener("close", () => settle(done), { once: true });
      sheet.show();
    });
  }
}
