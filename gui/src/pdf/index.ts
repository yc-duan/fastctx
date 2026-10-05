import type { PDFDocumentProxy, PDFPageProxy } from "pdfjs-dist/types/src/display/api";
import { PdfDocument } from "./document";
import { PageSurface } from "./surface";
import { PdfViewError } from "./types";
import type { PdfSource, PdfViewOptions, PdfViewState, PdfZoom } from "./types";
import "./view.css";

export { PdfViewError } from "./types";
export { readPdf } from "./read";
export type { PdfRead } from "./read";
export type { PdfSource, PdfViewOptions, PdfViewState, PdfZoom } from "./types";

interface PageSlot {
  element: HTMLDivElement;
  width: number;
  height: number;
  measured: boolean;
  failed?: boolean;
  surface?: PageSurface | undefined;
  retainedPage?: PDFPageProxy | undefined;
}

/** A scrollable PDF surface without application chrome. Listen to `change`
 * (CustomEvent<PdfViewState>) and `error` (CustomEvent<PdfViewError>). The host
 * owns labels, toolbar, error presentation, and the containing element's size.
 * Call destroy when leaving the reader. One instance loads one document. */
export class PdfView extends EventTarget {
  readonly element = document.createElement("div");
  private readonly pages = document.createElement("div");
  private readonly resize = new ResizeObserver(() => this.relayout());
  private readonly slots: PageSlot[] = [];
  private readonly session: PdfDocument;
  private document: PDFDocumentProxy | undefined;
  private current: PdfViewState;
  private frame = 0;
  private draining = false;
  private generation = 0;
  private viewportWidth = 0;
  private viewportHeight = 0;
  private ratio = rasterRatio();
  private readonly wake = () => this.schedule();
  private readonly dprChanged = () => this.relayout();

  constructor(private readonly options: PdfViewOptions) {
    super();
    const zoom = options.zoom ?? "fit-width";
    validateZoom(zoom);
    this.current = { status: "empty", page: 0, pageCount: 0, zoom, scale: 0, rendering: false };
    this.element.className = "world-pdf-view";
    this.element.tabIndex = 0;
    this.element.setAttribute("role", "region");
    this.element.setAttribute("aria-label", options.label);
    this.pages.className = "world-pdf-pages";
    this.element.append(this.pages);
    this.element.addEventListener("scroll", this.wake, { passive: true });
    window.addEventListener("resize", this.dprChanged);
    window.visualViewport?.addEventListener("resize", this.dprChanged);
    window.visualViewport?.addEventListener("scroll", this.wake);
    window.document.addEventListener("selectionchange", this.wake);
    this.resize.observe(this.element);
    this.session = new PdfDocument((error) => this.fail(error));
  }

  get state(): PdfViewState { return { ...this.current }; }

  /** Resolves when page count and initial geometry are available; `rendering`
   * becomes false after the visible canvases and selection layers are ready.
   * Rejected loads also emit `error`. Password failure requires a new instance. */
  async load(source: PdfSource, password?: string): Promise<void> {
    if (this.current.status !== "empty") throw new Error("A PdfView loads one document");
    this.update({ status: "loading" });
    try {
      const document = await this.session.open(source, password);
      if (this.closed) throw new PdfViewError("closed", "PDF view was closed");
      this.document = document;
      const { info } = await Promise.race([document.getMetadata(), this.session.failure]);
      if ((info as { IsXFAPresent?: boolean }).IsXFAPresent) {
        throw new PdfViewError("unsupported", "XFA documents are not supported");
      }
      const first = await Promise.race([document.getPage(1), this.session.failure]);
      if (this.closed) throw new PdfViewError("closed", "PDF view was closed");
      const size = first.getViewport({ scale: 1 });
      const fragment = window.document.createDocumentFragment();
      for (let number = 1; number <= document.numPages; number++) {
        const element = window.document.createElement("div");
        element.className = "world-pdf-page";
        element.dataset.page = String(number);
        element.setAttribute("role", "group");
        element.setAttribute("aria-label", this.options.pageLabel?.(number, document.numPages) ?? String(number));
        this.slots.push({ element, width: size.width, height: size.height, measured: number === 1 });
        fragment.append(element);
      }
      this.pages.append(fragment);
      this.update({ status: "ready", page: 1, pageCount: document.numPages, rendering: true });
      this.relayout(true);
    } catch (cause) {
      const error = this.state.status === "destroyed"
        ? new PdfViewError("closed", "PDF view was closed", null, { cause })
        : cause instanceof PdfViewError ? cause : new PdfViewError("invalid-pdf", String(cause), null, { cause });
      if (!this.closed) this.fail(error);
      await this.session.destroy();
      throw error;
    }
  }

