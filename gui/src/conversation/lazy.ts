// A conversation draws only what is near the reader (`world-gui.md`, 对话):
// every block keeps its place in the page as an element of its own, but
// what it shows is made only when it comes within a screen of the view,
// and let go again once it is several screens away, its height kept so
// nothing moves. A conversation of thousands of tool calls is then as
// light to read, scroll and stream into as a short one.
//
// The distances are the scroll region's own height: a block is drawn a
// whole screen before it can be seen, so a reader scrolling at any pace a
// hand or wheel gives never meets an undrawn one; one let go is three
// screens away, far enough that turning back and forth never draws the same
// block over and over.

import type { Block } from "./model";

export interface LazySeams {
  /** The block at `element` is near the view: draw it. */
  near(element: HTMLElement): void;
  /** The block at `element` is far from the view: let it go if it can be,
   *  keeping `height`. */
  far(element: HTMLElement, height: number): void;
}

export class Lazy {
  private readonly nearby: IntersectionObserver;
  private readonly distant: IntersectionObserver;

  constructor(scroller: HTMLElement, seams: LazySeams) {
    this.nearby = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) if (entry.isIntersecting) seams.near(entry.target as HTMLElement);
      },
      { root: scroller, rootMargin: "100% 0px" },
    );
    this.distant = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) if (!entry.isIntersecting) seams.far(entry.target as HTMLElement, entry.boundingClientRect.height);
      },
      { root: scroller, rootMargin: "300% 0px" },
    );
  }

  watch(element: HTMLElement): void {
    this.nearby.observe(element);
    this.distant.observe(element);
  }

  unwatch(element: HTMLElement): void {
    this.nearby.unobserve(element);
    this.distant.unobserve(element);
  }

  close(): void {
    this.nearby.disconnect();
    this.distant.disconnect();
  }
}

/** How tall a block not yet drawn is taken to be, from what it holds and
 *  the column's width: near enough that the scroll bar does not lie, and
 *  set right the moment it is drawn. `line` is the body's line height,
 *  `chars` how many of its characters a line of the column holds. */
export function estimate(block: Block, line: number, chars: number): number {
  const lines = (text: string, width = chars) =>
    text.split("\n").reduce((sum, row) => {
      // A wide character takes about two narrow ones' room.
      const span = [...row].reduce((n, char) => n + (char.charCodeAt(0) > 0x2e80 ? 2 : 1), 0);
      return sum + Math.max(1, Math.ceil(span / width));
    }, 0);
  switch (block.kind) {
    case "activity-group":
      return 28;
    case "heard":
      if (block.notice) return 24;
      return lines(block.text, Math.floor(chars * 0.78)) * line + 20 + (block.attached?.length ? 120 : 0);
    case "prose": {
      const paragraphs = block.text.split(/\n{2,}/).length;
      return lines(block.text) * line + paragraphs * line * 0.6;
    }
    case "ended":
      return block.end.how === "completed" ? 0 : line;
    case "steps":
      return block.steps.length * line;
    case "ask":
    case "question":
      return 160;
    default:
      return line;
  }
}
