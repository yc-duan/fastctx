// A small panel opened from a button in a conversation (`world-gui.md`,
// 界面的结构): the button opens it and a second press closes it; a press
// anywhere inside never closes it, a press outside does; it rises in and
// settles away. On a phone the same panel is a sheet from the bottom, and
// the shade behind it is "outside".

import { layerOpened, type Layer } from "../app/back";
import { reducedMotion } from "../motion";

export class Popover {
  private leaving = false;
  private readonly layer: Layer = {
    isOpen: () => this.panel.isConnected && !this.panel.hidden,
    close: () => this.hide(),
  };

  constructor(
    private readonly panel: HTMLElement,
    private readonly button: HTMLElement,
    private readonly closed: () => void = () => {},
  ) {
    button.setAttribute("aria-expanded", "false");
    // Opened from a caption, it may lie over one, which drags the window on Windows.
    panel.dataset.worldTitlebar ??= "client";
    // Judged by the press's path as it was when the press began: what the
    // press does inside the panel may redraw the very element it landed on.
    document.addEventListener(
      "pointerdown",
      (event) => {
        if (!this.open) return;
        const path = event.composedPath();
        if (!path.includes(panel) && !path.includes(button)) this.hide();
      },
      true,
    );
    panel.addEventListener("animationend", (event) => {
      if (event.target === panel && this.leaving) this.settle();
    });
  }

  get open(): boolean {
    return !this.panel.hidden && !this.leaving;
  }

  show(): void {
    if (this.leaving) this.settle();
    this.leaving = false;
    this.panel.classList.remove("popover-leaving");
    this.panel.hidden = false;
    this.button.setAttribute("aria-expanded", "true");
    layerOpened(this.layer);
  }

  hide(): void {
    if (this.panel.hidden || this.leaving) return;
    this.button.setAttribute("aria-expanded", "false");
    if (reducedMotion()) {
      this.panel.hidden = true;
      this.closed();
      return;
    }
    this.leaving = true;
    const before = getComputedStyle(this.panel).animationName;
    this.panel.classList.add("popover-leaving");
    // A panel drawn with no way of leaving goes at once rather than never.
    const after = getComputedStyle(this.panel).animationName;
    if (after === "none" || after === before) this.settle();
  }

  toggle(): void {
    if (this.open) this.hide();
    else this.show();
  }

  private settle(): void {
    this.leaving = false;
    this.panel.classList.remove("popover-leaving");
    this.panel.hidden = true;
    this.closed();
  }
}