  /** Page numbers are one-based. Unvisited page sizes are estimated from the
   * first page and replaced on arrival while preserving the reading position. */
  goToPage(page: number): void {
    if (this.current.status !== "ready") throw new Error("PDF document is not ready");
    if (!Number.isInteger(page) || page < 1 || page > this.slots.length) throw new RangeError("PDF page is out of range");
    this.element.scrollTop = this.slots[page - 1].element.offsetTop;
    this.update({ page, rendering: true });
    this.schedule();
  }

  /** Numeric zoom is a positive multiplier of 96 dpi; fit modes follow resize. */
  setZoom(zoom: PdfZoom): void {
    validateZoom(zoom);
    if (this.closed) throw new PdfViewError("closed", "PDF view was closed");
    this.update({ zoom });
    this.relayout(true);
  }

  async destroy(): Promise<void> {
    if (this.current.status === "destroyed") return this.session.destroy();
    this.generation++;
    cancelAnimationFrame(this.frame);
    this.resize.disconnect();
    this.element.removeEventListener("scroll", this.wake);
    window.removeEventListener("resize", this.dprChanged);
    window.visualViewport?.removeEventListener("resize", this.dprChanged);
    window.visualViewport?.removeEventListener("scroll", this.wake);
    window.document.removeEventListener("selectionchange", this.wake);
    for (const slot of this.slots) {
      slot.surface?.destroy();
      slot.retainedPage?.cleanup();
    }
    this.slots.length = 0;
    this.pages.replaceChildren();
    this.document = undefined;
    this.update({ status: "destroyed", rendering: false });
    await this.session.destroy();
  }

  private get closed(): boolean { return this.current.status === "destroyed" || this.current.status === "error"; }

  private scale(slot: PageSlot): number {
    if (typeof this.current.zoom === "number") return this.current.zoom * 96 / 72;
    const width = this.viewportWidth / slot.width;
    return this.current.zoom === "fit-page" ? Math.min(width, this.viewportHeight / slot.height) : width;
  }

  private relayout(force = false): void {
    if (this.closed || !this.slots.length || !this.element.clientWidth || !this.element.clientHeight) return;
    const { clientWidth: width, clientHeight: height } = this.element;
    if (!force && width === this.viewportWidth && height === this.viewportHeight && this.ratio === rasterRatio()) return;
    const slot = this.slots[this.current.page - 1];
    const oldHeight = slot.element.offsetHeight;
    const offset = oldHeight ? (this.element.scrollTop - slot.element.offsetTop) / oldHeight : 0;
    const horizontal = this.viewportWidth ? (this.element.scrollLeft + this.viewportWidth / 2) / this.pages.scrollWidth : 0.5;
    const tilesChanged = width !== this.viewportWidth || height !== this.viewportHeight || this.ratio !== rasterRatio();
    this.viewportWidth = width;
    this.viewportHeight = height;
    this.ratio = rasterRatio();
    this.generation++;
    for (const entry of this.slots) {
      const scale = this.scale(entry);
      // Measuring a newly visited page must not remove text selected on
      // another page whose geometry has not changed.
      if (entry.surface && (tilesChanged || entry.surface.viewport.scale !== scale)) {
        // Zoom reuses decoded images and drawing instructions. Offscreen
        // pages release this retained proxy in the next visibility pass.
        entry.retainedPage = entry.surface.page;
        entry.surface.destroy(false);
        entry.surface = undefined;
      }
      if (entry.failed) entry.element.dataset.status = "error";
      entry.element.style.width = `${entry.width * scale}px`;
      entry.element.style.height = `${entry.height * scale}px`;
    }
    this.element.scrollTop = slot.element.offsetTop + offset * slot.element.offsetHeight;
    this.element.scrollLeft = horizontal * this.pages.scrollWidth - width / 2;
    this.update({ rendering: true });
    this.schedule();
  }

