// Names that match what is typed into a field, listed under it as one types
// (`world-gui.md`, 账号、机器、共享与协助): those that start with it first,
// then those that hold it; the up and down keys move among them, Enter or a
// press fills the field with one, Escape or a press elsewhere puts the list
// away. A whole name can still be typed; nothing is listed when nothing
// matches.

import { escape } from "../conversation/prose";
import { composing } from "../typing";

/** At most this many names listed at once. */
const SHOWN = 6;

let lists = 0;

export class NameSuggest {
  private readonly list: HTMLElement;
  private input: HTMLInputElement | null = null;
  private names: string[] = [];
  private shown: string[] = [];
  private active = -1;

  constructor(label: string) {
    lists += 1;
    this.list = document.createElement("div");
    this.list.className = "name-suggest";
    this.list.id = `name-suggest-${lists}`;
    this.list.setAttribute("role", "listbox");
    this.list.setAttribute("aria-label", label);
    this.list.hidden = true;
    // A press on a name keeps the field's focus, so the list stays to be
    // pressed.
    this.list.addEventListener("pointerdown", (event) => event.preventDefault());
    this.list.addEventListener("click", (event) => {
      const name = (event.target as HTMLElement).closest<HTMLElement>("[data-name]")?.dataset.name;
      if (name !== undefined) this.pick(name);
    });
  }

  /** Lists `names` under `input` as it is typed into; a field drawn again
   *  is attached again. */
  attach(input: HTMLInputElement, names: string[]): void {
    this.input = input;
    this.names = names;
    input.setAttribute("role", "combobox");
    input.setAttribute("aria-autocomplete", "list");
    input.setAttribute("aria-controls", this.list.id);
    input.setAttribute("aria-expanded", "false");
    input.after(this.list);
    input.addEventListener("input", () => this.update());
    input.addEventListener("keydown", (event) => this.key(event));
    input.addEventListener("blur", () => this.close());
    this.close();
  }

  private update(): void {
    const typed = this.input?.value.trim().toLowerCase() ?? "";
    const starts = this.names.filter((name) => typed && name.toLowerCase().startsWith(typed) && name.toLowerCase() !== typed);
    const holds = this.names.filter((name) => typed && !name.toLowerCase().startsWith(typed) && name.toLowerCase().includes(typed));
    this.shown = [...starts, ...holds].slice(0, SHOWN);
    this.active = this.shown.length ? 0 : -1;
    this.draw();
  }

  private draw(): void {
    const open = this.shown.length > 0;
    this.list.hidden = !open;
    this.input?.setAttribute("aria-expanded", String(open));
    this.list.innerHTML = this.shown
      .map((name, at) => `<button type="button" role="option" class="name-option${at === this.active ? " active" : ""}" aria-selected="${at === this.active}" id="${this.list.id}-${at}" data-name="${escape(name)}" tabindex="-1">${escape(name)}</button>`)
      .join("");
    if (this.active >= 0) this.input?.setAttribute("aria-activedescendant", `${this.list.id}-${this.active}`);
    else this.input?.removeAttribute("aria-activedescendant");
  }

  private key(event: KeyboardEvent): void {
    // The input method's own keys pick its candidates, not a name.
    if (!this.shown.length || composing(event)) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      this.active = (this.active + step + this.shown.length) % this.shown.length;
      this.draw();
    } else if (event.key === "Enter" && this.active >= 0) {
      // The name fills the field; the form goes on its next Enter.
      event.preventDefault();
      this.pick(this.shown[this.active]!);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      this.close();
    }
  }

  private pick(name: string): void {
    if (!this.input) return;
    this.input.value = name;
    this.close();
    this.input.focus();
  }

  private close(): void {
    this.shown = [];
    this.active = -1;
    this.draw();
  }
}
