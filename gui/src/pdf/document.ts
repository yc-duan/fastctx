import { getDocument, PDFWorker, TextLayer } from "pdfjs-dist/legacy/build/pdf.mjs";
import type { PDFDocumentLoadingTask, PDFDocumentProxy } from "pdfjs-dist/types/src/display/api";
import { CMapReaderFactory, StandardFontDataFactory } from "./resources";
import { PdfViewError } from "./types";
import type { PdfSource } from "./types";

/** One document owns one real worker, including base64 decoding. No fake
 * worker fallback may move untrusted parsing onto the interface thread. */
export class PdfDocument {
  private worker: Worker | undefined = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  private pdfWorker: PDFWorker | undefined = PDFWorker.create({ port: this.worker });
  private task: PDFDocumentLoadingTask | undefined;
  private rejectFailure!: (error: PdfViewError) => void;
  // PDF.js requests cannot settle themselves after a real worker dies.
  readonly failure = new Promise<never>((_, reject) => { this.rejectFailure = reject; });
  private stopped = false;
  private closing: Promise<void> | undefined;

  constructor(private readonly onFailure: (error: PdfViewError) => void) {
    void this.failure.catch(() => {});
    const failed = (event: Event) => {
      const error = new PdfViewError("worker", event instanceof ErrorEvent ? event.message : "PDF worker message could not be read");
      this.rejectFailure(error);
      this.stopped = true;
      this.release();
      this.onFailure(error);
      queueMicrotask(() => TextLayer.cleanup());
    };
    this.worker!.addEventListener("error", failed);
    this.worker!.addEventListener("messageerror", failed);
  }

  async open(source: PdfSource, password?: string): Promise<PDFDocumentProxy> {
    try {
      const data = source instanceof Uint8Array && source.byteLength === source.buffer.byteLength
        ? source
        : await Promise.race([this.decode(source), this.failure]);
      if (this.stopped) throw new PdfViewError("closed", "PDF view was closed");
      this.task = getDocument({
        data,
        password,
        worker: this.pdfWorker,
        CMapReaderFactory,
        StandardFontDataFactory,
        useWorkerFetch: false,
        useSystemFonts: false,
        useWasm: false,
        wasmUrl: new URL(`${import.meta.env.BASE_URL}pdfjs/`, location.href).href,
        isEvalSupported: false,
        enableXfa: false,
        stopAtErrors: true,
      });
      return await Promise.race([this.task.promise, this.failure]);
    } catch (cause) {
      if (cause instanceof PdfViewError) throw cause;
      const error = cause instanceof Error ? cause : new Error(String(cause));
      throw new PdfViewError(error.name === "PasswordException" ? "password" : "invalid-pdf", error.message, null, { cause });
    }
  }

  destroy(): Promise<void> {
    if (this.closing) return this.closing;
    this.rejectFailure(new PdfViewError("closed", "PDF view was closed"));
    if (this.stopped) return Promise.resolve();
    this.stopped = true;
    this.closing = (async () => {
      try {
        await this.task?.destroy();
      } finally {
        this.release();
      }
    })();
    return this.closing;
  }

  private decode(source: PdfSource): Promise<Uint8Array> {
    return new Promise((resolve, reject) => {
      const worker = this.worker!;
      const receive = (event: MessageEvent) => {
        if (event.data?.type !== "world-pdf-bytes") return;
        worker.removeEventListener("message", receive);
        if (event.data.error) reject(new Error(event.data.error));
        else resolve(event.data.bytes);
      };
      worker.addEventListener("message", receive);
      if (source instanceof Uint8Array) {
        worker.postMessage({ type: "world-pdf-data", bytes: source }, [source.buffer as ArrayBuffer]);
      } else {
        worker.postMessage({ type: "world-pdf-data", base64: source.base64 });
      }
    });
  }

  private release(): void {
    // A dead worker cannot acknowledge PDF.js's destroy handshake. Remove
    // only this document's generated fonts (the 5.4 loadedName prefix), and
    // drop the worker/transport references even on this failure path.
    const id = this.task?.docId;
    if (id) {
      for (const font of document.fonts) {
        if (font.family.startsWith(`g_${id}_`)) document.fonts.delete(font);
      }
    }
    this.pdfWorker?.destroy();
    this.worker?.terminate();
    this.worker = undefined;
    this.pdfWorker = undefined;
    this.task = undefined;
  }
}
