// The user's word on whether Claude on a machine may work without asking
// for approval. Claude Code runs the World's sessions in its bypass mode
// only once the user has accepted it in that login on that machine; this
// sheet says what that means, as plainly as Claude Code's own dialog, and
// only the user's press writes it there.

import { escape } from "../conversation/prose";
import { Sheet } from "./confirm";
import type { ConsentWords } from "./words";

/** Writes the word in Claude's home `home` on `machine`, or its default
 *  one where null. */
export type WriteConsent = (machine: string, accept: boolean, home: string | null) => Promise<{ accepted: boolean; changed: boolean }>;

/** Which of Claude's logins on the machine: its home's directory, or null
 *  for the default one, and who is logged in there when the machine has
 *  more than one. */
export interface ConsentHome {
  dir: string | null;
  label: string | null;
}

const usual: ConsentHome = { dir: null, label: null };

export class ClaudeConsent {
  private readonly sheet = new Sheet("device-sheet confirm-sheet consent-sheet");

  constructor(
    private readonly words: ConsentWords,
    private readonly write: WriteConsent,
  ) {}

  /** Asks, and settles true once Claude Code on `machine` records the
   *  user's consent; false when the sheet is closed without it. `go` names
   *  the button, as what pressing it will do there. */
  allow(machine: string, go: string, home: ConsentHome = usual): Promise<boolean> {
    const { words } = this;
    return this.ask(words.title(machine, home.label), words.what(machine, home.label), go, () => this.write(machine, true, home.dir));
  }

  /** Asks before taking the consent back; true once it is withdrawn. */
  withdraw(machine: string, home: ConsentHome = usual): Promise<boolean> {
    const { words } = this;
    return this.ask(words.withdrawTitle(machine, home.label), words.withdrawWhat, words.withdrawGo, () => this.write(machine, false, home.dir));
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
            note.title = error instanceof Error ? error.message : String(error);
            note.hidden = false;
          });
      };
      sheet.dialog.addEventListener("close", () => settle(done), { once: true });
      sheet.show();
    });
  }
}
