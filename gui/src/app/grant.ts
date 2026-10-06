// Setting what a member may use, and whether it holds management rights
// (`world-network.md`, 管理权与范围), from a machine that holds them: the
// member's scope as it stands, set part by part, and the one button that
// signs the change. The World refuses what this machine may not sign, and
// the sheet stays open with why.

import { escape } from "../conversation/prose";
import type { Scope, Standing } from "../world/world";
import { Sheet } from "./confirm";
import { ScopeEditor, type ScopeChoices } from "./scope";
import type { MachinesWords, ScopeWords } from "./words";

export interface GrantWords {
  close: string;
  working: string;
  failed(why: string): string;
}

export class GrantSheet {
  private readonly sheet = new Sheet("device-sheet grant-sheet");

  constructor(
    private readonly words: MachinesWords,
    private readonly scopeWords: ScopeWords,
    private readonly sheetWords: GrantWords,
  ) {}

  /** Opens the sheet for `member`, as it stands; `grant` signs what is set. */
  open(
    member: string,
    standing: Standing,
    choices: Omit<ScopeChoices, "manage">,
    grant: (scope: Scope, manage: boolean) => Promise<void>,
    done: () => void,
  ): void {
    const { sheet } = this;
    const editor = new ScopeEditor(this.scopeWords, { ...choices, manage: { held: standing.manage === true } }, standing.scope);
    sheet.body.innerHTML = `
      ${sheet.head(this.words.scopeTitle(member), this.sheetWords.close)}
      <div class="grant-fields">${editor.html()}</div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(this.words.scopeGo)}</button></footer>`;
    editor.hook(sheet.body.querySelector<HTMLElement>(".grant-fields")!);
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      if (sheet.busy) return;
      const button = sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
      const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      sheet.busy = true;
      button.disabled = true;
      button.textContent = this.sheetWords.working;
      grant(editor.value(), editor.managing())
        .then(() => {
          sheet.busy = false;
          sheet.close();
          done();
        })
        .catch((error: unknown) => {
          sheet.busy = false;
          button.disabled = false;
          button.textContent = this.words.scopeGo;
          note.textContent = this.sheetWords.failed(error instanceof Error ? error.message : String(error));
          note.hidden = false;
        });
    };
    sheet.show();
  }
}
