import type { LineCursor, LineMatch, LineSource } from "./source";
import { LineViewport, type LinePaint } from "./viewport";

export interface ViewerSource { label: string; source: LineSource; paint?: LinePaint; }
export interface ViewerWords {
  close: string; search: string; next: string; previous: string;
  line: string; go: string; searching: string; notFound: string;
  failed: string | ((error: unknown) => string); invalidLine: string;
}

export const englishViewer: ViewerWords = {
  close: "Close", search: "Search", next: "Next", previous: "Previous",
  line: "Line", go: "Go", searching: "Searching…", notFound: "No match",
  failed: "Unable to read content", invalidLine: "Line is outside the content",
};
export const chineseViewer: ViewerWords = {
  close: "关闭", search: "搜索", next: "下一个", previous: "上一个",
  line: "行号", go: "跳转", searching: "搜索中…", notFound: "无匹配",
  failed: "读取失败", invalidLine: "行号超出内容范围",
};

/** Modal viewer. Optional sources distinguish model-visible text from a full
 * output file. Source lifetime belongs to the caller, view lifetime to this dialog. */
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
    const header = document.createElement("header");
    header.className = "lv-viewer-head";
    const name = document.createElement("strong");
    name.textContent = title;
    header.append(name);
    if (sources.length > 1) {
      const select = document.createElement("select");
      select.setAttribute("aria-label", title);
      sources.forEach(({ label }, index) => select.add(new Option(label, String(index))));
      select.addEventListener("change", () => { this.selection = Number(select.value); this.mount(); });
      header.append(select);
    }
    header.append(this.count, this.button(words.close, () => this.close()));
    const toolbar = document.createElement("div");
    toolbar.className = "lv-viewer-tools";
    const search = document.createElement("form");
    search.className = "lv-search-form";
    this.search.type = "search";
    this.search.placeholder = words.search;
    this.search.setAttribute("aria-label", words.search);
    this.search.addEventListener("input", () => {
      this.abort?.abort(); this.match = null; this.message.textContent = "";
      this.viewport?.setMatch(null);
    });
    search.addEventListener("submit", (event) => { event.preventDefault(); void this.find(1); });
    search.append(this.search, this.button(words.previous, () => void this.find(-1)), this.button(words.next, () => void this.find(1)));
    const jump = document.createElement("form");
    jump.className = "lv-jump-form";
    this.line.type = "text";
    this.line.inputMode = "numeric";
    this.line.placeholder = words.line;
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
    const go = document.createElement("button");
    go.textContent = words.go;
    jump.append(this.line, go);
    toolbar.append(search, jump);
    this.body.className = "lv-viewer-body";
    this.message.className = "lv-viewer-message";
    this.message.setAttribute("role", "status");
    this.element.append(header, toolbar, this.message, this.body);
    this.element.addEventListener("close", () => this.dispose());
    this.element.addEventListener("keydown", (event) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault(); this.search.focus();
      }
    });
  }

  private button(label: string, clicked: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = label;
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
    this.viewport = new LineViewport(source, { label: this.title, failed: this.words.failed, ...(paint ? { paint } : {}) });
    this.body.replaceChildren(this.viewport.element);
    const count = (): void => { this.count.textContent = `${source.extent.length.toLocaleString()}${source.extent.complete ? "" : "+"}`; };
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
