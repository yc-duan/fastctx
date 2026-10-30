// The command palette, Ctrl+K: one place to go anywhere in the World and
// to do what is done often. Typing narrows the list to what contains the
// words typed, in order; the arrow keys choose and Enter goes.

import { leave } from "./leave";
import { onShade, watchShade } from "./shade-press";
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

/** How many candidates are drawn as the list opens or narrows: more than a
 *  screen holds. The rest follow a batch a frame, so a World of thousands
 *  opens at once; one the keys reach before its batch is drawn on the spot. */
const FIRST = 120;
const BATCH = 400;

export class Palette {
  private readonly dialog: HTMLDialogElement;
  private readonly input: HTMLInputElement;
  private readonly list: HTMLElement;
  private shown: PaletteItem[] = [];
  private chosen = 0;
  private marked: HTMLElement | null = null;
  /** How many of `shown` the list holds, and the group the last of them
   *  is in. */
  private drawn = 0;
  private group = "";
  private filling = 0;

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
    watchShade(this.dialog);
    // Escape closes it as a click outside does, going as it came.
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    this.dialog.addEventListener("click", (event) => {
      if (onShade(this.dialog, event)) this.close();
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
        if (this.dialog.open) this.close();
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
    cancelAnimationFrame(this.filling);
    this.marked = null;
    const query = this.input.value.trim();
    this.shown = this.options.items().filter((item) => !query || matches(`${item.label} ${item.note ?? ""} ${item.group}`, query));
    if (!this.shown.length) {
      this.list.innerHTML = `<li class="palette-nothing">${escape(this.options.words.nothing)}</li>`;
      this.input.removeAttribute("aria-activedescendant");
      return;
    }
    this.list.innerHTML = "";
    this.drawn = 0;
    this.group = "";
    this.more(Math.max(FIRST, this.chosen + 1));
    this.mark();
  }

  /** Draws the candidates up to `until`, and then the rest a batch a
   *  frame. */
  private more(until: number): void {
    const end = Math.min(until, this.shown.length);
    if (end > this.drawn) {
      let html = "";
      for (let index = this.drawn; index < end; index += 1) {
        const item = this.shown[index]!;
        if (item.group !== this.group) html += `<li class="palette-group" role="presentation">${escape(item.group)}</li>`;
        this.group = item.group;
        const label = [item.label, item.note].filter(Boolean).join(" ");
        html += `<li class="palette-item" role="option" aria-label="${escape(label)}" aria-selected="false" id="palette-item-${index}" data-index="${index}"><span class="palette-label">${escape(item.label)}</span>${item.note ? `<span class="palette-note">${escape(item.note)}</span>` : ""}</li>`;
      }
      this.list.insertAdjacentHTML("beforeend", html);
      this.drawn = end;
    }
    cancelAnimationFrame(this.filling);
    if (this.drawn < this.shown.length) this.filling = requestAnimationFrame(() => this.more(this.drawn + BATCH));
  }

  private mark(): void {
    if (this.chosen >= this.drawn) this.more(this.chosen + 1);
    const element = this.list.querySelector<HTMLElement>(`[data-index="${this.chosen}"]`);
    if (this.marked !== element) {
      this.marked?.classList.remove("chosen");
      this.marked?.setAttribute("aria-selected", "false");
      element?.classList.add("chosen");
      element?.setAttribute("aria-selected", "true");
      this.marked = element;
    }
    if (element) {
      element.scrollIntoView({ block: "nearest" });
      this.input.setAttribute("aria-activedescendant", element.id);
    }
  }

  private run(index: number): void {
    const item = this.shown[index];
    if (!item) return;
    this.close();
    item.run();
  }

  /** Goes as a sheet does (`sheet.css`). */
  private close(): void {
    cancelAnimationFrame(this.filling);
    const body = this.dialog.querySelector(".sheet-body");
    if (this.dialog.open) leave(this.dialog, () => this.dialog.close(), { as: "sheet-leaving", ...(body ? { moving: body } : {}) });
  }
}
