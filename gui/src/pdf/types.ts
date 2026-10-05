export type PdfZoom = number | "fit-width" | "fit-page";

/** Uint8Array ownership passes to the viewer; its buffer may be detached.
 * Base64 is accepted for the existing read_bytes bridge. */
export type PdfSource = Uint8Array | { base64: string };

export interface PdfViewOptions {
  /** Accessible name of this document's scroll region, supplied by the page. */
  label: string;
  pageLabel?: (page: number, pageCount: number) => string;
  zoom?: PdfZoom;
}

export interface PdfViewState {
  status: "empty" | "loading" | "ready" | "error" | "destroyed";
  page: number;
  pageCount: number;
  zoom: PdfZoom;
  /** CSS pixels per PDF point on the current page. Numeric zoom 1 is 96 dpi. */
  scale: number;
  rendering: boolean;
}

export class PdfViewError extends Error {
  constructor(
    readonly code: "password" | "invalid-pdf" | "worker" | "render" | "text-layer" | "unsupported" | "closed",
    message: string,
    readonly page: number | null = null,
    options?: ErrorOptions,
  ) {
    super(message, options);
    this.name = "PdfViewError";
  }
}