  private schedule(): void {
    if (this.closed || this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      void this.draw();
    });
  }

  private async draw(): Promise<void> {
    if (this.closed || !this.document || !this.element.clientWidth || !this.element.clientHeight) return;
    const root = this.element.getBoundingClientRect();
    const visual = window.visualViewport;
    const left = Math.max(root.left, visual?.offsetLeft ?? 0);
    const top = Math.max(root.top, visual?.offsetTop ?? 0);
    const right = Math.min(root.left + this.element.clientWidth, (visual?.offsetLeft ?? 0) + (visual?.width ?? innerWidth));
    const bottom = Math.min(root.top + this.element.clientHeight, (visual?.offsetTop ?? 0) + (visual?.height ?? innerHeight));
    const tileWidth = Math.ceil(Math.min(this.viewportWidth, visual?.width ?? innerWidth));
    const tileHeight = Math.ceil(Math.min(this.viewportHeight, visual?.height ?? innerHeight));
    const visible: number[] = [];
    const selection = window.getSelection();
    let largest = 0;
    let current = this.current.page;
    for (let index = 0; index < this.slots.length; index++) {
      const slot = this.slots[index];
      const rect = slot.element.getBoundingClientRect();
      const x = Math.max(0, left - rect.left);
      const y = Math.max(0, top - rect.top);
      const width = Math.min(rect.right, right) - Math.max(rect.left, left);
      const height = Math.min(rect.bottom, bottom) - Math.max(rect.top, top);
      if (width > 0 && height > 0) {
        visible.push(index);
        if (width * height > largest) { largest = width * height; current = index + 1; }
        slot.surface?.update({ x, y, width, height }, tileWidth, tileHeight);
      } else {
        slot.retainedPage?.cleanup();
        slot.retainedPage = undefined;
        if (slot.surface) {
          if (selection && !selection.isCollapsed && selection.containsNode(slot.element, true)) {
            slot.surface.releaseCanvas();
          } else {
            slot.surface.destroy();
            slot.surface = undefined;
          }
        }
      }
    }
    this.update({ page: current, scale: this.scale(this.slots[current - 1]) });
    if (this.draining) return;
    this.draining = true;
    const generation = this.generation;
    let drawingPage = current;
    try {
      // One page/tile at a time bounds transient decoded image and canvas work.
      for (const index of visible) {
        const slot = this.slots[index];
        if (slot.failed) continue;
        drawingPage = index + 1;
        if (!slot.surface) {
          this.update({ rendering: true });
          const page = slot.retainedPage ?? await Promise.race([this.document.getPage(index + 1), this.session.failure]);
          slot.retainedPage = undefined;
          if (this.closed || generation !== this.generation) { page.cleanup(); return; }
          const size = page.getViewport({ scale: 1 });
          if (!slot.measured) {
            slot.measured = true;
            slot.width = size.width;
            slot.height = size.height;
            this.relayout(true);
            page.cleanup();
            return;
          }
          slot.surface = new PageSurface(page, slot.element, page.getViewport({ scale: this.scale(slot) }), this.ratio);
          this.schedule();
          return;
        }
        if (slot.surface.pending) {
          this.update({ rendering: true });
          await slot.surface.renderNext();
          this.schedule();
          return;
        }
      }
      this.update({ rendering: false });
    } catch (cause) {
      if (!this.closed && generation === this.generation) {
        const slot = this.slots[drawingPage - 1];
        slot.failed = true;
        slot.element.dataset.status = "error";
        const error = cause instanceof PdfViewError ? cause : new PdfViewError("render", String(cause), drawingPage, { cause });
        this.dispatchEvent(new CustomEvent("error", { detail: error }));
        this.update({ rendering: false });
        this.schedule();
      }
    } finally {
      this.draining = false;
      if (generation !== this.generation && !this.closed) this.schedule();
    }
  }

  private update(change: Partial<PdfViewState>): void {
    if (Object.entries(change).every(([key, value]) => this.current[key as keyof PdfViewState] === value)) return;
    Object.assign(this.current, change);
    this.dispatchEvent(new CustomEvent("change", { detail: this.state }));
  }

  private fail(error: PdfViewError): void {
    if (this.closed) return;
    this.generation++;
    cancelAnimationFrame(this.frame);
    this.frame = 0;
    for (const slot of this.slots) {
      slot.surface?.destroy();
      slot.surface = undefined;
      slot.retainedPage?.cleanup();
      slot.retainedPage = undefined;
    }
    this.document = undefined;
    this.update({ status: "error", rendering: false });
    this.dispatchEvent(new CustomEvent("error", { detail: error }));
  }
}

function validateZoom(zoom: PdfZoom): void {
  if (zoom === "fit-width" || zoom === "fit-page" || typeof zoom === "number" && Number.isFinite(zoom) && zoom > 0) return;
  throw new RangeError("PDF zoom must be positive, fit-width, or fit-page");
}

// Native mobile pinch zoom changes the visual viewport, not devicePixelRatio.
function rasterRatio(): number {
  return devicePixelRatio * (window.visualViewport?.scale ?? 1);
}
