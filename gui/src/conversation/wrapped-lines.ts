// Lines held in the page (a command's printout, a file read, what a tool
// gave back) set in the page's flow, each going on to the next line rather
// than running off sideways (`world-gui.md`, 对话). Few, they are simply
// shown. Many, they show their first few and a button that says how many
// more; pressed, the rest comes in below what was read, the whole of it
// where it fits in most of the conversation's height, and only past that a
// box of that height that scrolls, handing the wheel on to the page at its
// ends. Or, for a file's lines, the whole from the start in a box that
// scrolls past half the conversation's height. It is what finding reaches
// of them: its rows are the lines, in order.

import "./wrapped-lines.css";
import type { OpenedText } from "./long-text";
import { handOnAtEdge } from "../scroll-handoff";
import { AnchoredHeight, type AnchorLayout, type LinePaint } from "../longview";

export interface WrappedWords {
  /** The button for the rest: "+ 38 行". */
  more(lines: number): string;
  less: string;
}

export interface WrappedOptions {
  lines: readonly string[];
  /** Draws a line, where it says more than its text. */
  paint?: LinePaint;
  /** How many lines are shown outright, and how many before the button
   *  where there are more than that. */
  few?: number;
  shown?: number;
  /** Opened past the conversation's height, it scrolls rather than grows. */
  scrolls?: boolean;
  /** No button: the whole from the start, in a box that scrolls past this
   *  share of the conversation's height. */
  box?: number;
  /** A class the lines take, for how they read. */
  name?: string;
  layout: AnchorLayout;
  words: WrappedWords;
}

/** How much of the conversation's height opened lines take at most before
 *  they scroll in their own box. */
const TALLEST = 0.7;

export class WrappedLines implements OpenedText {
  readonly element = document.createElement("div");
  /** The box over the lines: it closes over those past the first few, and
   *  scrolls where the whole is too tall to show. */
  readonly clip = document.createElement("div");
  /** The button for the rest, for the caller to put where it says it. */
  readonly more: HTMLButtonElement | null = null;
  private opened = false;
  private motion: AnchoredHeight | null = null;
  private readonly shown: number;

  constructor(private readonly options: WrappedOptions) {
    const { lines, paint, few = 14, box } = options;
    this.shown = options.shown ?? 12;
    const many = box === undefined && lines.length > few;
    this.element.className = `long-static wrapped-lines${options.name ? ` ${options.name}` : ""}${many ? " wrapped-shut" : ""}`;
    this.clip.className = "wrapped-clip";
    this.clip.append(this.element);
    lines.forEach((line, index) => {
      const row = document.createElement("div");
      row.className = many && index >= this.shown ? "lv-line wrapped-past" : "lv-line";
      this.element.append(row);
      if (paint) paint(row, line, index, null);
      else {
        const text = document.createElement("span");
        text.className = "lv-text";
        text.textContent = line;
        row.append(text);
      }
    });
    if (box !== undefined) {
      this.clip.classList.add("wrapped-scroll");
      handOnAtEdge(this.clip);
      this.settle();
      return;
    }
    if (!many) return;
    const more = document.createElement("button");
    more.type = "button";
    more.className = "wrapped-more";
    more.addEventListener("click", () => void this.toggle(!this.opened));
    this.more = more;
    this.say();
  }

  private say(): void {
    if (!this.more) return;
    this.more.textContent = this.opened ? this.options.words.less : this.options.words.more(this.element.childElementCount - this.shown);
    this.more.setAttribute("aria-expanded", String(this.opened));
  }

  /** Opens or closes over the lines past the first few. What the reader
   *  looks at stays put: opening, the lines read so far, the rest coming in
   *  below them; closing, the lines left at the top, unless the box's top
   *  is already above the view, where the button under the pointer stays
   *  and what is above it closes up. */
  private async toggle(open: boolean): Promise<void> {
    if (!this.more || this.opened === open) return;
    this.opened = open;
    this.say();
    const { layout } = this.options;
    this.motion ??= new AnchoredHeight(this.clip, layout);
    const from = this.clip.getBoundingClientRect().height;
    this.clip.style.height = `${from}px`;
    if (open) {
      this.element.classList.remove("wrapped-shut");
      const whole = this.element.offsetHeight;
      const most = this.options.scrolls ? Math.max(from, Math.round(layout.scroller.clientHeight * TALLEST)) : Infinity;
      const target = Math.min(whole, most);
      const done = await this.motion.to(target, this.clip);
      if (done.cancelled) return;
      if (whole > target) {
        this.clip.classList.add("wrapped-scroll");
        handOnAtEdge(this.clip);
      } else this.clip.style.height = "";
      return;
    }
    const view = layout.scroller.getBoundingClientRect();
    const anchor = this.clip.getBoundingClientRect().top >= view.top ? this.clip : this.more;
    this.clip.classList.remove("wrapped-scroll");
    this.clip.scrollTop = 0;
    const past = this.element.querySelector<HTMLElement>(".wrapped-past");
    const done = await this.motion.to(past ? past.offsetTop : this.element.offsetHeight, anchor);
    if (done.cancelled) return;
    this.element.classList.add("wrapped-shut");
    this.clip.style.height = "";
  }

  /** Brings line `line` out of the fold, and into the box's view where it
   *  scrolls, as finding goes to it. */
  async reveal(line: number): Promise<void> {
    if (this.more && !this.opened && line >= this.shown) await this.toggle(true);
    const row = this.element.children[line] as HTMLElement | undefined;
    if (row && this.clip.classList.contains("wrapped-scroll")) this.clip.scrollTop = Math.max(0, row.offsetTop - this.clip.clientHeight / 3);
  }

  /** A box from the start is as tall as its share of the conversation. */
  settle(): void {
    const { box, layout } = this.options;
    if (box === undefined) return;
    const height = layout.scroller.clientHeight;
    if (height) this.clip.style.maxHeight = `${Math.round(height * box)}px`;
  }

  destroy(): void {
    this.motion?.destroy();
  }
}
