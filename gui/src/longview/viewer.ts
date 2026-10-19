import { glyph } from "../conversation/glyphs";
import { onShade, watchShade } from "../app/shade-press";
import type { LineCursor, LineMatch, LineSource } from "./source";
import { LineViewport, type LinePaint } from "./viewport";

export interface ViewerSource { label: string; source: LineSource; paint?: LinePaint; }
export interface ViewerWords {
  close: string; search: string; next: string; previous: string;
  line: string; go: string; searching: string; notFound: string;
  failed: string | ((error: unknown) => string); invalidLine: string;
  /** The way to the lines broke; they come back once it is open. */
  interrupted: string;
  /** How many lines there are, the count already written out. */
  lines(count: string): string;
}

export const englishViewer: ViewerWords = {
  close: "Close", search: "Search", next: "Next", previous: "Previous",
  line: "Line", go: "Go", searching: "Searching…", notFound: "No match",
  failed: "Unable to read content", invalidLine: "Line is outside the content",
  interrupted: "Disconnected, reconnecting…", lines: (count) => `${count} lines`,
};
export const chineseViewer: ViewerWords = {
  close: "关闭", search: "搜索", next: "下一个", previous: "上一个",
  line: "行号", go: "跳转", searching: "搜索中…", notFound: "无匹配",
  failed: "读取失败", invalidLine: "行号超出内容范围",
  interrupted: "连接中断，正在重连…", lines: (count) => `${count} 行`,
};

/** Modal viewer: a card over the page, of a size of its own, whose lines
 * scroll inside it; on a phone it rises from the foot of the screen. Optional
 * sources distinguish model-visible text from a full output file. Source
 * lifetime belongs to the caller, view lifetime to this dialog. */
export class TextViewer {
  readonly element = document.createElement("dialog");
  viewport: LineViewport | null = null;
  private abort: AbortController | null = null;
  private selection = 0;
  private match: LineMatch | null = null;
  private lastQuery = "";
  private search = document.createElement("input");
  private line = document.createElement("input");
  private message = document.createElement("span");
  private body = document.createElement("div");
  private count = document.createElement("span");
  private unsubscribe: (() => void) | null = null;
  private returnFocus: HTMLElement | null = null;

