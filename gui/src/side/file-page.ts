// A file read in the side over the conversation's parts (`world-gui.md`,
// 对话的侧面板): set as the plan's page is, its machine and folder quiet on
// the first line beside the way to open it with this device's program, its
// name over a ruled stroke, then the file itself, which alone scrolls:
// Markdown as prose, anything else as its lines, numbered and coloured for
// their language. The machine holding it gives the lines a screen at a time
// and says when they change, and the page follows; what can be read is what
// the account logged in here may work on there.

import hljs from "highlight.js/lib/common";
import { drawDiagrams } from "../conversation/diagrams";
import { language } from "../conversation/diff";
import { isMarkdown, leafOf } from "../conversation/file-kind";
import { glyph } from "../conversation/glyphs";
import { escape, filePathOf, render } from "../conversation/prose";
import { LineViewport, type LineInterruption, type LinePaint, type LineSource } from "../longview";
import { Sketch } from "../pen/sketch";
import type { Peek } from "./pane";
import type { FileWords } from "./words";
import "./file-page.css";

/** A file's lines as the machine holding it gives them, let go when done. */
export type FileLines = LineSource & { close(): Promise<void> };

export interface FilePageOptions {
  words: FileWords;
  machine: string;
  path: string;
  lines(machine: string, path: string): Promise<FileLines>;
  /** Opens it with this device's program for its kind. */
  outside(): void;
  /** Another file a link in it names, on the same machine. */
  open(machine: string, path: string): void;
  /** A picture its Markdown shows, made small by the machine holding it,
   *  as an address an `<img>` takes. */
  picture?: ((machine: string, path: string, max: number) => Promise<string>) | undefined;
  /** Whether a failure says the file is not there. */
  gone(error: unknown): boolean;
  /** A failure's original words, for the pointer resting on what says it. */
  detail(error: unknown): string;
}

/** Markdown up to this many lines is set as prose; a longer one is shown
 *  as its lines, as any other text. */
const PROSE_LINES = 4000;
/** How long the prose waits after a change before it is set again, so a
 *  file written in many small pieces is set once they settle. */
const SETTLE = 300;
/** Pixels on the longer side of a picture in the prose: twice the widest
 *  the side draws it, for screens of twice the density. */
const PICTURE = 1200;

export class FilePage implements Peek {
  readonly element = document.createElement("div");
  private readonly body: HTMLElement;
  private readonly rule: HTMLElement;
  private readonly request = new AbortController();
  private source: FileLines | null = null;
  private viewport: LineViewport | null = null;
  private stops: (() => void)[] = [];
  private ruled = 0;
  private resizing: ResizeObserver;
  private settling = 0;

  constructor(private readonly options: FilePageOptions) {
    const { words, machine, path } = options;
    const name = leafOf(path);
    const folder = path.slice(0, path.length - name.length);
    this.element.className = "fv-page";
    this.element.innerHTML = `<header class="fv-head">
        <span class="fv-where" title="${escape(`${machine}  ${path}`)}">${glyph("machine", 14)}<span class="fv-machine">${escape(machine)}</span><span class="fv-folder"><bdi>${escape(folder)}</bdi></span></span>
        <button class="icon-button fv-outside" type="button" aria-label="${escape(words.outside)}" title="${escape(words.outside)}">${glyph("launch", 18)}</button>
      </header>
      <h2 class="fv-name">${escape(name)}</h2>
      <div class="fv-rule" aria-hidden="true"></div>
      <div class="fv-body"><p class="fv-note">${escape(words.reading)}</p></div>`;
    this.body = this.element.querySelector<HTMLElement>(".fv-body")!;
    this.rule = this.element.querySelector<HTMLElement>(".fv-rule")!;
    this.element.querySelector(".fv-outside")!.addEventListener("click", () => options.outside());
    this.body.addEventListener("click", (event) => this.pressed(event));
    this.resizing = new ResizeObserver(() => this.drawRule());
    this.resizing.observe(this.element);
    void this.read();
  }

  leave(): void {
    this.request.abort();
    clearTimeout(this.settling);
    this.resizing.disconnect();
    for (const stop of this.stops) stop();
    this.stops = [];
    this.viewport?.destroy();
    this.viewport = null;
    const source = this.source;
    this.source = null;
    if (source) void source.close().catch((error: unknown) => console.warn("a file's lines could not be let go", error));
  }

  /** The rule under the name, drawn again when the page's width changes. */
  private drawRule(): void {
    const width = this.rule.clientWidth;
    if (!width || width === this.ruled) return;
    this.ruled = width;
    this.rule.innerHTML = new Sketch("file-title").rule([0, 3], [width, 3], 1.1, "graphite").svg(width, 6, "fv-rule-svg");
  }

  private async read(): Promise<void> {
    const { options } = this;
    const { signal } = this.request;
    let source: FileLines;
    try {
      source = await options.lines(options.machine, options.path);
    } catch (error) {
      if (!signal.aborted) this.fail(error);
      return;
    }
    if (signal.aborted) {
      void source.close().catch(() => {});
      return;
    }
    this.source = source;
    if (isMarkdown(options.path) && source.extent.length <= PROSE_LINES) await this.prose(source);
    else this.showLines(source);
  }

