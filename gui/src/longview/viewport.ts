import type { LineInterruption, LineMatch, LineSource } from "./source";
import { chainTouch } from "./touch";
import { handOnAtEdge } from "../scroll-handoff";

/** Draws one row from its line, for lines that carry more than their text
 *  (a diff row's kind and numbers, an output line's place in the whole).
 *  The row comes bare, its class reset to `lv-line`; `match` holds UTF-16
 *  offsets into `line` to mark, and a `mark` the painter puts in the row is
 *  scrolled into view. */
export type LinePaint = (row: HTMLElement, line: string, index: number, match: { start: number; end: number } | null) => void;

export interface LineViewportOptions {
  label: string;
  /** Limits a preview without changing the source's identity. */
  limit?: number;
  follow?: boolean;
  loading?: string;
  failed?: string | ((error: unknown) => string);
  /** Said while the way to the lines is broken, why on hover. */
  interrupted?: string;
  paint?: LinePaint;
  /** Unpainted lines are numbered, unless this is false. */
  numbers?: boolean;
}

/** Fixed-height logical lines; native horizontal scrolling preserves shaping,
 * tabs, CJK, emoji, and long lines without guessing character widths.
 * DOM and cache contain only the current visible range. No whole-source scan.
 * CSS scroll-height saturation is measured and mapped to logical positions. */
export class LineViewport {
  readonly element = document.createElement("div");
  readonly track = document.createElement("div");
  readonly status = document.createElement("div");
  private rows = new Map<number, HTMLElement>();
  private cache = new Map<number, string>();
  private painted = new WeakMap<HTMLElement, { value: string; match: string }>();
  private request: AbortController | null = null;
  private unsubscribe: () => void;
  private observer: ResizeObserver;
  private lineHeight = 0;
  private width = 0;
  private maximumWidth = 0;
  private first = 0;
  private end = 0;
  private destroyed = false;
  private match: LineMatch | null = null;
  private lastTop = 0;
  private atEnd = true;
  private pending: Promise<void> = Promise.resolve();
  private requestRange: [number, number] | null = null;
  private unchain: () => void;
  /** What the status says of the reads, and why, under a broken way. */
  private said = { text: "", why: "" };
  private broken: LineInterruption | null = null;
  private unhear: () => void;
  /** The frame on which the lines in view are looked at again after a read. */
  private again = 0;

  constructor(readonly source: LineSource, private options: LineViewportOptions) {
    this.element.className = "lv-viewport";
    this.element.tabIndex = 0;
    this.element.setAttribute("role", "region");
    this.element.setAttribute("aria-label", options.label);
    this.track.className = "lv-track";
    this.status.className = "lv-status";
    this.status.setAttribute("role", "status");
    this.element.append(this.track, this.status);
    this.element.addEventListener("scroll", this.scroll, { passive: true });
    this.element.addEventListener("wheel", this.wheel, { passive: false });
    handOnAtEdge(this.element);
    this.unchain = chainTouch(this);
    this.unsubscribe = source.subscribe(({ from }) => {
      if (this.match && this.match.line >= from) this.match = null;
      for (const index of this.cache.keys()) if (index >= from) this.cache.delete(index);
      this.request?.abort();
      this.requestRange = null;
      this.resize();
    });
    this.observer = new ResizeObserver(() => this.resize());
    this.observer.observe(this.element);
    this.unhear =
      source.interruptions?.((broken) => {
        this.broken = broken;
        this.tell();
      }) ?? (() => {});
  }

  /** Says `text` in the status, why on hover, unless the way is broken. */
  private say(text: string, why = ""): void {
    this.said = { text, why };
    this.tell();
  }

  private tell(): void {
    const { text, why } = this.broken ? { text: this.options.interrupted ?? "Disconnected, reconnecting…", why: this.broken.why } : this.said;
    this.status.textContent = text;
    if (why) this.status.title = why;
    else this.status.removeAttribute("title");
  }

  get length(): number {
    const length = this.source.extent.length;
    if (!Number.isSafeInteger(length) || length < 0) throw new RangeError("source length must be a nonnegative safe integer");
    return Math.min(length, this.options.limit ?? Infinity);
  }
  get rowHeight(): number { return this.lineHeight || Math.round(parseFloat(getComputedStyle(this.element).lineHeight)); }
  get ready(): Promise<void> {
    return (async () => {
      do {
        await this.pending;
        // The look after a read was asked for first, so it runs before this frame's wait ends.
        if (this.again) await new Promise<void>((settle) => requestAnimationFrame(() => settle()));
      } while (this.requestRange || this.again);
    })();
  }
  get range(): { start: number; end: number; cached: number; drawn: number } {
    return { start: this.first, end: this.end, cached: this.cache.size, drawn: this.rows.size };
  }

