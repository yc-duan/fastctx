// Long text under an open tool row (`world-gui.md`, 对话): its first lines,
// then "+ 28 行", nothing scrolling. Lines held here that are not a change
// go on to the next line rather than run off sideways (`wrapped-lines.ts`);
// a change keeps its columns. Pressed, the lines read so far stay
// where they are and the rest comes in below them, the whole of it where it
// fits in most of the conversation's height; only longer text is a box of
// that height that draws the lines in view and scrolls inside, handing the
// wheel on to the conversation at its ends. The button then reads
// "− 收起". The box's corner opens the whole of it in a popup, to
// search and to go to a line, beside any other source of it (a command's
// whole output on its machine). Short text is simply shown. In the
// conversation the lines go unnumbered, unless their painter numbers them
// (a diff's); the popup numbers them.

import { glyph } from "./glyphs";
import { WrappedLines } from "./wrapped-lines";
import {
  AnchoredHeight,
  LineViewport,
  MemoryLines,
  TextViewer,
  chineseViewer,
  englishViewer,
  type AnchorLayout,
  type LinePaint,
  type LineSource,
  type ViewerSource,
  type ViewerWords,
} from "../longview";

export interface LongTextWords {
  more(lines: number): string;
  less: string;
  full: string;
  english: boolean;
  /** Why lines could not be read, said plainly; the error goes on hover. */
  failed?: (error: unknown) => string;
}

/** Another source of the same thing for the popup; it is let go
 *  when the view closes. */
export interface OtherSource extends ViewerSource {
  close?(): void;
}

export interface LongLines {
  /** Lines known here are shown outright when they are few; a source is
   *  always read into its box. */
  lines: readonly string[] | LineSource;
  /** The lines known here as a source, where it searches them as they read
   *  rather than as they are held. */
  source?: LineSource;
  /** Draws a line itself, for lines that say more than their text. */
  paint?: LinePaint;
  /** Draws a line in the conversation, where that is not as `paint` draws
   *  it: a command's output, numbered only in the popup. */
  inline?: LinePaint;
  /** What these lines are called beside the others in the popup. */
  name?: string;
  others?: readonly OtherSource[];
  /** A class the lines' holder takes, for how they read (a diff). */
  kind?: string;
  /** Shown whole in a box that scrolls from the start, as a change is: no
   *  first lines and button for the rest, however few or many. */
  scroll?: boolean;
  /** Lines held here that wrap rather than scroll sideways: all but a
   *  change's. */
  wrap?: boolean;
}

/** Lines shown before the rest is asked for, and how many more lines are
 *  shown outright rather than behind a button for them. */
const PREVIEW = 12;
const SLACK = 2;
/** Lines few enough to be shown outright. */
export const FEW = PREVIEW + SLACK;
/** How much of the conversation's height open text takes at most before
 *  it scrolls in its box. */
const TALLEST = 0.7;
/** How much of it a box that scrolls from the start takes at most. */
const SCROLLING = 0.5;

/** Text opened under a row, as finding and folding reach it: a long text,
 *  or a command's printout (`printout.ts`). */
export interface OpenedText {
  readonly element: HTMLElement;
  /** Brings a line found in it into view. */
  reveal(line: number): Promise<void>;
  /** Sets its height once it is on the page. */
  settle(): void;
  destroy(): void;
}

export class LongText implements OpenedText {
  readonly element = document.createElement("div");
  private readonly source: LineSource;
  private viewport: LineViewport | null = null;
  private wrapped: WrappedLines | null = null;
  private motion: AnchoredHeight | null = null;
  private full = false;
  /** A box that scrolls from the start (`LongLines.scroll`). */
  private readonly scrolling: boolean;
  private readonly more = document.createElement("button");
  private readonly box = document.createElement("div");
  private corner: HTMLButtonElement | null = null;
  private unsubscribe: () => void = () => {};

