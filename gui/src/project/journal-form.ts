// The user writing in a project's journals (`world-journals.md`): an entry
// in a journal, or a new journal with what it covers and its first entry.
// What the user writes is quoted as their own words; once written, an
// entry is never changed, and the sheet says so.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { ProjectWords } from "./words";

export interface JournalFormOptions {
  words: ProjectWords;
  /** Writes an entry; `description` starts the journal when it has none. */
  write(town: string, journal: string, text: string, description: string | null): Promise<string>;
  /** The entry was written. */
  written(town: string, journal: string): void;
}

const subject = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/;

export class JournalForm {
  private readonly dialog: HTMLDialogElement;
  private readonly body: HTMLElement;
  private target: { town: string; journal: string | null } = { town: "", journal: null };
  private busy = false;

  constructor(private readonly options: JournalFormOptions) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "sheet form-sheet";
    this.dialog.innerHTML = `<form class="sheet-body" method="dialog"></form>`;
    document.body.append(this.dialog);
    this.body = this.dialog.querySelector("form")!;
    this.dialog.addEventListener("click", (event) => {
      if (event.target === this.dialog || (event.target as HTMLElement).closest(".sheet-close")) this.close();
    });
    this.dialog.addEventListener("cancel", (event) => {
      if (this.busy) event.preventDefault();
    });
    this.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
  }

  /** Opens the sheet for an entry in `journal`, or for a new journal when
   *  `journal` is null. */
  open(town: string, journal: string | null): void {
    const { words } = this.options;
    this.target = { town, journal };
    this.body.innerHTML = `
      <header class="sheet-head">
        <h2 class="sheet-title">${escape(journal ? words.writeTitle(journal) : words.newJournalTitle)}</h2>
        <button class="icon-button sheet-close" type="button" aria-label="${escape(words.close)}">${glyph("close", 18)}</button>
      </header>
      <div class="form-scroll">
        ${
          journal
            ? ""
            : `<label class="field"><span class="field-label">${escape(words.journalName)}</span>
          <input name="journal" class="mono" required autocomplete="off" spellcheck="false" pattern="[a-z][a-z0-9]*(-[a-z0-9]+)*">
          <span class="field-hint">${escape(words.journalNameHint)}</span></label>
        <label class="field"><span class="field-label">${escape(words.description)}</span>
          <textarea name="description" rows="2" required></textarea>
          <span class="field-hint">${escape(words.descriptionHint)}</span></label>`
        }
        <label class="field"><span class="field-label">${escape(words.entry)}</span>
          <textarea name="text" rows="9" required></textarea>
          <span class="field-hint">${escape(words.entryHint)}</span></label>
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.save)}</button></footer>`;
    if (!this.dialog.open) this.dialog.showModal();
    this.body.querySelector<HTMLElement>(journal ? "[name=text]" : "[name=journal]")?.focus();
  }

  close(): void {
    if (!this.busy) this.dialog.close();
  }

  private async submit(): Promise<void> {
    if (this.busy) return;
    const { words } = this.options;
    const value = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`)?.value.trim() ?? "";
    const journal = this.target.journal ?? value("journal");
    const text = value("text");
    if (!subject.test(journal) || !text) return;
    const description = this.target.journal ? null : value("description") || null;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    const note = this.body.querySelector<HTMLElement>(".sheet-note")!;
    this.busy = true;
    go.disabled = true;
    go.textContent = words.saving;
    note.hidden = true;
    try {
      await this.options.write(this.target.town, journal, text, description);
      this.busy = false;
      this.dialog.close();
      this.options.written(this.target.town, journal);
    } catch (error) {
      this.busy = false;
      go.disabled = false;
      go.textContent = words.save;
      note.textContent = words.failed(String(error));
      note.title = String(error);
      note.hidden = false;
    }
  }
}