  /** The file as prose, set again a moment after it changes there, where
   *  the reader was left. */
  private async prose(source: FileLines): Promise<void> {
    const { signal } = this.request;
    const scroll = document.createElement("div");
    scroll.className = "fv-scroll";
    const article = document.createElement("article");
    article.className = "prose fv-prose";
    scroll.append(article);
    const status = document.createElement("p");
    status.className = "fv-status";
    status.hidden = true;
    const set = async (): Promise<boolean> => {
      const length = source.extent.length;
      // Grown past what prose is set for, it is shown as its lines.
      if (length > PROSE_LINES) {
        this.body.replaceChildren();
        this.showLines(source);
        return false;
      }
      const lines = length ? await source.read(0, length, signal) : [];
      if (signal.aborted) return false;
      const top = scroll.scrollTop;
      article.innerHTML = render(lines.join("\n"));
      this.placePictures(article);
      void drawDiagrams(article);
      scroll.scrollTop = top;
      return true;
    };
    try {
      if (!(await set())) return;
    } catch (error) {
      if (!signal.aborted) this.fail(error);
      return;
    }
    this.body.replaceChildren(status, scroll);
    this.stops.push(
      source.subscribe(() => {
        clearTimeout(this.settling);
        this.settling = window.setTimeout(() => {
          void set().catch((error: unknown) => {
            if (signal.aborted) return;
            status.hidden = false;
            status.textContent = this.options.words.failed;
            status.title = this.options.detail(error);
          });
        }, SETTLE);
      }),
    );
    const heard = source.interruptions?.((broken: LineInterruption | null) => {
      status.hidden = !broken;
      status.textContent = broken ? this.options.words.interrupted : "";
      status.title = broken?.why ?? "";
    });
    if (heard) this.stops.push(heard);
  }

  /** The file as its lines, numbered, in its language's colours. */
  private showLines(source: FileLines): void {
    const { words, path } = this.options;
    const viewport = new LineViewport(source, {
      label: leafOf(path),
      paint: linePaint(path),
      failed: (error) => (this.options.gone(error) ? words.missing : words.failed),
      interrupted: words.interrupted,
    });
    viewport.element.classList.add("fv-lines");
    const digits = () => viewport.element.style.setProperty("--line-digits", String(Math.max(2, String(source.extent.length).length)));
    digits();
    this.stops.push(source.subscribe(digits));
    this.viewport = viewport;
    this.body.replaceChildren(viewport.element);
  }

  private fail(error: unknown): void {
    const { words } = this.options;
    const note = document.createElement("p");
    note.className = "fv-note";
    note.textContent = this.options.gone(error) ? words.missing : words.failed;
    note.title = this.options.detail(error);
    this.body.replaceChildren(note);
  }

  /** The pictures the prose names by a path, drawn small by the machine
   *  holding them; a path of its own is read from the file's folder. */
  private placePictures(article: HTMLElement): void {
    const picture = this.options.picture;
    for (const image of article.querySelectorAll<HTMLImageElement>("img")) {
      const named = image.dataset.path ?? relative(image.getAttribute("src") ?? "");
      if (!named) continue;
      const path = image.dataset.path ? named : join(this.folder, named);
      image.removeAttribute("data-picture");
      image.removeAttribute("src");
      image.dataset.filePath = path;
      if (!picture) continue;
      void picture(this.options.machine, path, PICTURE).then(
        (address) => {
          if (!this.request.signal.aborted) image.src = address;
        },
        (error: unknown) => {
          image.title = this.options.detail(error);
        },
      );
    }
  }

  /** A link in the prose: a file it names opens as this one did, one named
   *  from the file's folder too; a link within the page goes nowhere, as
   *  the window's own address is the interface's. */
  private pressed(event: MouseEvent): void {
    const target = event.target as Element;
    const image = target.closest<HTMLImageElement>("img[data-file-path]");
    if (image) {
      event.preventDefault();
      this.options.open(this.options.machine, image.dataset.filePath!);
      return;
    }
    const link = target.closest<HTMLAnchorElement>("a[href]");
    if (!link) return;
    const named = link.dataset.file ?? relative(link.getAttribute("href") ?? "");
    if (named) {
      event.preventDefault();
      this.options.open(this.options.machine, link.dataset.file ? named : join(this.folder, named));
      return;
    }
    if ((link.getAttribute("href") ?? "").startsWith("#")) event.preventDefault();
  }

  private get folder(): string {
    const { path } = this.options;
    return path.slice(0, path.length - leafOf(path).length);
  }
}

/** A path a link names from the file's own folder; null for an address
 *  of the web, a path from the root, or a place within the page. */
function relative(href: string): string | null {
  if (!href || href.startsWith("#") || /^[a-z][a-z0-9+.-]*:/i.test(href) || filePathOf(href)) return null;
  try {
    return decodeURI(href.replace(/[?#].*$/, ""));
  } catch {
    return href.replace(/[?#].*$/, "");
  }
}

/** `name` read from `folder`, its `.` and `..` walked. */
function join(folder: string, name: string): string {
  const slash = /^[A-Za-z]:/.test(folder) || folder.includes("\\") ? (folder.includes("/") ? "/" : "\\") : "/";
  const parts = `${folder}${name}`.split(/[\\/]/);
  const walked: string[] = [];
  for (const part of parts) {
    if (part === "." || (part === "" && walked.length > 0)) continue;
    if (part === ".." && walked.length > 1) walked.pop();
    else walked.push(part);
  }
  return walked.join(slash);
}

/** Draws a line numbered, in its language's colours where it has one. */
function linePaint(path: string): LinePaint {
  const lang = language(path);
  return (row, line, index, match) => {
    const number = document.createElement("span");
    number.className = "lv-number";
    number.textContent = String(index + 1);
    number.setAttribute("aria-hidden", "true");
    const text = document.createElement("span");
    text.className = "lv-text";
    if (match) {
      const mark = document.createElement("mark");
      mark.textContent = line.slice(match.start, match.end);
      text.append(line.slice(0, match.start), mark, line.slice(match.end));
    } else if (lang && line) text.innerHTML = hljs.highlight(line, { language: lang, ignoreIllegals: true }).value;
    else text.textContent = line;
    row.append(number, text);
  };
}
