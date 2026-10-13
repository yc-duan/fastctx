// A short menu of what can be done to one thing, opened by its "⋯": beside
// the button on a computer, rising from below on a narrow screen, drawn as
// a conversation's "⋯" is (`talk-menu.css`). A press outside, Escape or the
// system back closes it.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { layerOpened, type Layer } from "./back";

export interface MenuAct {
  act: string;
  icon: GlyphName;
  label: string;
  danger?: boolean;
}

const narrow = matchMedia("(max-width: 760px)");

export class ActsMenu {
  private readonly element: HTMLElement;
  private readonly shade: HTMLElement;
  private anchor: HTMLElement | null = null;
  private chosen: ((act: string) => void) | null = null;
  private readonly layer: Layer = { isOpen: () => !this.element.hidden, close: () => this.close() };

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
      }
    });
    document.addEventListener(
      "pointerdown",
      (event) => {
        if (this.element.hidden) return;
        const path = event.composedPath();
        if (path.includes(this.element) || (this.anchor && path.includes(this.anchor))) return;
        this.close();
      },
      true,
    );
  }

  /** Opens beside `anchor` with `acts`, headed by `title` on a narrow
   *  screen; pressed again, it closes. */
  open(anchor: HTMLElement, title: string, acts: MenuAct[], chosen: (act: string) => void): void {
    if (!this.element.hidden && this.anchor === anchor) return this.close();
    this.anchor = anchor;
    this.chosen = chosen;
    const sheet = narrow.matches;
    const head = sheet ? `<div class="talk-menu-head"><span class="talk-menu-name">${escape(title)}</span></div>` : "";
    this.element.innerHTML =
      head +
      acts
        .map(
          (act) =>
            `<button class="talk-menu-item${act.danger ? " danger" : ""}" type="button" role="menuitem" data-act="${escape(act.act)}">${glyph(act.icon, 16)}<span class="talk-menu-label">${escape(act.label)}</span></button>`,
        )
        .join("");
    this.element.classList.toggle("as-sheet", sheet);
    this.shade.hidden = !sheet;
    this.element.hidden = false;
    anchor.setAttribute("aria-expanded", "true");
    if (!sheet) this.place(anchor);
    layerOpened(this.layer);
    this.element.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  }

  close(): void {
    if (this.element.hidden) return;
    this.element.hidden = true;
    this.shade.hidden = true;
    this.anchor?.setAttribute("aria-expanded", "false");
    if (this.element.contains(document.activeElement)) this.anchor?.focus({ preventScroll: true });
    this.anchor = null;
    this.chosen = null;
  }

  /** Under the button, its right edge on the button's, in the window. */
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
