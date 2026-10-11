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
 *  `chars` how many of its characters a line of the column holds, `acts`
 *  the room a turn's actions take beneath it where it has them. */
export function estimate(block: Block, line: number, chars: number, acts: number): number {
  const wrapped = (row: string, width: number) => {
    // A wide character takes about two narrow ones' room.
    const span = [...row].reduce((n, char) => n + (char.charCodeAt(0) > 0x2e80 ? 2 : 1), 0);
    return Math.max(1, Math.ceil(span / width));
  };
  // Prose by its lines: a blank one is a paragraph's margin, a line of code
  // is in the smaller code face, a fence is the block's padding.
  const prose = (text: string, width: number) => {
    let height = 0;
    let code = false;
    for (const row of text.split("\n")) {
      if (/^\s*(```|~~~)/.test(row)) {
        code = !code;
        height += line * 0.7;
      } else if (code) height += wrapped(row, Math.floor(width * 0.95)) * line * 0.8;
      else if (!row.trim()) height += line * 0.6;
      else height += wrapped(row, width) * line;
    }
    return height;
  };
  switch (block.kind) {
    case "activity-group":
      return 28;
    case "heard":
      if (block.notice) return 24;
      return prose(block.text, Math.floor(chars * 0.78)) + 20 + (block.attached?.length ? 120 : 0) + acts;
    case "prose":
      return prose(block.text, chars) + acts;
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
