// A file on one of the World's machines, read for the user: a decision of a
// project, the output of a job, a journal's attachment kept where it was
// written. Markdown is read as prose; an image or a PDF as itself; anything
// else as its own text, the end of it where it is long and still growing.
// Bytes that are not UTF-8 are said to be so, never decoded by guess.

import { drawDiagrams } from "../conversation/diagrams";
import { glyph } from "../conversation/glyphs";
import { escape, render } from "../conversation/prose";
import type { FileRange, FileReadInfo } from "../world/file-read";
import type { PdfView, PdfViewState } from "../pdf";
import type { Page } from "./shell";

export interface TextRead {
  info: FileReadInfo;
  text: string | null;
}

export interface BytesRead {
  info: FileReadInfo;
  base64: string;
}

/** The images a page can show, by the name's ending. */
const images: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  bmp: "image/bmp",
  avif: "image/avif",
  // In an image, an SVG's scripts never run.
  svg: "image/svg+xml",
};

export interface ReaderWords {
  reread: string;
  reading: string;
  /** Only the end is shown: how much, of how much. */
  tail(shown: string, size: string): string;
  notText(size: string): string;
  /** A PDF's page, for its position and for a screen reader. */
  page(page: number, count: number): string;
  fitWidth: string;
  zoomIn: string;
  zoomOut: string;
  /** A PDF that needs a password, which is not asked for here. */
  pdfLocked: string;
  pdfFailed(why: string): string;
  failed(why: string): string;
  bytes(count: number): string;
  modified(ago: string): string;
}

export interface ReaderOptions {
  words: ReaderWords;
  machine: string;
  path: string;
  /** The end of the file, for what keeps growing; the whole otherwise. */
  tail: boolean;
  head(title: string, actions?: string): string;
  read(machine: string, path: string, range: FileRange): Promise<TextRead>;
  readBytes(machine: string, path: string): Promise<BytesRead>;
  /** Says how long ago a moment was. */
  ago(at: Date): string;
}

/** How much of a growing file's end is read: about the last few thousand
 *  lines of a log, which is what a person scrolls back through. */
const TAIL_BYTES = 256 * 1024;

