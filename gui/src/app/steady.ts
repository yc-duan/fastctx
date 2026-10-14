// A panel that keeps the size it opened at (`world-gui.md`, 审美): what
// changes inside it while it is open, an answer come, a row chosen, a
// number grown, is laid out within it, scrolling where it outgrows it; the
// panel itself never shrinks under the reader. One whose content is still
// coming in when it opens may grow as it comes, up to its tallest, never
// back. It takes its size anew only when it opens again or the window
// changes size.

export class Steady {
  private held = false;
  private changes: MutationObserver | null = null;
  private readonly resized = (): void => {
    if (!this.held) return;
    this.frame.style.height = "";
    this.measure();
  };

  /** `grows`: what comes in after it opened may make it taller. */
  constructor(
    private readonly frame: HTMLElement,
    private readonly grows = false,
  ) {}

  /** Takes the size the panel has now, its content laid out, as its own
   *  until it is let go. */
  hold(): void {
    this.frame.style.height = "";
    this.measure();
    if (!this.held) addEventListener("resize", this.resized);
    this.held = true;
  }

  /** After its content changed: a panel that grows takes what more it now
   *  needs, as far as its tallest; none ever gives any back. */
  settle(): void {
    if (!this.held || !this.grows) return;
    const held = this.frame.offsetHeight;
    this.frame.style.height = "";
    const natural = this.frame.offsetHeight;
    this.frame.style.height = `${Math.max(held, natural)}px`;
  }

  /** Takes what more it needs as what is in it changes, before the change
   *  is painted, until it is let go. */
  watch(): void {
    this.changes ??= new MutationObserver(() => this.settle());
    this.changes.observe(this.frame, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["hidden", "class"] });
  }

  /** The panel's size is its content's again. */
  free(): void {
    this.changes?.disconnect();
    this.frame.style.height = "";
    if (this.held) removeEventListener("resize", this.resized);
    this.held = false;
  }

  /** Its laid-out size, whatever it is drawn at as it comes in. */
  private measure(): void {
    const height = this.frame.offsetHeight;
    if (height > 0) this.frame.style.height = `${height}px`;
  }
}
