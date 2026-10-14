// A draft's box as tall as its words, up to the most its style allows.
//
// It is measured on a hidden copy of itself, beside it so the same style
// applies, never by collapsing the box: a box collapsed for a moment gives
// the conversation above it its room back, and at the foot of the
// conversation the browser scrolls it up a line to keep within the room,
// so each key pressed shook the page.

export class DraftFit {
  private mirror: HTMLTextAreaElement | null = null;

  constructor(private readonly input: HTMLTextAreaElement) {}

  fit(): void {
    const { input } = this;
    if (!input.isConnected) return;
    const mirror = this.mirrorOf();
    mirror.style.width = `${input.getBoundingClientRect().width}px`;
    mirror.value = input.value;
    const height = `${mirror.scrollHeight}px`;
    if (input.style.height !== height) input.style.height = height;
    // A scrollbar only once the words outgrow the box's tallest; below that
    // a rounding of the line height would show one for nothing.
    const overflow = mirror.scrollHeight > input.clientHeight + 1 ? "auto" : "hidden";
    if (input.style.overflowY !== overflow) input.style.overflowY = overflow;
  }

  private mirrorOf(): HTMLTextAreaElement {
    if (this.mirror?.isConnected) return this.mirror;
    const mirror = document.createElement("textarea");
    mirror.rows = this.input.rows;
    mirror.tabIndex = -1;
    mirror.setAttribute("aria-hidden", "true");
    mirror.className = this.input.className;
    Object.assign(mirror.style, {
      position: "absolute",
      top: "0",
      left: "0",
      height: "0",
      minHeight: "0",
      overflow: "hidden",
      visibility: "hidden",
      pointerEvents: "none",
      boxSizing: getComputedStyle(this.input).boxSizing,
    });
    this.input.after(mirror);
    this.mirror = mirror;
    return mirror;
  }
}