export class ReaderPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private facts: HTMLElement;
  private body: HTMLElement;
  private asked = 0;
  private pdf: PdfView | null = null;

  constructor(private readonly options: ReaderOptions) {
    const { words, path } = options;
    const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    this.title = path.slice(cut + 1) || path;
    this.element = document.createElement("div");
    this.element.className = "reader-page";
    this.element.innerHTML = `
      ${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`, `<button class="text-button reread" type="button">${glyph("reread", 16)}<span>${escape(words.reread)}</span></button>`)}
      <p class="reader-facts"></p>
      <div class="reader-body scroller"></div>`;
    this.facts = this.element.querySelector<HTMLElement>(".reader-facts")!;
    this.body = this.element.querySelector<HTMLElement>(".reader-body")!;
    this.element.querySelector(".reread")!.addEventListener("click", () => void this.read());
    void this.read();
  }

  leave(): void {
    this.asked += 1;
    this.closePdf();
  }

  private closePdf(): void {
    void this.pdf?.destroy();
    this.pdf = null;
    this.body.classList.remove("reader-pdf");
  }

  private async read(): Promise<void> {
    const { words, machine, path, tail } = this.options;
    const asked = ++this.asked;
    this.facts.innerHTML = `<span class="fact">${escape(machine)}</span><span class="sep">·</span><span class="fact reader-path">${escape(path)}</span><span class="sep">·</span><span class="fact">${escape(words.reading)}</span>`;
    const ending = /\.([a-z0-9]+)$/i.exec(path)?.[1]?.toLowerCase() ?? "";
    if (ending === "pdf") return this.readPdf(asked);
    const image = images[ending];
    let read: TextRead | (BytesRead & { image: string });
    try {
      read = image
        ? { ...(await this.options.readBytes(machine, path)), image }
        : await this.options.read(machine, path, tail ? { kind: "tail", bytes: TAIL_BYTES } : { kind: "all" });
    } catch (error) {
      if (asked !== this.asked) return;
      this.facts.lastElementChild!.textContent = words.failed(String(error));
      this.facts.lastElementChild!.classList.add("caution");
      return;
    }
    if (asked !== this.asked) return;
    this.closePdf();
    this.draw(read);
  }

  /** A PDF, page by page in its own viewer, which is loaded only when one is
   *  read; its bytes come whole in one read, with their facts. */
  private async readPdf(asked: number): Promise<void> {
    const { words, machine, path } = this.options;
    let read = false;
    try {
      const { PdfView, readPdf } = await import("../pdf");
      const bytes = await readPdf(machine, path);
      if (asked !== this.asked) return;
      this.closePdf();
      this.drawFacts(bytes.info);
      read = true;
      const view = new PdfView({ label: this.title, pageLabel: (page, count) => words.page(page, count) });
      this.pdf = view;
      const bar = document.createElement("div");
      bar.className = "reader-pdf-bar";
      bar.innerHTML = `<span class="reader-pdf-page" aria-live="polite"></span><span class="reader-pdf-zoom">
        <button class="icon-button" type="button" data-zoom="out" aria-label="${escape(words.zoomOut)}" title="${escape(words.zoomOut)}">${glyph("minus", 16)}</button>
        <button class="text-button" type="button" data-zoom="fit">${escape(words.fitWidth)}</button>
        <button class="icon-button" type="button" data-zoom="in" aria-label="${escape(words.zoomIn)}" title="${escape(words.zoomIn)}">${glyph("plus", 16)}</button></span>`;
      const position = bar.querySelector<HTMLElement>(".reader-pdf-page")!;
      const shown = (state: PdfViewState) => {
        position.textContent = state.pageCount ? words.page(state.page, state.pageCount) : "";
      };
      view.addEventListener("change", (event) => shown((event as CustomEvent<PdfViewState>).detail));
      bar.addEventListener("click", (event) => {
        const zoom = (event.target as HTMLElement).closest<HTMLElement>("[data-zoom]")?.dataset.zoom;
        if (!zoom || view.state.status !== "ready") return;
        // Steps of a quarter, from what fitting the width gave.
        const scale = view.state.scale / (96 / 72);
        if (zoom === "fit") view.setZoom("fit-width");
        else view.setZoom(Math.min(8, Math.max(0.25, Math.round((scale + (zoom === "in" ? 0.25 : -0.25)) * 4) / 4)));
      });
      this.body.classList.add("reader-pdf");
      this.body.replaceChildren(bar, view.element);
      await view.load(bytes.bytes);
      view.element.focus({ preventScroll: true });
    } catch (error) {
      if (asked !== this.asked) return;
      const code = (error as { code?: string }).code;
      if (code === "closed") return;
      this.closePdf();
      const why = error instanceof Error ? error.message : String(error);
      if (!read) {
        // Not read at all: said where the facts would be, as for any file.
        this.facts.lastElementChild!.textContent = words.failed(why);
        this.facts.lastElementChild!.classList.add("caution");
        return;
      }
      this.body.innerHTML = `<p class="quiet">${escape(code === "password" ? words.pdfLocked : words.pdfFailed(why))}</p>`;
    }
  }

  private drawFacts(info: FileReadInfo): void {
    const { words } = this.options;
    const size = words.bytes(info.file.size);
    const facts = [
      escape(info.machine),
      `<span class="reader-path">${escape(info.path)}</span>`,
      escape(size),
      escape(words.modified(this.options.ago(new Date(info.file.modified)))),
      info.offset > 0 ? escape(words.tail(words.bytes(info.length), size)) : "",
    ].filter(Boolean);
    this.facts.innerHTML = facts.map((fact) => `<span class="fact">${fact}</span>`).join(`<span class="sep">·</span>`);
  }

  private draw(read: TextRead | (BytesRead & { image: string })): void {
    const { words } = this.options;
    const info = read.info;
    const size = words.bytes(info.file.size);
    this.drawFacts(info);
    if ("image" in read) {
      this.body.innerHTML = `<figure class="reader-image"><img alt="${escape(info.path)}" src="data:${read.image};base64,${read.base64}"></figure>`;
      return;
    }
    let text = read.text;
    if (text === null) {
      this.body.innerHTML = `<p class="quiet">${escape(words.notText(size))}</p>`;
      return;
    }
    // The end of a file begins inside a line; that broken line is left out.
    if (info.offset > 0) text = text.slice(text.indexOf("\n") + 1);
    const markdown = /\.(md|markdown)$/i.test(info.path) && info.offset === 0;
    this.body.innerHTML = markdown
      ? `<article class="prose reader-prose">${render(text)}</article>`
      : `<pre class="reader-text"><code>${escape(text)}</code></pre>`;
    if (markdown) void drawDiagrams(this.body);
    // What keeps growing is read from its newest end.
    else if (info.offset > 0) this.body.scrollTop = this.body.scrollHeight;
  }
}
