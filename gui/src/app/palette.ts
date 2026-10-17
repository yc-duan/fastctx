// The command palette, Ctrl+K: one place to go anywhere in the World and
// to do what is done often. Typing narrows the list to what contains the
// words typed, in order; the arrow keys choose and Enter goes.

import { escape } from "../conversation/prose";
import type { PaletteWords } from "./words";
import { composing } from "../typing";

export interface PaletteItem {
  group: string;
  label: string;
  /** Smaller text beside the label: where, what. */
  note?: string;
  run(): void;
}

export interface PaletteOptions {
  words: PaletteWords;
  /** Everything that can be chosen now. */
  items(): PaletteItem[];
}

/** Whether every word typed is found in the text, in any order. */
function matches(text: string, query: string): boolean {
  const haystack = text.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

export class Palette {
  private readonly dialog: HTMLDialogElement;
  private readonly input: HTMLInputElement;
  private readonly list: HTMLElement;
  private shown: PaletteItem[] = [];
  private chosen = 0;

  constructor(private readonly options: PaletteOptions) {
    const { words } = options;
    this.dialog = document.createElement("dialog");
    this.dialog.className = "sheet palette";
    this.dialog.setAttribute("aria-label", words.placeholder);
    this.dialog.innerHTML = `
      <div class="sheet-body palette-body">
        <input class="palette-input" type="text" autocomplete="off" spellcheck="false" placeholder="${escape(words.placeholder)}" aria-label="${escape(words.placeholder)}" role="combobox" aria-expanded="true" aria-controls="palette-list" aria-autocomplete="list">
        <ul class="palette-list" id="palette-list" role="listbox"></ul>
        <p class="palette-hint">${escape(words.hint)}</p>
      </div>`;
    document.body.append(this.dialog);
    this.input = this.dialog.querySelector<HTMLInputElement>(".palette-input")!;
    this.list = this.dialog.querySelector<HTMLElement>(".palette-list")!;
    this.dialog.addEventListener("click", (event) => {
      if (event.target === this.dialog) this.dialog.close();
      const item = (event.target as HTMLElement).closest<HTMLElement>("[data-index]");
      if (item) this.run(Number(item.dataset.index));
    });
    this.input.addEventListener("input", () => {
      this.chosen = 0;
      this.draw();
    });
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const step = event.key === "ArrowDown" ? 1 : -1;
        this.chosen = (this.chosen + step + this.shown.length) % Math.max(1, this.shown.length);
        this.mark();
      } else if (event.key === "Enter" && !composing(event)) {
        event.preventDefault();
        this.run(this.chosen);
      }
    });
    addEventListener("keydown", (event) => {
      // A managed machine's one page has nowhere else to go.
      if (document.querySelector(".shell.alone")) return;
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        if (this.dialog.open) this.dialog.close();
        else this.open();
      }
    });
  }

  open(): void {
    this.input.value = "";
    this.chosen = 0;
    this.draw();
    this.dialog.showModal();
    this.input.focus();
  }

  private draw(): void {
    const query = this.input.value.trim();
    this.shown = this.options.items().filter((item) => !query || matches(`${item.label} ${item.note ?? ""} ${item.group}`, query));
    if (!this.shown.length) {
      this.list.innerHTML = `<li class="palette-nothing">${escape(this.options.words.nothing)}</li>`;
      this.input.removeAttribute("aria-activedescendant");
      return;
    }
    let group = "";
    this.list.innerHTML = this.shown
      .map((item, index) => {
        const head = item.group !== group ? `<li class="palette-group" role="presentation">${escape(item.group)}</li>` : "";
        group = item.group;
        return `${head}<li class="palette-item" role="option" id="palette-item-${index}" data-index="${index}"><span class="palette-label">${escape(item.label)}</span>${item.note ? `<span class="palette-note">${escape(item.note)}</span>` : ""}</li>`;
      })
      .join("");
    this.mark();
  }

  private mark(): void {
    for (const element of this.list.querySelectorAll<HTMLElement>("[data-index]")) {
      const chosen = Number(element.dataset.index) === this.chosen;
      element.classList.toggle("chosen", chosen);
      element.setAttribute("aria-selected", String(chosen));
      if (chosen) {
        element.scrollIntoView({ block: "nearest" });
        this.input.setAttribute("aria-activedescendant", element.id);
      }
    }
  }

  private run(index: number): void {
    const item = this.shown[index];
    if (!item) return;
    this.dialog.close();
    item.run();
  }
}