  constructor(
    content: string | LongLines,
    label: string,
    private readonly layout: AnchorLayout,
    private readonly words: LongTextWords,
  ) {
    const given: LongLines = typeof content === "string" ? { lines: content.replace(/\n$/, "").split("\n"), wrap: true } : content;
    const others = given.others ?? [];
    const lines = Array.isArray(given.lines) ? (given.lines as readonly string[]) : null;
    this.source = given.source ?? (lines ? new MemoryLines(lines, true) : (given.lines as LineSource));
    this.element.className = `long-text${given.kind ? ` ${given.kind}` : ""}`;
    const inline = given.inline ?? given.paint;
    this.scrolling = !!given.scroll;
    if (lines && lines.length <= FEW && !others.length && !this.scrolling && !given.wrap) {
      this.element.append(inline ? painted(lines, inline) : plainText(lines));
      return;
    }
    const viewerWords: ViewerWords = { ...(words.english ? englishViewer : chineseViewer), ...(words.failed ? { failed: words.failed } : {}) };
    if (lines && given.wrap) {
      this.wrapped = new WrappedLines({
        lines,
        ...(inline ? { paint: inline } : {}),
        ...(this.scrolling ? { box: SCROLLING } : { scrolls: true }),
        layout,
        words,
      });
      this.element.append(this.wrapped.clip);
      if (this.wrapped.more) this.element.append(this.wrapped.more);
      if (lines.length <= FEW && !others.length) return;
    }
    this.box.className = "long-text-box";
    const corner = document.createElement("button");
    this.corner = corner;
    corner.type = "button";
    corner.className = "long-text-full";
    corner.title = words.full;
    corner.setAttribute("aria-label", words.full);
    corner.innerHTML = glyph("expand", 15);
    corner.addEventListener("click", () => {
      const first: ViewerSource = { label: given.name ?? label, source: this.source, ...(given.paint ? { paint: given.paint } : {}) };
      const viewer = new TextViewer(label, [first, ...others], viewerWords);
      viewer.element.addEventListener("close", () => {
        for (const other of others) other.close?.();
      });
      viewer.open(this.viewport?.range.start ?? 0);
    });
    if (this.wrapped) {
      this.element.append(corner);
      return;
    }
    this.viewport = new LineViewport(this.source, {
      label,
      ...(this.scrolling ? {} : { limit: PREVIEW }),
      failed: viewerWords.failed,
      interrupted: viewerWords.interrupted,
      numbers: false,
      ...(inline ? { paint: inline } : {}),
    });
    this.box.append(this.viewport.element, corner);
    this.more.type = "button";
    this.more.className = "io-more";
    this.more.addEventListener("click", () => void this.setFull(!this.full));
    this.element.append(this.box, ...(this.scrolling ? [] : [this.more]));
    this.motion = new AnchoredHeight(this.box, layout);
    this.label_();
    // A source read from elsewhere may grow while it is shown.
    if (!lines) {
      this.unsubscribe = this.source.subscribe(() => {
        this.label_();
        if (!this.full || this.scrolling) this.settle();
      });
    }
    queueMicrotask(() => this.settle());
  }

  /** The button for the rest, out of the text, for the caller to put on
   *  the line it shares with what else is said under the text; none where
   *  the text is short enough to be shown whole. */
  takeMore(): HTMLElement | null {
    if (this.wrapped) {
      this.wrapped.more?.remove();
      return this.wrapped.more;
    }
    if (!this.viewport || this.scrolling) return null;
    this.more.remove();
    return this.more;
  }

  /** The button that opens the whole in a popup, out of the box's corner,
   *  for the caller to put on that same line, where it covers no line of
   *  the text. */
  takeFull(): HTMLElement | null {
    if (!this.corner) return null;
    this.corner.remove();
    return this.corner;
  }

  private get length(): number {
    return this.source.extent.length;
  }

  private label_(): void {
    if (this.scrolling) {
      this.more.hidden = true;
      return;
    }
    const rest = this.length - PREVIEW;
    this.more.hidden = rest <= 0 && !this.full;
    this.more.textContent = this.full ? this.words.less : this.words.more(rest);
    this.more.setAttribute("aria-expanded", String(this.full));
  }

  private height(): number {
    const row = this.viewport?.rowHeight ?? 0;
    if (this.scrolling) return Math.round(Math.min(this.layout.scroller.clientHeight * SCROLLING, this.length * row));
    const lines = this.full ? this.length : Math.min(this.length, PREVIEW);
    return Math.round(this.full ? Math.min(this.layout.scroller.clientHeight * TALLEST, lines * row) : lines * row);
  }

  /** Sets the box to the preview's height; called again once it is on the page, where its rows have a height. */
  settle(): void {
    this.wrapped?.settle();
    if (!this.viewport) return;
    const height = this.height();
    this.viewport.element.style.height = `${height}px`;
    this.box.style.height = `${height}px`;
  }

  /** Brings a search hit beyond the preview into the existing line box. */
  async reveal(line: number): Promise<void> {
    if (this.wrapped) return this.wrapped.reveal(line);
    if (!this.viewport) return;
    if (line >= PREVIEW && !this.scrolling) await this.setFull(true);
    this.settle();
    this.viewport.setLimit(this.full || this.scrolling ? undefined : PREVIEW);
    this.viewport.jump(line);
    await this.viewport.ready;
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
    // What the reader looks at stays put: opening, the lines read so far,
    // the rest coming in below; closing, the lines left at the top, unless
    // the box's top is already above the view, where the button under the
    // pointer stays and what is above it closes up.
    const view = this.layout.scroller.getBoundingClientRect();
    const anchor = full || this.box.getBoundingClientRect().top >= view.top ? this.box : this.more;
    const result = await this.motion.to(target, anchor);
    if (!full && !result.cancelled && this.viewport) {
      this.viewport.element.style.height = `${target}px`;
      this.viewport.setLimit(PREVIEW);
      this.viewport.setTop(0);
    }
  }

  /** Every line, read from wherever they are kept. */
  async all(signal: AbortSignal): Promise<readonly string[]> {
    return this.source.read(0, this.length, signal);
  }

  destroy(): void {
    this.wrapped?.destroy();
    this.unsubscribe();
    this.motion?.destroy();
    this.viewport?.destroy();
  }
}

function plainText(lines: readonly string[]): HTMLElement {
  const pre = document.createElement("pre");
  pre.className = "io-text";
  pre.textContent = lines.join("\n");
  return pre;
}

/** Few lines drawn by their painter, in the page's flow: they wrap rather
 *  than scroll. */
function painted(lines: readonly string[], paint: LinePaint): HTMLElement {
  const holder = document.createElement("div");
  holder.className = "long-static";
  lines.forEach((line, index) => {
    const row = document.createElement("div");
    row.className = "lv-line";
    holder.append(row);
    paint(row, line, index, null);
  });
  return holder;
}
