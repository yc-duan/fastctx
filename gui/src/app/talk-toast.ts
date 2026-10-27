// One line said for a moment: what was just done to a conversation, with
// the way back where there is one, or what was not done, why kept for
// hovering (`world-gui.md`, 界面的结构: 几秒内能撤回). It rests over the
// sidebar just above its foot, so the accounts stay in sight, and at the
// window's foot where no sidebar shows. A new one takes the place of the one
// before.

import { leave, leaving, stay } from "./leave";
import { escape } from "../conversation/prose";

/** How long a line stays, and an undo with it. */
const SHOWN_MS = 6_000;
/** Between the line and the sidebar's foot, in CSS pixels. */
const ABOVE_FOOT = 4;

export class TalkToast {
  readonly element: HTMLElement;
  private timer = 0;
  private undo: (() => void) | null = null;

  constructor(
    private readonly words: { undo: string },
    /** Where the sidebar's foot is on the screen, while it shows. */
    private readonly foot: () => DOMRect | null = () => null,
  ) {
    this.element = document.createElement("div");
    this.element.className = "talk-toast";
    this.element.setAttribute("role", "status");
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => {
      if (!(event.target as HTMLElement).closest(".talk-toast-undo")) return;
      const undo = this.undo;
      this.hide();
      undo?.();
    });
    // Kept while the pointer rests on it, to be read or undone.
    this.element.addEventListener("pointerenter", () => clearTimeout(this.timer));
    this.element.addEventListener("pointerleave", () => this.later());
    document.body.append(this.element);
  }

  say(text: string, options: { undo?: () => void; detail?: string } = {}): void {
    this.undo = options.undo ?? null;
    this.element.innerHTML = `<span class="talk-toast-text"${options.detail ? ` title="${escape(options.detail)}"` : ""}>${escape(text)}</span>${
      options.undo ? `<button class="talk-toast-undo" type="button">${escape(this.words.undo)}</button>` : ""
    }`;
    this.element.classList.toggle("wrong", !!options.detail);
    this.place();
    stay(this.element);
    this.element.hidden = false;
    // Played again for a line said over another.
    this.element.classList.remove("talk-toast-in");
    void this.element.offsetWidth;
    this.element.classList.add("talk-toast-in");
    this.later();
  }

  private place(): void {
    const foot = this.foot();
    const style = this.element.style;
    style.left = foot ? `${foot.left}px` : "";
    style.right = foot ? "auto" : "";
    style.bottom = foot ? `${innerHeight - foot.top + ABOVE_FOOT}px` : "";
    style.maxWidth = foot ? `${foot.width}px` : "";
  }

  private later(): void {
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.hide(), SHOWN_MS);
  }

  private hide(): void {
    clearTimeout(this.timer);
    this.undo = null;
    if (this.element.hidden || leaving(this.element)) return;
    leave(this.element, () => {
      this.element.hidden = true;
      this.element.innerHTML = "";
    });
  }
}