  get top(): number {
    const physical = this.element.scrollHeight - this.element.clientHeight;
    const logical = Math.max(0, this.length * this.rowHeight - this.element.clientHeight);
    return physical > 0 ? this.element.scrollTop * logical / physical : 0;
  }

  setTop(top: number): void {
    const logical = Math.max(0, this.length * this.rowHeight - this.element.clientHeight);
    const physical = this.element.scrollHeight - this.element.clientHeight;
    this.element.scrollTop = logical > 0 ? Math.max(0, Math.min(logical, top)) * physical / logical : 0;
    this.scroll();
  }

  /** Returns unused vertical distance for scroll chaining. */
  scrollBy(delta: number): number {
    const before = this.top;
    this.setTop(before + delta);
    return delta - (this.top - before);
  }

  setLimit(limit: number | undefined): void {
    if (limit === undefined) delete this.options.limit;
    else this.options.limit = limit;
    this.resize();
  }

  jump(line: number, match: LineMatch | null = null): void {
    if (!Number.isSafeInteger(line) || line < 0 || line >= this.length) throw new RangeError("line is outside the source");
    // A viewport just put on the page is sized here, not first on its
    // observer's turn, which would come too late for this position.
    if (!this.lineHeight) this.resize();
    this.match = match;
    this.setTop(line * this.rowHeight);
  }

  setMatch(match: LineMatch | null): void { this.match = match; this.draw(); }

  private resize(): void {
    if (this.destroyed || this.element.clientHeight === 0) return;
    const top = this.lastTop;
    // Rows step by whole pixels: each row's top is exactly the one above's
    // bottom, so tinted rows meet without a seam at any display scale.
    this.lineHeight = Math.round(parseFloat(getComputedStyle(this.element).lineHeight));
    if (!(this.lineHeight > 0)) throw new Error("line-height must resolve to pixels");
    if (this.width !== this.element.clientWidth) {
      this.width = this.element.clientWidth;
      this.maximumWidth = 0;
      this.track.style.width = "100%";
    }
    this.track.style.height = `${this.length * this.lineHeight}px`;
    this.setTop(this.options.follow && this.atEnd ? this.length * this.lineHeight : top);
  }

  private scroll = (): void => {
    if (this.destroyed || !this.lineHeight || this.element.clientHeight === 0) return;
    this.status.style.top = `${this.element.scrollTop}px`;
    this.status.style.left = `${this.element.scrollLeft}px`;
    const top = this.top;
    this.lastTop = top;
    this.atEnd = Math.ceil(top + this.element.clientHeight) >= Math.floor(this.length * this.lineHeight);
    const first = Math.max(0, Math.floor(top / this.lineHeight));
    const end = Math.min(this.length, Math.ceil((top + this.element.clientHeight) / this.lineHeight));
    const changed = first !== this.first || end !== this.end;
    this.first = first;
    this.end = end;
    for (const index of this.cache.keys()) if (index < first || index >= end) this.cache.delete(index);
    this.draw();
    if (changed && this.requestRange && (this.requestRange[0] < first || this.requestRange[1] > end)) {
      this.request?.abort();
      this.requestRange = null;
    }
    if (this.requestRange) return;
    const missing: number[] = [];
    for (let index = first; index < end; index++) if (!this.cache.has(index)) missing.push(index);
    if (!missing.length) { this.say(""); return; }
    const start = missing[0];
    const count = missing[missing.length - 1] + 1 - start;
    const request = new AbortController();
    this.request = request;
    this.requestRange = [start, start + count];
    this.say(this.options.loading ?? "Loading…");
    this.element.setAttribute("aria-busy", "true");
    let succeeded = false;
    this.pending = this.source.read(start, count, request.signal).then((lines) => {
      if (request.signal.aborted || this.destroyed) return;
      if (lines.length !== count) throw new Error(`source returned ${lines.length} lines for ${start}+${count}`);
      if (lines.some((line) => typeof line !== "string")) throw new TypeError("source lines must be strings");
      for (let offset = 0; offset < count; offset++) {
        const index = start + offset;
        if (index >= this.first && index < this.end) this.cache.set(index, lines[offset]);
      }
      this.say("");
      this.draw();
      succeeded = true;
    }).catch((error: unknown) => {
      if (request.signal.aborted) return;
      this.say(typeof this.options.failed === "function" ? this.options.failed(error)
        : (this.options.failed ?? "Unable to read content"), String(error));
      throw error;
    }).finally(() => {
      if (this.request === request) {
        this.requestRange = null;
        this.element.setAttribute("aria-busy", "false");
        // What came into view while it was read is read on the next frame,
        // never in this task: no read and what it draws can then keep the
        // page from painting and taking input.
        if (succeeded && !this.again) {
          this.again = requestAnimationFrame(() => {
            this.again = 0;
            this.scroll();
          });
        }
      }
    });
    // The visible error remains; callers can also await ready to receive it.
    void this.pending.catch(() => {});
  };

