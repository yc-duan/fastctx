// How tall a block not drawn is taken to be (`view.ts`): near enough that
// the scroll bar does not lie, and set right the moment it is drawn.

import type { Block } from "./model";

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
    case "agent-told":
      return 24;
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
