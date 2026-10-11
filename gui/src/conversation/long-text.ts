// Long text under an open tool row (`world-gui.md`, 对话): its first lines,
// then "+ 28 行". Pressed, that button stays where it is and what is above
// it grows upward, into a box of at most half the conversation's height
// that draws only the lines in view and scrolls inside; the button then
// reads "− 收起". The box's corner opens the whole of it full screen, to
// search and to go to a line. Short text is simply shown.

import { glyph } from "./glyphs";
import { AnchoredHeight, LineViewport, MemoryLines, TextViewer, chineseViewer, englishViewer, type AnchorLayout, type LineSource, type ViewerSource } from "../longview";

export interface LongTextWords {
  more(lines: number): string;
  less: string;
  full: string;
  english: boolean;
}

/** Lines shown before the rest is asked for, and how many more lines are
 *  shown outright rather than behind a button for them. */
const PREVIEW = 12;
const SLACK = 2;

export class LongText {
  readonly element = document.createElement("div");
  private readonly source: LineSource;
  private viewport: LineViewport | null = null;
  private motion: AnchoredHeight | null = null;
  private full = false;
  private readonly more = document.createElement("button");
  private readonly box = document.createElement("div");

  constructor(
    text: string,
    label: string,
    private readonly layout: AnchorLayout,
    private readonly words: LongTextWords,
    /** Further sources for the full-screen view, the whole output among them. */
    others: readonly ViewerSource[] = [],
  ) {
    const lines = text.replace(/\n$/, "").split("\n");
    this.source = new MemoryLines(lines, true);
    this.element.className = "long-text";
    if (lines.length <= PREVIEW + SLACK && !others.length) {
      const pre = document.createElement("pre");
      pre.className = "io-text";
      pre.textContent = lines.join("\n");
      this.element.append(pre);
      return;
    }
    this.box.className = "long-text-box";
    const corner = document.createElement("button");
    corner.type = "button";
    corner.className = "long-text-full";
    corner.title = words.full;
    corner.setAttribute("aria-label", words.full);
    corner.innerHTML = glyph("expand", 15);
    corner.addEventListener("click", () => {
      const viewer = new TextViewer(label, [{ label, source: this.source }, ...others], words.english ? englishViewer : chineseViewer);
      viewer.open(this.viewport?.range.start ?? 0);
    });
    this.viewport = new LineViewport(this.source, { label, limit: PREVIEW, failed: (words.english ? englishViewer : chineseViewer).failed });
    this.box.append(this.viewport.element, corner);
    this.more.type = "button";
    this.more.className = "io-more";
    this.more.addEventListener("click", () => void this.setFull(!this.full));
    this.element.append(this.box, this.more);
    this.motion = new AnchoredHeight(this.box, layout);
    this.label_();
    queueMicrotask(() => this.settle());
  }

  private get length(): number {
    return this.source.extent.length;
  }

  private label_(): void {
    const rest = this.length - PREVIEW;
    this.more.hidden = rest <= 0 && !this.full;
    this.more.textContent = this.full ? this.words.less : this.words.more(rest);
    this.more.setAttribute("aria-expanded", String(this.full));
  }

  private height(): number {
    const row = this.viewport?.rowHeight ?? 0;
    const lines = this.full ? this.length : Math.min(this.length, PREVIEW);
    return Math.round(this.full ? Math.min(this.layout.scroller.clientHeight / 2, lines * row) : lines * row);
  }

  /** Sets the box to the preview's height; called again once it is on the page, where its rows have a height. */
  settle(): void {
    if (!this.viewport) return;
    const height = this.height();
    this.viewport.element.style.height = `${height}px`;
    this.box.style.height = `${height}px`;
  }

  private async setFull(full: boolean): Promise<void> {
    if (!this.viewport || !this.motion || this.full === full) return;
    this.full = full;
    this.label_();
    const target = this.height();
    if (full) {
      this.viewport.setLimit(undefined);
      this.viewport.element.style.height = `${target}px`;
    }
    const result = await this.motion.to(target, this.more);
    if (!full && !result.cancelled && this.viewport) {
      this.viewport.element.style.height = `${target}px`;
      this.viewport.setLimit(PREVIEW);
      this.viewport.setTop(0);
    }
  }

  destroy(): void {
    this.motion?.destroy();
    this.viewport?.destroy();
  }
}