  constructor(private title: string, private sources: readonly ViewerSource[], private words = englishViewer) {
    if (!sources.length) throw new Error("viewer needs a source");
    this.element.className = "lv-viewer";
    this.element.setAttribute("aria-label", title);
    const card = document.createElement("div");
    card.className = "lv-card";
    // The head: what it is, how long, which of its sources, and the way out.
    const header = document.createElement("header");
    header.className = "lv-viewer-head";
    const name = document.createElement("strong");
    name.textContent = title;
    name.title = title;
    this.count.className = "lv-viewer-count";
    header.append(name, this.count);
    if (sources.length > 1) {
      const choices = document.createElement("div");
      choices.className = "lv-viewer-sources";
      choices.setAttribute("role", "group");
      choices.setAttribute("aria-label", title);
      const pills = sources.map(({ label }, index) => {
        const pill = document.createElement("button");
        pill.type = "button";
        pill.textContent = label;
        pill.setAttribute("aria-pressed", String(index === 0));
        pill.addEventListener("click", () => {
          if (this.selection === index) return;
          this.selection = index;
          for (const [at, one] of pills.entries()) one.setAttribute("aria-pressed", String(at === index));
          this.mount();
        });
        return pill;
      });
      choices.append(...pills);
      header.append(choices);
    }
    const close = this.icon("close", words.close, () => this.close());
    close.classList.add("lv-viewer-close");
    header.append(close);
    // Beneath it, finding in it: a field with its two ways to go, and a line
    // to go to.
    const toolbar = document.createElement("div");
    toolbar.className = "lv-viewer-tools";
    const search = document.createElement("form");
    search.className = "lv-search-form";
    search.insertAdjacentHTML("afterbegin", glyph("search", 14));
    this.search.type = "search";
    this.search.placeholder = words.search;
    this.search.enterKeyHint = "search";
    this.search.setAttribute("aria-label", words.search);
    this.search.addEventListener("input", () => {
      this.abort?.abort(); this.match = null; this.message.textContent = "";
      this.viewport?.setMatch(null);
    });
    search.addEventListener("submit", (event) => { event.preventDefault(); void this.find(1); });
    const previous = this.icon("chevron", words.previous, () => void this.find(-1));
    previous.classList.add("lv-previous");
    search.append(this.search, previous, this.icon("chevron", words.next, () => void this.find(1)));
    const jump = document.createElement("form");
    jump.className = "lv-jump-form";
    this.line.type = "text";
    this.line.inputMode = "numeric";
    this.line.enterKeyHint = "go";
    this.line.placeholder = words.line;
    this.line.title = words.go;
    this.line.setAttribute("aria-label", words.line);
    jump.addEventListener("submit", (event) => {
      event.preventDefault();
      const value = Number(this.line.value);
      if (!/^\d+$/.test(this.line.value) || !Number.isSafeInteger(value) || value < 1 || value > this.sources[this.selection].source.extent.length) {
        this.message.textContent = words.invalidLine; return;
      }
      this.viewport?.jump(value - 1);
      this.viewport?.element.focus({ preventScroll: true });
      this.message.textContent = "";
    });
    jump.append(this.line);
    this.body.className = "lv-viewer-body";
    this.message.className = "lv-viewer-message";
    this.message.setAttribute("role", "status");
    toolbar.append(search, jump, this.message);
    card.append(header, toolbar, this.body);
    this.element.append(card);
    this.element.addEventListener("close", () => this.dispose());
    // A press on the shade around the card puts it away.
    watchShade(this.element);
    this.element.addEventListener("click", (event) => {
      if (onShade(this.element, event)) this.close();
    });
    this.element.addEventListener("keydown", (event) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault(); this.search.focus();
      }
    });
  }

  private icon(name: Parameters<typeof glyph>[0], label: string, clicked: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "icon-button";
    button.title = label;
    button.setAttribute("aria-label", label);
    button.innerHTML = glyph(name, 16);
    button.addEventListener("click", clicked);
    return button;
  }

  open(line = 0): void {
    this.returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    document.body.append(this.element);
    this.element.showModal();
    this.mount();
    requestAnimationFrame(() => {
      if (this.viewport && this.viewport.length > 0) this.viewport.jump(Math.min(line, this.viewport.length - 1));
    });
  }

  private mount(): void {
    this.abort?.abort();
    this.unsubscribe?.();
    this.viewport?.destroy();
    this.match = null;
    this.message.textContent = "";
    const { source, paint } = this.sources[this.selection];
    this.viewport = new LineViewport(source, { label: this.title, failed: this.words.failed, interrupted: this.words.interrupted, ...(paint ? { paint } : {}) });
    this.body.replaceChildren(this.viewport.element);
    const count = (): void => { this.count.textContent = this.words.lines(`${source.extent.length.toLocaleString()}${source.extent.complete ? "" : "+"}`); };
    count();
    this.unsubscribe = source.subscribe(({ from }) => {
      count();
      this.abort?.abort();
      this.message.textContent = "";
      if (this.match && this.match.line >= from) {
        this.match = null;
        this.viewport?.setMatch(null);
      }
    });
  }

  private async find(direction: 1 | -1): Promise<void> {
    this.abort?.abort();
    const query = this.search.value;
    if (!query) return;
    if (query !== this.lastQuery) this.match = null;
    this.lastQuery = query;
    const request = new AbortController();
    this.abort = request;
    this.message.textContent = this.words.searching;
    const source = this.sources[this.selection].source;
    const from: LineCursor = this.match
      ? direction === 1 ? { line: this.match.line, offset: this.match.start + 1 }
        : this.match.start > 0 ? { line: this.match.line, offset: this.match.start - 1 } : { line: this.match.line - 1 }
      : { line: direction === 1 ? 0 : source.extent.length - 1 };
    try {
      const found = await source.find(query, from, direction, request.signal);
      if (request.signal.aborted) return;
      this.match = found;
      this.message.textContent = found ? `${this.words.line} ${found.line + 1}` : this.words.notFound;
      if (found) this.viewport?.jump(found.line, found);
      else this.viewport?.setMatch(null);
    } catch (error: unknown) {
      if (request.signal.aborted) return;
      this.message.textContent = typeof this.words.failed === "function" ? this.words.failed(error) : this.words.failed;
      this.message.title = String(error);
    }
  }

  close(): void { this.element.close(); }

  private dispose(): void {
    this.abort?.abort();
    this.unsubscribe?.();
    this.viewport?.destroy();
    this.element.remove();
    this.returnFocus?.focus({ preventScroll: true });
  }
}
