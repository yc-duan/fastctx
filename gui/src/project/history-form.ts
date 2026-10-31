// The user writing a new document into a history (`world-history.md`): its
// name, the line that says what it covers and the words to find it by, and
// its first paragraph, which starts with today's date as every paragraph
// does. A name the World would not take says why beside it, in a line, only
// when it is wrong.

import { whyOf } from "../world/again";
import { leave } from "../app/leave";
import { onShade, watchShade } from "../app/shade-press";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { composeText, NAME, type HistoryScope } from "../world/history";
import type { HistoryWords } from "./history-words";

export interface HistoryFormOptions {
  words: HistoryWords;
  close: string;
  /** Writes the document's text; resolves once the World has it. */
  write(scope: HistoryScope, name: string, text: string): Promise<void>;
  /** The document was written. */
  written(scope: HistoryScope, name: string): void;
  /** Today, as the paragraph's date. */
  today(): string;
}

export class HistoryForm {
  private readonly dialog: HTMLDialogElement;
  private readonly body: HTMLElement;
  private scope: HistoryScope | null = null;
  private busy = false;

  constructor(private readonly options: HistoryFormOptions) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "sheet form-sheet";
    this.dialog.innerHTML = `<form class="sheet-body" method="dialog"></form>`;
    document.body.append(this.dialog);
    this.body = this.dialog.querySelector("form")!;
    watchShade(this.dialog);
    this.dialog.addEventListener("click", (event) => {
      if (onShade(this.dialog, event) || (event.target as HTMLElement).closest(".sheet-close")) this.close();
    });
    // Escape closes it as the close button does, going as it came.
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    this.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    this.body.addEventListener("input", (event) => {
      if ((event.target as HTMLElement).matches("[name=name]")) this.checkName(false);
    });
  }

  open(scope: HistoryScope): void {
    const { words } = this.options;
    this.scope = scope;
    this.body.innerHTML = `
      <header class="sheet-head">
        <h2 class="sheet-title">${escape(words.newDocument)}</h2>
        <button class="icon-button sheet-close" type="button" aria-label="${escape(this.options.close)}">${glyph("close", 18)}</button>
      </header>
      <div class="form-scroll">
        <label class="field"><span class="field-label">${escape(words.name)}</span>
          <input name="name" required autocomplete="off" spellcheck="false" autocapitalize="off">
          <span class="field-hint hf-rule" hidden>${escape(words.nameRule)}</span></label>
        <label class="field"><span class="field-label">${escape(words.description)}</span>
          <input name="description" required autocomplete="off"></label>
        <label class="field"><span class="field-label">${escape(words.firstParagraph)}</span>
          <textarea name="text" rows="8" required>${escape(`${this.options.today()}：`)}</textarea></label>
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.write)}</button></footer>`;
    if (!this.dialog.open) this.dialog.showModal();
    this.body.querySelector<HTMLElement>("[name=name]")?.focus();
  }

  close(): void {
    if (!this.busy && this.dialog.open) leave(this.dialog, () => this.dialog.close(), { as: "sheet-leaving", moving: this.body });
  }

  /** Whether the name is one the World takes; its rule shown beside it
   *  where it is not, once something is typed or it is sent. */
  private checkName(sending: boolean): boolean {
    const input = this.body.querySelector<HTMLInputElement>("[name=name]")!;
    const rule = this.body.querySelector<HTMLElement>(".hf-rule")!;
    const name = input.value.trim();
    const good = NAME.test(name) && name.length <= 120;
    rule.hidden = good || (!sending && name === "");
    return good;
  }

  private async submit(): Promise<void> {
    if (this.busy || !this.scope) return;
    const { words } = this.options;
    const value = (name: string) => this.body.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[name="${name}"]`)?.value.trim() ?? "";
    const name = value("name");
    const description = value("description");
    const text = value("text");
    // A first paragraph is more than the date it was handed.
    if (!this.checkName(true) || !description || !text.replace(/^\d{4}-\d{2}-\d{2}\s*[:：]\s*/, "")) return;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    const note = this.body.querySelector<HTMLElement>(".sheet-note")!;
    const scope = this.scope;
    this.busy = true;
    go.disabled = true;
    note.hidden = true;
    try {
      await this.options.write(scope, name, composeText(name, description, text));
      this.busy = false;
      this.close();
      this.options.written(scope, name);
    } catch (error) {
      this.busy = false;
      go.disabled = false;
      // A new document fails on another version only where one of that name
      // is written already: the World says so as it does of any version
      // that came first (`fastctx-types/src/history.rs`).
      const why = whyOf(error);
      const taken = /^the document changed;|already exists/i.test(why);
      note.textContent = taken ? words.taken : words.notSaved;
      note.title = why;
      note.hidden = false;
    }
  }
}
