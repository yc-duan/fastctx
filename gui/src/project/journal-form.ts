// The user writing a journal in a project (`world-memory.md`): its name,
// the line that says what it is, and its text, written once as the user's
// own and never changed after. A name the World would not take says why
// beside it, in a line, only when it is wrong.

import { onShade, watchShade } from "../app/shade-press";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { composeText } from "../world/memory";
import type { MemoryWords } from "./memory-words";

export interface JournalFormOptions {
  words: MemoryWords;
  close: string;
  /** Writes the journal's text; resolves once the World has it. */
  write(town: string, name: string, text: string): Promise<void>;
  /** The journal was written. */
  written(town: string, name: string): void;
}

/** Letters and numbers of any script, with hyphens between them, as the
 *  World names a document. */
const NAME = /^[\p{L}\p{N}][\p{L}\p{N}\p{M}]*(?:-[\p{L}\p{N}][\p{L}\p{N}\p{M}]*)*$/u;

export class JournalForm {
  private readonly dialog: HTMLDialogElement;
  private readonly body: HTMLElement;
  private town = "";
  private busy = false;

  constructor(private readonly options: JournalFormOptions) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "sheet form-sheet";
    this.dialog.innerHTML = `<form class="sheet-body" method="dialog"></form>`;
    document.body.append(this.dialog);
    this.body = this.dialog.querySelector("form")!;
    watchShade(this.dialog);
    this.dialog.addEventListener("click", (event) => {
      if (onShade(this.dialog, event) || (event.target as HTMLElement).closest(".sheet-close")) this.close();
    });
    this.dialog.addEventListener("cancel", (event) => {
      if (this.busy) event.preventDefault();
    });
    this.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    this.body.addEventListener("input", (event) => {
      if ((event.target as HTMLElement).matches("[name=name]")) this.checkName(false);
    });
  }

  open(town: string): void {
    const { words } = this.options;
    this.town = town;
    this.body.innerHTML = `
      <header class="sheet-head">
        <h2 class="sheet-title">${escape(words.newJournal)}</h2>
        <button class="icon-button sheet-close" type="button" aria-label="${escape(this.options.close)}">${glyph("close", 18)}</button>
      </header>
      <div class="form-scroll">
        <label class="field"><span class="field-label">${escape(words.name)}</span>
          <input name="name" required autocomplete="off" spellcheck="false">
          <span class="field-hint jf-rule" hidden>${escape(words.nameRule)}</span></label>
        <label class="field"><span class="field-label">${escape(words.description)}</span>
          <input name="description" required autocomplete="off"></label>
        <label class="field"><span class="field-label">${escape(words.body)}</span>
          <textarea name="text" rows="10" required></textarea></label>
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit" title="${escape(words.writeHint)}">${escape(words.write)}</button></footer>`;
    if (!this.dialog.open) this.dialog.showModal();
    this.body.querySelector<HTMLElement>("[name=name]")?.focus();
  }

  close(): void {
    if (!this.busy) this.dialog.close();
  }

  /** Whether the name is one the World takes; its rule shown beside it
   *  where it is not, once something is typed or it is sent. */
  private checkName(sending: boolean): boolean {
    const input = this.body.querySelector<HTMLInputElement>("[name=name]")!;
    const rule = this.body.querySelector<HTMLElement>(".jf-rule")!;
    const name = input.value.trim();
    const good = NAME.test(name) && new TextEncoder().encode(name).length <= 240;
    rule.hidden = good || (!sending && name === "");
    return good;
  }

  private async submit(): Promise<void> {
    if (this.busy) return;
    const { words } = this.options;
    const value = (name: string) => this.body.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[name="${name}"]`)?.value.trim() ?? "";
    const name = value("name");
    const description = value("description");
    const text = value("text");
    if (!this.checkName(true) || !description || !text) return;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    const note = this.body.querySelector<HTMLElement>(".sheet-note")!;
    this.busy = true;
    go.disabled = true;
    note.hidden = true;
    try {
      await this.options.write(this.town, name, composeText(name, description, text));
      this.busy = false;
      this.dialog.close();
      this.options.written(this.town, name);
    } catch (error) {
      this.busy = false;
      go.disabled = false;
      const taken = /exist|taken|already/i.test(String(error));
      note.textContent = taken ? words.taken : words.notSaved;
      note.title = String(error);
      note.hidden = false;
    }
  }
}
