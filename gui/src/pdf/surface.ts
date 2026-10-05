import { AnnotationMode, TextLayer } from "pdfjs-dist/legacy/build/pdf.mjs";
import type { PDFPageProxy, RenderTask } from "pdfjs-dist/types/src/display/api";
import type { PageViewport } from "pdfjs-dist/types/src/display/display_utils";
import { PdfViewError } from "./types";

export interface Rectangle { x: number; y: number; width: number; height: number }
interface Tile extends Rectangle { canvas: HTMLCanvasElement; ready: boolean }

/** A page retains only tiles intersecting the scroll viewport. Tile edges
 * derive from its dimensions, so zoom and DPR never imply a full-page bitmap. */
export class PageSurface {
  private readonly tiles = new Map<string, Tile>();
  private text: TextLayer | undefined;
  private textElement: HTMLDivElement | undefined;
  private textReady = false;
  private dead = false;
  private failed = false;
  private rendering: { task: RenderTask; tile: Tile } | undefined;
  private frame = 0;

  constructor(
    readonly page: PDFPageProxy,
    readonly element: HTMLElement,
    readonly viewport: PageViewport,
    private readonly ratio: number,
  ) {
    element.style.setProperty("--total-scale-factor", String(viewport.scale));
    element.dataset.status = "loading";
  }

  update(area: Rectangle, tileWidth: number, tileHeight: number): void {
    const wanted = new Set<string>();
    for (let y = Math.floor(area.y / tileHeight) * tileHeight; y < area.y + area.height; y += tileHeight) {
      for (let x = Math.floor(area.x / tileWidth) * tileWidth; x < area.x + area.width; x += tileWidth) {
        const key = `${x}:${y}`;
        wanted.add(key);
        if (this.tiles.has(key)) continue;
        const canvas = document.createElement("canvas");
        canvas.className = "world-pdf-tile";
        canvas.setAttribute("aria-hidden", "true");
        const width = Math.min(tileWidth, this.viewport.width - x);
        const height = Math.min(tileHeight, this.viewport.height - y);
        Object.assign(canvas.style, { left: `${x}px`, top: `${y}px`, width: `${width}px`, height: `${height}px` });
        this.tiles.set(key, { x, y, width, height, canvas, ready: false });
      }
    }
    for (const [key, tile] of this.tiles) {
      if (wanted.has(key)) continue;
      if (this.rendering?.tile === tile) this.rendering.task.cancel();
      tile.canvas.remove();
      tile.canvas.width = tile.canvas.height = 0;
      this.tiles.delete(key);
    }
    this.updateStatus();
  }

  get pending(): boolean {
    return !this.dead && !this.failed && (!this.textReady || [...this.tiles.values()].some((tile) => !tile.ready));
  }

  async renderNext(): Promise<void> {
    if (!this.pending) return;
    try {
      const tile = [...this.tiles.values()].find((tile) => !tile.ready);
      if (tile) {
        tile.canvas.width = Math.ceil(tile.width * this.ratio);
        tile.canvas.height = Math.ceil(tile.height * this.ratio);
        const task = this.page.render({
          canvas: tile.canvas,
          viewport: this.viewport,
          transform: [this.ratio, 0, 0, this.ratio, -tile.x * this.ratio, -tile.y * this.ratio],
          // Paint saved appearances, but never build annotation/form DOM.
          annotationMode: AnnotationMode.ENABLE,
        });
        this.rendering = { task, tile };
        task.onContinue = (resume: () => void) => {
          this.frame = requestAnimationFrame(() => {
            this.frame = 0;
            if (!this.dead && this.rendering?.task === task) resume();
          });
        };
        try {
          await task.promise;
          if (!this.dead && [...this.tiles.values()].includes(tile)) {
            tile.ready = true;
            this.element.prepend(tile.canvas);
          }
        } finally {
          this.rendering = undefined;
        }
      } else if (!this.textReady) {
        const content = await this.page.getTextContent();
        if (this.dead) return;
        // PDF.js 5.4.624 tests textDivs.length > 100000 before appending:
        // 100001 runs fit. It silently drops the rest; this view reports it.
        if (content.items.filter((item) => "str" in item).length > 100001) {
          throw new PdfViewError("text-layer", "PDF.js cannot select more than 100001 text runs on one page", this.page.pageNumber);
        }
        this.textElement = document.createElement("div");
        this.textElement.className = "world-pdf-text";
        this.element.append(this.textElement);
        this.text = new TextLayer({ textContentSource: content, container: this.textElement, viewport: this.viewport });
        await this.text.render();
        this.textReady = true;
      }
      this.updateStatus();
    } catch (cause) {
      if (this.dead || cause instanceof Error && cause.name === "RenderingCancelledException") return;
      this.failed = true;
      this.releaseCanvas();
      this.text?.cancel();
      this.textElement?.remove();
      this.element.dataset.status = "error";
      if (cause instanceof PdfViewError) throw cause;
      throw new PdfViewError("render", String(cause), this.page.pageNumber, { cause });
    }
  }

  destroy(cleanup = true): void {
    if (this.dead) return;
    this.dead = true;
    this.rendering?.task.cancel();
    cancelAnimationFrame(this.frame);
    this.text?.cancel();
    this.textElement?.remove();
    this.releaseCanvas(cleanup);
    delete this.element.dataset.status;
  }

  /** Preserve selected text when its page scrolls away, but free its bitmap. */
  releaseCanvas(cleanup = true): void {
    this.rendering?.task.cancel();
    for (const tile of this.tiles.values()) {
      tile.canvas.remove();
      tile.canvas.width = tile.canvas.height = 0;
    }
    this.tiles.clear();
    if (cleanup) this.page.cleanup();
  }

  private updateStatus(): void {
    if (!this.dead && !this.failed) this.element.dataset.status = this.pending ? "loading" : "ready";
  }
}
