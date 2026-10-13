import { AnchorLayout, AnchoredHeight } from "./anchor";
import type { FoldFrame, FoldMotion } from "./anchor";
import type { LineSource } from "./source";
import { LineViewport } from "./viewport";
import { TextViewer } from "./viewer";
import type { ViewerSource, ViewerWords } from "./viewer";

let serial = 0;

export interface TextFoldOptions {
  heading: string;
  source: LineSource;
  layout: AnchorLayout;
  previewLines?: number;
  more?: (remaining: number, complete: boolean) => string;
  less?: string;
  fullScreen?: string;
  viewerWords?: ViewerWords;
  viewerSources?: readonly ViewerSource[];
  onFrame?: (frame: FoldFrame) => void;
}

/** Fold starts closed. Only its buttons or explicit setOpen/setFull calls
 * change disclosure state. Streaming updates never change that state. */
export class TextFold {
  readonly element = document.createElement("section");
  readonly heading = document.createElement("button");
  readonly more = document.createElement("button");
  readonly box = document.createElement("div");
  viewport: LineViewport | null = null;
  viewer: TextViewer | null = null;
  private content = document.createElement("div");
  private corner = document.createElement("button");
  private motion: AnchoredHeight;
  private opened = false;
  private full = false;
  private unsubscribe: () => void;
  private observer: ResizeObserver;
  private operation = 0;
  private pending: Promise<FoldMotion> = Promise.resolve({ cancelled: false, reduced: false });

  constructor(private options: TextFoldOptions) {
    this.element.className = "lv-fold";
    this.heading.className = "lv-fold-heading";
    this.heading.type = this.more.type = this.corner.type = "button";
    this.heading.textContent = options.heading;
    this.heading.setAttribute("aria-expanded", "false");
    this.more.className = "lv-more";
    this.more.hidden = true;
    this.box.className = "lv-fold-box";
    this.box.id = `lv-fold-${++serial}`;
    this.heading.setAttribute("aria-controls", this.box.id);
    this.more.setAttribute("aria-controls", this.box.id);
    this.box.style.height = "0px";
    this.content.className = "lv-fold-content";
    this.content.hidden = true;
    this.content.inert = true;
    this.corner.className = "lv-corner";
    this.corner.textContent = "⤢";
    this.corner.setAttribute("aria-label", options.fullScreen ?? "Full screen");
    this.corner.title = options.fullScreen ?? "Full screen";
    this.content.append(this.corner);
    this.box.append(this.content);
    this.element.append(this.heading, this.box, this.more);
    this.motion = new AnchoredHeight(this.box, options.layout, options.onFrame);
    this.heading.addEventListener("click", () => void this.setOpen(!this.opened));
    this.more.addEventListener("click", () => void this.setFull(!this.full));
    this.corner.addEventListener("click", () => {
      this.viewer = new TextViewer(options.heading, options.viewerSources ?? [{ label: options.heading, source: options.source }], options.viewerWords);
      this.viewer.open(this.viewport?.range.start ?? 0);
    });
    this.unsubscribe = options.source.subscribe(() => this.refresh());
    this.observer = new ResizeObserver(() => this.refresh());
    this.observer.observe(options.layout.scroller);
  }

  get isOpen(): boolean { return this.opened; }
  get isFull(): boolean { return this.full; }
  get settled(): Promise<FoldMotion> { return this.pending; }
  private get previewLines(): number { return this.options.previewLines ?? 12; }

  private height(): number {
    if (!this.opened || !this.viewport) return 0;
    const natural = Math.min(this.options.source.extent.length, this.full ? Infinity : this.previewLines) * this.viewport.rowHeight;
    return Math.round(Math.min(this.options.layout.scroller.clientHeight / 2, natural));
  }

  private label(): void {
    const { length, complete } = this.options.source.extent;
    this.more.hidden = !this.opened || (length <= this.previewLines && !this.full);
    this.more.textContent = this.full ? (this.options.less ?? "− show less")
      : (this.options.more?.(Math.max(0, length - this.previewLines), complete) ?? `+ ${Math.max(0, length - this.previewLines).toLocaleString()} more lines${complete ? "" : "…"}`);
    this.more.setAttribute("aria-expanded", String(this.full));
  }

  private refresh(): void {
    this.label();
    if (this.opened && this.viewport) {
      this.viewport.element.style.height = `${this.height()}px`;
      this.motion.resize(this.height(), this.full ? this.more : this.heading);
    }
  }

  setOpen(open: boolean): Promise<FoldMotion> {
    if (this.opened === open) return this.pending;
    const operation = ++this.operation;
    this.opened = open;
    this.heading.setAttribute("aria-expanded", String(open));
    if (open) {
      this.content.hidden = false;
      if (!this.viewport) {
        this.viewport = new LineViewport(this.options.source, {
          label: this.options.heading,
          limit: this.previewLines,
          ...(this.options.viewerWords ? { interrupted: this.options.viewerWords.interrupted } : {}),
        });
        this.content.append(this.viewport.element);
      }
      this.full = false;
      this.viewport.setLimit(this.previewLines);
      this.viewport.element.style.height = `${this.height()}px`;
    }
    this.content.inert = !open;
    // Removing the footer belongs to the same heading-anchored mutation.
    const held = this.options.layout.hold(this.heading);
    this.label(); held.correct(); held.release();
    this.pending = this.motion.to(this.height(), this.heading).then((result) => {
      if (operation !== this.operation || result.cancelled) return result;
      if (!open) {
        this.content.hidden = true;
        this.viewport?.destroy(); this.viewport = null;
        this.full = false;
      }
      return result;
    });
    return this.pending;
  }

  setFull(full: boolean): Promise<FoldMotion> {
    if (!this.opened || this.full === full || !this.viewport) return this.pending;
    const operation = ++this.operation;
    this.full = full;
    this.label();
    const target = this.height();
    if (full) {
      this.viewport.setLimit(undefined);
      this.viewport.element.style.height = `${target}px`;
    }
    this.pending = this.motion.to(target, this.more).then((result) => {
      if (operation !== this.operation || result.cancelled) return result;
      if (!full && this.viewport) {
        this.viewport.element.style.height = `${this.height()}px`;
        this.viewport.setLimit(this.previewLines);
        this.viewport.setTop(0);
      }
      return result;
    });
    return this.pending;
  }

  destroy(): void {
    this.operation++;
    this.motion.destroy();
    this.observer.disconnect();
    this.unsubscribe();
    this.viewport?.destroy();
    this.viewer?.close();
    this.element.remove();
  }
}