  private draw(): void {
    for (const [index, row] of this.rows) {
      if (index < this.first || index >= this.end) { row.remove(); this.rows.delete(index); }
    }
    const paint = this.options.paint;
    // All rows share this physical-to-logical offset. Reading it between
    // row writes would force layout once per line.
    let physicalTop = this.element.scrollTop;
    let logicalTop = this.top;
    for (let index = this.first; index < this.end; index++) {
      let row = this.rows.get(index);
      if (!row) {
        row = document.createElement("div");
        row.className = "lv-line";
        row.dataset.line = String(index);
        if (!paint) {
          const text = document.createElement("span");
          text.className = "lv-text";
          if (this.options.numbers !== false) {
            const number = document.createElement("span");
            number.className = "lv-number";
            number.textContent = String(index + 1);
            number.setAttribute("aria-hidden", "true");
            row.append(number);
          }
          row.append(text);
        }
        this.rows.set(index, row);
        this.track.append(row);
      }
      row.style.top = `${physicalTop + index * this.lineHeight - logicalTop}px`;
      row.style.height = `${this.lineHeight}px`;
      const value = this.cache.get(index) ?? "";
      const match = this.match?.line === index ? { start: this.match.start, end: this.match.end } : null;
      const matchKey = match ? `${match.start}:${match.end}` : "";
      const painted = this.painted.get(row);
      if (painted?.value === value && painted.match === matchKey) continue;
      this.painted.set(row, { value, match: matchKey });
      if (paint) {
        row.className = "lv-line";
        row.replaceChildren();
        paint(row, value, index, match);
        const mark = row.querySelector("mark");
        if (mark) {
          mark.scrollIntoView({ block: "nearest", inline: "nearest" });
          physicalTop = this.element.scrollTop;
          logicalTop = this.top;
        }
        continue;
      }
      const text = row.lastElementChild as HTMLElement;
      if (match) {
        const mark = document.createElement("mark");
        mark.textContent = value.slice(match.start, match.end);
        text.replaceChildren(value.slice(0, match.start), mark, value.slice(match.end));
        mark.scrollIntoView({ block: "nearest", inline: "nearest" });
        physicalTop = this.element.scrollTop;
        logicalTop = this.top;
      } else text.textContent = value;
    }
    this.maximumWidth = Math.max(this.maximumWidth, ...Array.from(this.rows.values(), (row) => row.scrollWidth));
    this.track.style.width = `${Math.max(this.width, this.maximumWidth)}px`;
  }

  private wheel = (event: WheelEvent): void => {
    if (event.ctrlKey || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
    // Above the engine's CSS-height range, native wheel distances would scale.
    if (this.length * this.rowHeight <= this.element.scrollHeight + 1) return;
    event.preventDefault();
    const delta = event.deltaY * (event.deltaMode === 1 ? this.rowHeight : event.deltaMode === 2 ? this.element.clientHeight : 1);
    this.chain(this.scrollBy(delta));
  };

  /** Used by touch routing as well as saturated scroll ranges. */
  chain(delta: number): void {
    for (let parent = this.element.parentElement; parent && Math.abs(delta) >= 1; parent = parent.parentElement) {
      if (!/(auto|scroll)/.test(getComputedStyle(parent).overflowY)) continue;
      const before = parent.scrollTop;
      parent.scrollTop += delta;
      delta -= parent.scrollTop - before;
    }
  }

  destroy(): void {
    this.destroyed = true;
    cancelAnimationFrame(this.again);
    this.unchain();
    this.request?.abort();
    this.unsubscribe();
    this.unhear();
    this.observer.disconnect();
    this.element.removeEventListener("scroll", this.scroll);
    this.element.removeEventListener("wheel", this.wheel);
    this.cache.clear();
    this.rows.clear();
    this.track.replaceChildren();
    this.element.remove();
  }
}
