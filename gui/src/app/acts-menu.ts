// A short menu of what can be done to one thing, opened by its "⋯": beside
// the button on a computer, rising from below on a narrow screen, drawn as
// a conversation's "⋯" is (`talk-menu.css`). Its button pressed again, a
// press outside, Escape, the system back, the page scrolling or the window
// resizing closes it.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { layerOpened, type Layer } from "./back";
import { stepped } from "./menu-keys";

export interface MenuAct {
  act: string;
  icon?: GlyphName;
  label: string;
  danger?: boolean;
  /** One of a set to choose from: marked when it is the one chosen. */
  checked?: boolean;
  /** A setting turned on or off by the press: a switch at the row's end. */
  on?: boolean;
  /** What it does, said under a pointer's rest, never on the menu. */
  hint?: string;
}

/** An act, a rule between groups, or a quiet label over a group. */
export type MenuEntry = MenuAct | { rule: true } | { note: string };

const narrow = matchMedia("(max-width: 760px)");

export class ActsMenu {
  private readonly element: HTMLElement;
  private readonly shade: HTMLElement;
  private anchor: HTMLElement | null = null;
  private chosen: ((act: string) => void) | null = null;
  private shown = false;
  private readonly layer: Layer = { isOpen: () => this.shown, close: () => this.close() };

  constructor() {
    this.shade = document.createElement("div");
    this.shade.className = "talk-menu-shade";
    this.shade.hidden = true;
    this.element = document.createElement("div");
    this.element.className = "talk-menu";
    this.element.setAttribute("role", "menu");
    this.element.hidden = true;
    document.body.append(this.shade, this.element);
    this.shade.addEventListener("click", () => this.close());
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
      if (!button || button.disabled) return;
      const chosen = this.chosen;
      this.close();
      chosen?.(button.dataset.act!);
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        this.close();
        return;
      }
      const items = [...this.element.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
      const next = stepped(items, document.activeElement, event.key);
      if (next === null) return;
      event.preventDefault();
      items[next]?.focus({ preventScroll: true });
    });
    this.element.addEventListener("animationend", (event) => {
      if (event.target === this.element && !this.shown) this.settle();
    });
    document.addEventListener(
      "pointerdown",
      (event) => {
        if (!this.shown) return;
        const path = event.composedPath();
        // Its own button is left to the press's click, which closes it.
        if (path.includes(this.element) || path.some((node) => node instanceof HTMLElement && this.isAnchor(node))) return;
        this.close();
      },
      true,
    );
    // Beside a button that moved, it would point at nothing.
    addEventListener("resize", () => this.close());
    document.addEventListener(
      "scroll",
      (event) => {
        if (this.shown && !this.element.contains(event.target as Node)) this.close();
      },
      true,
    );
  }

  /** Opens beside `anchor` with `entries`, headed by `title` on a narrow
   *  screen; pressed again, it closes. A button the page draws anew while
   *  the menu is open is known for the same by its `data-menu-key`.
   *  `beside` keeps it beside the anchor on a narrow screen too, for a
   *  short menu held up from what the finger rests on. */
  open(anchor: HTMLElement, title: string, entries: MenuEntry[], chosen: (act: string) => void, beside = false): void {
    if (this.shown && this.isAnchor(anchor)) return this.close();
    this.anchor = anchor;
    this.chosen = chosen;
    const sheet = narrow.matches && !beside;
    const head = sheet ? `<div class="talk-menu-head"><span class="talk-menu-name">${escape(title)}</span></div>` : "";
    this.element.innerHTML = head + entries.map(entry).join("");
    this.element.classList.remove("leaving");
    this.shade.classList.remove("leaving");
    this.element.classList.toggle("as-sheet", sheet);
    this.shade.hidden = !sheet;
    this.element.hidden = false;
    this.shown = true;
    anchor.setAttribute("aria-expanded", "true");
    if (!sheet) this.place(anchor);
    layerOpened(this.layer);
    this.element.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  }

  close(): void {
    if (!this.shown) return;
    this.shown = false;
    this.anchor?.setAttribute("aria-expanded", "false");
    if (this.element.contains(document.activeElement)) this.anchor?.focus({ preventScroll: true });
    this.anchor = null;
    this.chosen = null;
    const before = getComputedStyle(this.element).animationName;
    this.element.classList.add("leaving");
    this.shade.classList.add("leaving");
    // Where motion is off it goes at once.
    const after = getComputedStyle(this.element).animationName;
    if (after === "none" || after === before) this.settle();
  }

  private settle(): void {
    this.element.hidden = true;
    this.shade.hidden = true;
    this.element.classList.remove("leaving");
    this.shade.classList.remove("leaving");
  }

  private isAnchor(node: HTMLElement): boolean {
    const anchor = this.anchor;
    if (!anchor) return false;
    if (node === anchor) return true;
    const key = anchor.dataset.menuKey;
    return !!key && node.dataset.menuKey === key;
  }

  /** Under the button, its right edge on the button's, in the window;
   *  above it where the window ends first. */
  private place(anchor: HTMLElement): void {
    const box = anchor.getBoundingClientRect();
    const width = this.element.offsetWidth;
    const height = this.element.offsetHeight;
    const left = Math.min(Math.max(8, box.right - width), innerWidth - width - 8);
    let top = box.bottom + 4;
    if (top + height > innerHeight - 8) top = Math.max(8, box.top - 4 - height);
    this.element.style.left = `${Math.round(left)}px`;
    this.element.style.top = `${Math.round(top)}px`;
  }
}

function entry(item: MenuEntry): string {
  if ("rule" in item) return `<hr class="talk-menu-rule">`;
  if ("note" in item) return `<p class="talk-menu-note">${escape(item.note)}</p>`;
  const choice = item.checked !== undefined;
  const toggle = item.on !== undefined;
  const lead = choice
    ? `<span class="talk-menu-check">${item.checked ? glyph("check", 16) : ""}</span>`
    : item.icon
      ? glyph(item.icon, 16)
      : "";
  const role = choice
    ? `role="menuitemradio" aria-checked="${item.checked}"`
    : toggle
      ? `role="menuitemcheckbox" aria-checked="${item.on}"`
      : `role="menuitem"`;
  const tail = toggle ? `<span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span>` : "";
  const hint = item.hint ? ` title="${escape(item.hint)}"` : "";
  const kinds = `${item.danger ? " danger" : ""}${choice ? " talk-menu-choice" : ""}${toggle ? " talk-menu-toggle" : ""}`;
  return `<button class="talk-menu-item${kinds}" type="button" ${role} data-act="${escape(item.act)}"${hint}>${lead}<span class="talk-menu-label">${escape(item.label)}</span>${tail}</button>`;
}
