// Finding words in a conversation, as a browser's find does: a bar under
// the head, every match washed, the current one stronger, Enter and
// Shift+Enter between them. Matches are marked with the CSS Custom
// Highlight API, so the conversation's own elements are never touched and
// a stream of words keeps flowing while it is open.

import { glyph } from "./glyphs";
import { escape } from "./prose";

export interface FindWords {
  find: string;
  placeholder: string;
  count(current: number, total: number): string;
  none: string;
  previous: string;
  next: string;
  close: string;
}

export class Finder {
  readonly element: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly count: HTMLElement;
  private matches: Range[] = [];
  private current = -1;

  constructor(
    private readonly transcript: HTMLElement,
    private readonly scroller: HTMLElement,
    private readonly words: FindWords,
    /** The reader moved through the conversation by finding. */
    private readonly moved: () => void,
  ) {
    this.element = document.createElement("div");
    this.element.className = "find-bar";
    this.element.hidden = true;
    this.element.setAttribute("role", "search");
    this.element.innerHTML = `<span class="find-glyph">${glyph("search", 16)}</span><input class="find-input" type="search" enterkeyhint="search" placeholder="${escape(words.placeholder)}" aria-label="${escape(words.find)}"><span class="find-count" aria-live="polite"></span><button class="icon-button find-step" type="button" data-step="-1" aria-label="${escape(words.previous)}" title="${escape(words.previous)}">${glyph("chevron", 16)}</button><button class="icon-button find-step find-next" type="button" data-step="1" aria-label="${escape(words.next)}" title="${escape(words.next)}">${glyph("chevron", 16)}</button><button class="icon-button find-close" type="button" aria-label="${escape(words.close)}" title="${escape(words.close)}">${glyph("close", 16)}</button>`;
    this.input = this.element.querySelector<HTMLInputElement>(".find-input")!;
    this.count = this.element.querySelector<HTMLElement>(".find-count")!;
    this.input.addEventListener("input", () => this.search(true));
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.isComposing) {
        event.preventDefault();
        this.step(event.shiftKey ? -1 : 1);
      }
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      this.close();
    });
    this.element.addEventListener("click", (event) => {
      const target = (event.target as HTMLElement).closest<HTMLElement>("[data-step], .find-close");
      if (!target) return;
      if (target.classList.contains("find-close")) this.close();
      else this.step(Number(target.dataset.step));
    });
  }

  /** What had focus before the bar opened. */
  private before: HTMLElement | null = null;

  get open(): boolean {
    return !this.element.hidden;
  }

  show(): void {
    if (this.element.hidden && document.activeElement instanceof HTMLElement) this.before = document.activeElement;
    this.element.hidden = false;
    // What is selected is what is looked for, as browsers do.
    const selected = window.getSelection()?.toString().trim();
    if (selected && !selected.includes("\n")) this.input.value = selected;
    this.input.focus();
    this.input.select();
    this.search(true);
  }

  close(): void {
    const inside = this.element.contains(document.activeElement);
    this.element.hidden = true;
    if (inside && this.before?.isConnected) this.before.focus({ preventScroll: true });
    this.before = null;
    this.matches = [];
    this.current = -1;
    this.paint();
  }

  /** The conversation changed while open: matches are found again, the
   *  current one kept where it still is. */
  refresh(): void {
    if (this.open) this.search(false);
  }

  private search(fresh: boolean): void {
    const query = this.input.value.toLocaleLowerCase();
    const previous = this.matches[this.current];
    this.matches = query ? ranges(this.transcript, query) : [];
    if (fresh || !previous) {
      // From the newest match upward, the way a conversation is read back.
      this.current = this.matches.length - 1;
    } else {
      const same = this.matches.findIndex((range) => range.compareBoundaryPoints(Range.START_TO_START, previous) === 0);
      this.current = same >= 0 ? same : Math.min(this.current, this.matches.length - 1);
    }
    this.paint();
    if (fresh) this.reveal();
  }

  private step(by: number): void {
    if (!this.matches.length) return;
    this.current = (this.current + by + this.matches.length) % this.matches.length;
    this.paint();
    this.reveal();
  }

  private paint(): void {
    const highlights = (CSS as unknown as { highlights?: Map<string, unknown> }).highlights;
    const Highlight = (window as unknown as { Highlight?: new (...ranges: Range[]) => unknown }).Highlight;
    if (highlights && Highlight) {
      const current = this.matches[this.current];
      highlights.set("find", new Highlight(...this.matches.filter((range) => range !== current)));
      highlights.set("find-current", current ? new Highlight(current) : new Highlight());
    }
    const query = this.input.value;
    this.count.textContent = !query ? "" : this.matches.length ? this.words.count(this.current + 1, this.matches.length) : this.words.none;
    this.count.classList.toggle("none", !!query && !this.matches.length);
  }

  /** Brings the current match into view, opening what folds it away. */
  private reveal(): void {
    const range = this.matches[this.current];
    if (!range) return;
    let node: Node | null = range.startContainer;
    while (node && node !== this.transcript) {
      if (node instanceof HTMLElement && node.classList.contains("fold") && !node.classList.contains("open")) {
        (node.previousElementSibling as HTMLElement | null)?.click();
      }
      node = node.parentNode;
    }
    this.moved();
    const box = range.getBoundingClientRect();
    const view = this.scroller.getBoundingClientRect();
    if (box.top < view.top + 48 || box.bottom > view.bottom - 48) {
      this.scroller.scrollTop += box.top - view.top - view.height / 3;
    }
  }
}

/** Every place the query occurs in the text a reader sees, ignoring case.
 *  Each block's text is read as one, so a match may cross the edges of
 *  inline code or emphasis, but never runs from one block into the next. */
function ranges(root: HTMLElement, query: string): Range[] {
  const found: Range[] = [];
  for (const block of root.querySelectorAll<HTMLElement>(":scope > .block")) {
    const nodes: { node: Text; from: number }[] = [];
    let text = "";
    const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) =>
        node.parentElement?.closest("button, svg, .katex-mathml, [aria-hidden=true]") && !node.parentElement.closest(".row-line, .group-head")
          ? NodeFilter.FILTER_REJECT
          : NodeFilter.FILTER_ACCEPT,
    });
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      nodes.push({ node: node as Text, from: text.length });
      // Offsets must stay the node's own: where lower case changes a
      // text's length (a dotted İ), that text is matched as written.
      const raw = node.nodeValue ?? "";
      const lower = raw.toLocaleLowerCase();
      text += lower.length === raw.length ? lower : raw;
    }
    const place = (offset: number, end: boolean): [Text, number] => {
      // The node holding this offset; an end at a node's edge stays in it.
      let index = nodes.length - 1;
      while (index > 0 && (end ? nodes[index]!.from >= offset : nodes[index]!.from > offset)) index -= 1;
      const { node, from } = nodes[index]!;
      return [node, offset - from];
    };
    for (let at = text.indexOf(query); at >= 0; at = text.indexOf(query, at + query.length)) {
      const range = document.createRange();
      range.setStart(...place(at, false));
      range.setEnd(...place(at + query.length, true));
      found.push(range);
    }
  }
  return found;
}
