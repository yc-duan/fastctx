// What a command did, as the conversation shows it under the row that ran
// it (`world-gui.md`, 对话): one card, the command as it was given above a
// drawn rule and what it printed below, both in the code face and both
// going on to the next line rather than running off sideways. Many lines
// show their first few and, at the foot, how many more there are; pressed,
// the rest comes in below what was read, the whole of it where it fits in
// most of the conversation's height, and only past that a box of that
// height that scrolls. A copy of what it printed, and the reader that opens
// the whole numbered and searchable beside the whole output on its machine
// and the script, wait at the end of the command's line for the pointer. A
// command that did not end well says so at the foot, in the colour of a
// fault; one that did says nothing.

import "./printout.css";
import { glyph } from "./glyphs";
import { escape } from "./prose";
import { drops } from "./deeds/marks";
import type { OpenedText } from "./long-text";
import { WrappedLines } from "./wrapped-lines";
import { MemoryLines, TextViewer, chineseViewer, englishViewer, type AnchorLayout, type LinePaint, type ViewerSource, type ViewerWords } from "../longview";

export interface PrintoutWords {
  /** The fold's button: "+ 38 行". */
  more(lines: number): string;
  less: string;
  copy: string;
  copied: string;
  /** The whole, opened in the reader. */
  whole: string;
  english: boolean;
  failed?: (error: unknown) => string;
}

/** How the command ended, where that is worth a word. */
export type PrintoutEnd = { kind: "failed"; said: string } | { kind: "stopped"; said: string } | { kind: "said"; said: string };

export interface PrintoutOptions {
  /** What it is the output of, as the reader names it. */
  label: string;
  /** The command as it was given, line by line; a call given nothing
   *  worth showing has none, and its card is only what came back. */
  command: readonly string[];
  lines: readonly string[];
  /** The lines that are the World's own word in place of what was left
   *  out, by their place among `lines`: drawn by `paint`, not copied. */
  notes?: ReadonlySet<number>;
  /** How a line is drawn in the card, where it is not simply its text. */
  paint?: LinePaint;
  /** How the reader draws and numbers them, where they are not simply the
   *  output's from its first. */
  reading?: LinePaint;
  /** Said, quietly, where nothing was printed; empty says nothing. */
  nothing: string;
  end?: PrintoutEnd | null;
  /** Pictures it gave, as markup, after its lines. */
  pictures?: string;
  /** What the reader opens besides these lines: the whole output on its
   *  machine, the script as given. */
  others?: ViewerSource[];
  layout: AnchorLayout;
  words: PrintoutWords;
}

/** A script that is only an `on` with its commands for one machine: the
 *  machine said by the row, the card shows the commands. */
const ONLY_ON = /^(?:fastctx\s+)?on\s+\S+\s+<<-?\s*(['"]?)([A-Za-z_]\w*)\1[ \t]*\n([\s\S]*?)\n\2[ \t]*$/;

/** The command a card shows, line by line: the script as it was given,
 *  but for an `on` that is the whole of it, its commands alone. */
export function givenLines(script: string): string[] {
  const given = script.replace(/\n+$/, "");
  const body = ONLY_ON.exec(given)?.[3];
  return (body?.trim() ? body : given).split("\n");
}

/** How many lines of a command are shown outright, and how many before
 *  its button where there are more; an output's are the page's own. */
const COMMAND = { few: 8, shown: 6 };

export class Printout {
  readonly element = document.createElement("div");
  /** The command's lines and then what it printed, as finding reaches them. */
  readonly sides: OpenedText[] = [];
  private readonly parts: WrappedLines[] = [];

  constructor(private readonly options: PrintoutOptions) {
    const { command, lines, words, layout } = options;
    this.element.className = "po";
    const printed = lines.some((_, at) => !options.notes?.has(at));
    const out = document.createElement("div");
    out.className = "po-out";
    let output: WrappedLines | null = null;
    if (lines.length) {
      output = new WrappedLines({ lines, ...(options.paint ? { paint: options.paint } : {}), scrolls: true, layout, words, name: "po-printed" });
      out.append(output.clip);
    } else if (!options.pictures && options.nothing) {
      out.insertAdjacentHTML("beforeend", `<p class="po-nothing">${escape(options.nothing)}</p>`);
    }
    if (options.pictures) out.insertAdjacentHTML("beforeend", `<div class="po-pictures">${options.pictures}</div>`);
    const head = document.createElement("div");
    head.className = "po-in";
    let given: WrappedLines | null = null;
    if (command.length) {
      given = new WrappedLines({ lines: command, ...COMMAND, layout, words, name: "po-command" });
      const text = document.createElement("div");
      text.className = "po-given";
      text.append(given.clip);
      if (given.more) text.append(given.more);
      head.append(text);
    } else {
      // Nothing given to say above it: what came back is the card's first
      // line, its corner at that line's end.
      head.append(out);
    }
    // The corner is for lines a program printed.
    if (printed) {
      head.insertAdjacentHTML(
        "beforeend",
        `<div class="po-tools"><button class="po-tool" type="button" data-po="copy" title="${escape(words.copy)}" aria-label="${escape(words.copy)}">${glyph("copy", 15)}</button><button class="po-tool" type="button" data-po="whole" title="${escape(words.whole)}" aria-label="${escape(words.whole)}">${glyph("expand", 15)}</button></div>`,
      );
    }
    this.element.append(head);
    if (given && out.firstChild) {
      this.element.insertAdjacentHTML("beforeend", `<div class="po-rule" aria-hidden="true"></div>`);
      this.element.append(out);
    }
    const foot = document.createElement("div");
    foot.className = "po-foot";
    if (output?.more) foot.append(output.more);
    const end = options.end;
    if (end) {
      const mark = end.kind === "failed" ? drops(end.said, 1, "wrong") : end.kind === "stopped" ? `<span class="po-stop" aria-hidden="true"></span>` : "";
      foot.insertAdjacentHTML("beforeend", `<span class="po-end ${end.kind}">${mark}<span>${escape(end.said)}</span></span>`);
    }
    if (foot.childElementCount) this.element.append(foot);
    this.parts.push(...(given ? [given] : []), ...(output ? [output] : []));
    this.sides.push(...this.parts);
    this.element.addEventListener("click", (event) => {
      const tool = (event.target as HTMLElement).closest<HTMLElement>("[data-po]");
      if (!tool) return;
      if (tool.dataset.po === "copy") void this.copy(tool);
      else this.open();
    });
  }

  private async copy(button: HTMLElement): Promise<void> {
    const { lines, notes, words } = this.options;
    try {
      await navigator.clipboard.writeText(lines.filter((_, at) => !notes?.has(at)).join("\n"));
    } catch {
      return;
    }
    button.innerHTML = glyph("check", 15);
    button.title = words.copied;
    window.setTimeout(() => {
      button.innerHTML = glyph("copy", 15);
      button.title = words.copy;
    }, 1600);
  }

  /** The whole in the reader, numbered, to search and to go to a line,
   *  beside what else there is of it. */
  private open(): void {
    const { words, label, lines, reading, others = [] } = this.options;
    const viewerWords: ViewerWords = { ...(words.english ? englishViewer : chineseViewer), ...(words.failed ? { failed: words.failed } : {}) };
    const first: ViewerSource = { label, source: new MemoryLines(lines, true), ...(reading ? { paint: reading } : {}) };
    new TextViewer(label, [first, ...others], viewerWords).open(0);
  }

  destroy(): void {
    for (const part of this.parts) part.destroy();
  }
}
