// A file on one of the World's machines, read for the user: a decision of a
// project, the output of a job, a journal's attachment kept where it was
// written. Markdown is read as prose; anything else as its own text, the
// end of it where it is long and still growing. Bytes that are not UTF-8
// are said to be so, never decoded by guess.

import { drawDiagrams } from "../conversation/diagrams";
import { escape, render } from "../conversation/prose";
import type { FileRange, FileReadInfo } from "../world/file-read";
import type { Page } from "./shell";

export interface TextRead {
  info: FileReadInfo;
  text: string | null;
}

export interface ReaderWords {
  reread: string;
  reading: string;
  /** Only the end is shown: how much, of how much. */
  tail(shown: string, size: string): string;
  notText(size: string): string;
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

  constructor(private readonly options: ReaderOptions) {
    const { words, path } = options;
    const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    this.title = path.slice(cut + 1) || path;
    this.element = document.createElement("div");
    this.element.className = "reader-page";
    this.element.innerHTML = `
      ${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`, `<button class="text-button reread" type="button"><span>${escape(words.reread)}</span></button>`)}
      <p class="reader-facts"></p>
      <div class="reader-body scroller"></div>`;
    this.facts = this.element.querySelector<HTMLElement>(".reader-facts")!;
    this.body = this.element.querySelector<HTMLElement>(".reader-body")!;
    this.element.querySelector(".reread")!.addEventListener("click", () => void this.read());
    void this.read();
  }

  leave(): void {
    this.asked += 1;
  }

  private async read(): Promise<void> {
    const { words, machine, path, tail } = this.options;
    const asked = ++this.asked;
    this.facts.innerHTML = `<span class="fact">${escape(machine)}</span><span class="sep">·</span><span class="fact reader-path">${escape(path)}</span><span class="sep">·</span><span class="fact">${escape(words.reading)}</span>`;
    let read: TextRead;
    try {
      read = await this.options.read(machine, path, tail ? { kind: "tail", bytes: TAIL_BYTES } : { kind: "all" });
    } catch (error) {
      if (asked !== this.asked) return;
      this.facts.lastElementChild!.textContent = words.failed(String(error));
      this.facts.lastElementChild!.classList.add("caution");
      return;
    }
    if (asked !== this.asked) return;
    this.draw(read);
  }

  private draw({ info, text }: TextRead): void {
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
