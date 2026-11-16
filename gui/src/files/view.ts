// A file opened (`gui-files`): where it is, quiet on the first line beside
// what can be done with it; its name; then the file itself, or its
// changes, whichever the switch under the name chooses. What it holds is
// drawn as its kind asks: Markdown as prose, other text as its lines,
// numbered and coloured for their language, read from the machine a screen
// at a time and followed as it changes; a picture as the picture, made
// small by the machine holding it; anything else is said to be opened with
// this device's program. Its changes stand the latest first, each under who
// made it and when, its lines a row to a line; a change is read only when
// it is opened.
//
// One view serves everywhere a file is opened: beside a machine's folders,
// and in a conversation's side over its parts.

import hljs from "highlight.js/lib/common";
import { drawDiagrams } from "../conversation/diagrams";
import { language, type DiffRow } from "../conversation/diff";
import { fileKind, isMarkdown, leafOf } from "../conversation/file-kind";
import { glyph } from "../conversation/glyphs";
import { escape, filePathOf, render } from "../conversation/prose";
import { LineViewport, type LineInterruption, type LinePaint, type LineSource } from "../longview";
import { waiting } from "../app/waiting";
import { changeLine, diffLines, figures, type Named } from "./changed";
import { split, type FileChange } from "./model";
import type { FilesWords } from "./words";

/** A file's lines as the machine holding it gives them, let go when done. */
export type FileLines = LineSource & { close(): Promise<void> };

export interface ViewSeams {
  lines(machine: string, path: string): Promise<FileLines>;
  /** A picture made small by the machine holding it, as an address an
   *  `<img>` takes. */
  picture?: ((machine: string, path: string, max: number) => Promise<string>) | undefined;
  /** Opens it with this device's program for its kind. */
  outside?: ((machine: string, path: string) => void) | undefined;
  /** Another file a link in it names. */
  open(machine: string, path: string): void;
  /** Whether a failure says the file is not there. */
  gone(error: unknown): boolean;
  /** A failure's original words, for the pointer resting on what says it. */
  detail(error: unknown): string;
  named?: Named | undefined;
}

export type ViewTab = "content" | "changes";

export interface FileViewOptions {
  words: FilesWords;
  seams: ViewSeams;
  machine: string;
  path: string;
  /** Its changes, the latest first. */
  changes: FileChange[];
  tab?: ViewTab | undefined;
  now(): number;
  /** Where it is shown among its machine's folders, offered in the head. */
  folders?: string | undefined;
  /** Its changes are all one conversation's, which is not said on each. */
  own?: boolean | undefined;
  /** It stands on a tab that says its name: its head is one line, where it
   *  is and the switch. */
  tabbed?: boolean | undefined;
}

/** Markdown up to this many lines is set as prose; a longer one is shown
 *  as its lines, as any other text. */
const PROSE_LINES = 4000;
/** How long the prose waits after a change before it is set again. */
const SETTLE = 300;
/** Pixels on the longer side of a picture: twice the widest it is drawn. */
const PICTURE = 1600;
/** The changes open as the list is first shown; the rest open when pressed. */
const OPEN = 3;
/** The changes listed before the earlier ones are only counted. */
const LISTED = 40;

export class FileView {
  readonly element = document.createElement("div");
  private readonly body: HTMLElement;
  private readonly request = new AbortController();
  private source: FileLines | null = null;
  private viewport: LineViewport | null = null;
  private stops: (() => void)[] = [];
  private settling = 0;
  private tab: ViewTab;
  private changes: FileChange[];
  /** The changes opened, and those whose lines were read. */
  private readonly opened = new Set<string>();
  private readonly rows = new Map<string, DiffRow[] | null | "reading">();
  private contentDrawn = false;
  private readonly content = document.createElement("div");
  private readonly changed = document.createElement("div");

  constructor(private readonly options: FileViewOptions) {
    const { words, machine, path } = options;
    const { folder, name } = split(path);
    this.changes = options.changes;
    const gone = this.changes[0]?.how === "removed";
    this.tab = options.tab ?? (gone ? "changes" : "content");
    if (!this.changes.length) this.tab = "content";
    for (const change of this.changes.slice(0, OPEN)) this.opened.add(change.id);
    this.element.className = "fx-view";
    // Where it is, quiet, and the way to it among its machine's folders;
    // then its name, with the switch between what it holds and its
    // changes on the same ruled line.
    const place = `${glyph("machine", 14)}<span class="fx-machine">${escape(machine)}</span><span class="fx-folder"><bdi>${escape(folder)}</bdi></span>`;
    const where = options.folders
      ? `<a class="fx-place" href="${escape(options.folders)}" title="${escape(`${words.view.inFolders}\n${machine}  ${path}`)}">${place}</a>`
      : `<span class="fx-place" title="${escape(`${machine}  ${path}`)}">${place}</span>`;
    const out = options.seams.outside ? `<button class="icon-button fx-out" type="button" data-act="outside" aria-label="${escape(words.view.outside)}" title="${escape(words.view.outside)}">${glyph("launch", 16)}</button>` : "";
    const tabs = `<div class="segmented fx-tabs" role="radiogroup" aria-label="${escape(words.view.tabs)}"></div>`;
    this.element.innerHTML = `<header class="fx-head">${
      options.tabbed
        ? `<div class="fx-title fx-brief">${where}<span class="fx-sum"></span>${tabs}${out}</div>`
        : `<div class="fx-where">${where}${out}</div>
        <div class="fx-title"><div class="fx-named"><h2 class="fx-name">${escape(name)}</h2><span class="fx-sum"></span></div>${tabs}</div>`
    }</header>
      <div class="fx-body"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".fx-body")!;
    this.content.className = "fx-content";
    this.changed.className = "fx-changes scroller";
    this.body.append(this.content, this.changed);
    this.element.addEventListener("click", (event) => this.pressed(event));
    this.element.addEventListener("keydown", (event) => {
      const target = event.target as HTMLElement;
      if ((event.key === "Enter" || event.key === " ") && target.matches("[role='button']")) {
        event.preventDefault();
        target.click();
      }
    });
    this.drawTabs();
    this.show();
  }

  leave(): void {
    this.request.abort();
    clearTimeout(this.settling);
    for (const stop of this.stops) stop();
    this.stops = [];
    this.viewport?.destroy();
    this.viewport = null;
    const source = this.source;
    this.source = null;
    if (source) void source.close().catch((error: unknown) => console.warn("a file's lines could not be let go", error));
  }

  /** Turns it to what it holds, or to its changes where it has any. */
  turn(tab: ViewTab): void {
    if (tab === this.tab || (tab === "changes" && !this.changes.length)) return;
    this.tab = tab;
    this.drawTabs();
    this.show();
  }

  /** Its changes as they now stand: drawn again only where they differ. */
  update(changes: FileChange[]): void {
    const same = changes.length === this.changes.length && changes.every((change, index) => change.id === this.changes[index]!.id && change.added === this.changes[index]!.added && change.removed === this.changes[index]!.removed);
    if (same) return;
    const first = !this.changes.length;
    this.changes = changes;
    if (first) for (const change of changes.slice(0, OPEN)) this.opened.add(change.id);
    this.drawTabs();
    if (this.tab === "changes") this.drawChanges();
  }

  private drawTabs(): void {
    const { words } = this.options;
    const tabs = this.element.querySelector<HTMLElement>(".fx-tabs")!;
    const sum = this.element.querySelector<HTMLElement>(".fx-sum")!;
    const added = this.changes.reduce((total, change) => total + change.added, 0);
    const removed = this.changes.reduce((total, change) => total + change.removed, 0);
    sum.innerHTML = this.changes.length ? figures(added, removed) : "";
    tabs.hidden = !this.changes.length;
    const tab = (id: ViewTab, said: string) => `<button type="button" class="segment${id === this.tab ? " chosen" : ""}" role="radio" aria-checked="${id === this.tab}" data-tab="${id}">${escape(said)}</button>`;
    // How many changes is said only where there are several.
    tabs.innerHTML = `${tab("content", words.view.content)}${tab("changes", words.view.changes(this.changes.length > 1 ? this.changes.length : 0))}`;
  }

  private show(): void {
    this.content.hidden = this.tab !== "content";
    this.changed.hidden = this.tab !== "changes";
    if (this.tab === "content") {
      if (!this.contentDrawn) {
        this.contentDrawn = true;
        void this.read();
      }
    } else this.drawChanges();
  }

  private pressed(event: MouseEvent): void {
    const target = event.target as HTMLElement;
    const { machine, path, seams } = this.options;
    const act = target.closest<HTMLElement>("[data-act]");
    if (act?.dataset.act === "outside") return seams.outside?.(machine, path);
    const tab = target.closest<HTMLElement>("[data-tab]")?.dataset.tab as ViewTab | undefined;
    if (tab) {
      if (tab === this.tab) return;
      this.tab = tab;
      this.drawTabs();
      this.show();
      return;
    }
    if (target.closest(".fx-change-head a")) return;
    const head = target.closest<HTMLElement>(".fx-change-head");
    if (head) {
      const section = head.parentElement!;
      const id = section.dataset.change!;
      const open = !this.opened.has(id);
      if (open) this.opened.add(id);
      else this.opened.delete(id);
      head.setAttribute("aria-expanded", String(open));
      if (open) this.fill(section, id);
      void section.offsetHeight;
      section.classList.toggle("open", open);
      return;
    }
    if (this.content.contains(target)) this.link(event);
  }

  // The changes.

  private drawChanges(): void {
    const { words, seams } = this.options;
    const now = this.options.now();
    if (!this.changes.length) {
      this.changed.innerHTML = `<p class="fx-note">${escape(words.view.noChanges)}</p>`;
      return;
    }
    const listed = this.changes.slice(0, LISTED);
    // A conversation's one change of the file needs no line of its own: the
    // head already says how many lines.
    const lone = !!this.options.own && this.changes.length === 1;
    this.changed.innerHTML = `${listed
      .map((change) => {
        const line = changeLine(this.options.own ? { ...change, session: null, agent: null, from: null } : change, words, seams.named, now);
        // What has no lines kept is its line alone: nothing opens under it.
        if (!change.rows) return `<section class="fx-change bare"><div class="fx-change-line">${line}</div></section>`;
        const open = lone || this.opened.has(change.id);
        const head = lone ? "" : `<div class="fx-change-head" role="button" tabindex="0" aria-expanded="${open}">${line}${glyph("chevron", 16)}</div>`;
        return `<section class="fx-change${open ? " open" : ""}" data-change="${escape(change.id)}">${head}<div class="fx-change-fold"><div class="fx-change-body"></div></div></section>`;
      })
      .join("")}${this.changes.length > LISTED ? `<p class="fx-note">${escape(words.view.earlier(this.changes.length - LISTED))}</p>` : ""}`;
    for (const section of this.changed.querySelectorAll<HTMLElement>(".fx-change.open")) this.fill(section, section.dataset.change!);
  }

  /** Draws an opened change's lines, read the first time it is opened. */
  private fill(section: HTMLElement, id: string): void {
    const { words, path } = this.options;
    const body = section.querySelector<HTMLElement>(".fx-change-body")!;
    const change = this.changes.find((at) => at.id === id);
    if (!change) return;
    const held = this.rows.get(id);
    const note = (text: string) => `<p class="fx-note">${escape(text)}</p>`;
    if (held === "reading") return;
    if (held !== undefined) {
      body.innerHTML = held === null || !held.length ? note(change.seen ? words.view.seenOnly : change.uncounted ? words.view.uncounted : words.view.notKept) : diffLines(held, path, words);
      return;
    }
    if (!change.rows) {
      this.rows.set(id, null);
      return this.fill(section, id);
    }
    this.rows.set(id, "reading");
    body.innerHTML = waiting({ label: words.view.reading, shape: "lines", rows: Math.min(8, Math.max(2, change.added + change.removed)) });
    void change.rows().then(
      (rows) => rows,
      () => null,
    ).then((rows) => {
      if (this.request.signal.aborted) return;
      this.rows.set(id, rows);
      const still = [...this.changed.querySelectorAll<HTMLElement>(".fx-change")].find((at) => at.dataset.change === id);
      if (still) this.fill(still, id);
    });
  }

  // What the file holds.

  private async read(): Promise<void> {
    const { options } = this;
    const { words, machine, path, seams } = options;
    const { signal } = this.request;
    if (this.changes[0]?.how === "removed") return this.say(words.view.removed);
    const kind = fileKind(path);
    if (kind === "picture") return this.picture();
    if (kind === "outside") return this.outside();
    this.content.innerHTML = waiting({ label: words.view.reading, shape: "lines", rows: 8 });
    let source: FileLines;
    try {
      source = await seams.lines(machine, path);
    } catch (error) {
      if (!signal.aborted) this.fail(error);
      return;
    }
    if (signal.aborted) {
      void source.close().catch(() => {});
      return;
    }
    this.source = source;
    if (isMarkdown(path) && source.extent.length <= PROSE_LINES) await this.prose(source);
    else this.showLines(source);
  }

  private say(text: string, title = ""): void {
    const note = document.createElement("p");
    note.className = "fx-note";
    note.textContent = text;
    note.title = title;
    this.content.replaceChildren(note);
  }

  private fail(error: unknown): void {
    const { words } = this.options;
    this.say(this.options.seams.gone(error) ? words.view.missing : words.view.failed, this.options.seams.detail(error));
  }

  private picture(): void {
    const { words, machine, path, seams } = this.options;
    if (!seams.picture) return this.outside();
    this.content.innerHTML = waiting({ label: words.view.reading, shape: "lines", rows: 6 });
    void seams.picture(machine, path, PICTURE).then(
      (address) => {
        if (this.request.signal.aborted) return;
        const figure = document.createElement("figure");
        figure.className = "fx-picture scroller";
        const image = document.createElement("img");
        image.alt = leafOf(path);
        image.src = address;
        figure.append(image);
        this.content.replaceChildren(figure);
      },
      (error: unknown) => {
        if (!this.request.signal.aborted) this.fail(error);
      },
    );
  }

  /** A file that is no text and no picture: this device's program has it. */
  private outside(): void {
    const { words, seams } = this.options;
    this.content.innerHTML = `<div class="fx-outside">${glyph("file", 28)}<p>${escape(words.view.notShown)}</p>${seams.outside ? `<button class="quiet-button" type="button" data-act="outside">${escape(words.view.openWith)}</button>` : ""}</div>`;
  }

  /** The file as prose, set again a moment after it changes there, where
   *  the reader was left. */
  private async prose(source: FileLines): Promise<void> {
    const { signal } = this.request;
    const { words } = this.options;
    const scroll = document.createElement("div");
    scroll.className = "fx-scroll scroller";
    const article = document.createElement("article");
    article.className = "prose fx-prose";
    scroll.append(article);
    const status = document.createElement("p");
    status.className = "fx-status";
    status.hidden = true;
    const set = async (): Promise<boolean> => {
      const length = source.extent.length;
      // Grown past what prose is set for, it is shown as its lines.
      if (length > PROSE_LINES) {
        this.content.replaceChildren();
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
    this.content.replaceChildren(status, scroll);
    this.stops.push(
      source.subscribe(() => {
        clearTimeout(this.settling);
        this.settling = window.setTimeout(() => {
          void set().catch((error: unknown) => {
            if (signal.aborted) return;
            status.hidden = false;
            status.textContent = words.view.failed;
            status.title = this.options.seams.detail(error);
          });
        }, SETTLE);
      }),
    );
    const heard = source.interruptions?.((broken: LineInterruption | null) => {
      status.hidden = !broken;
      status.textContent = broken ? words.view.interrupted : "";
      status.title = broken?.why ?? "";
    });
    if (heard) this.stops.push(heard);
  }

  /** The file as its lines, numbered, in its language's colours. */
  private showLines(source: FileLines): void {
    const { words, path, seams } = this.options;
    const viewport = new LineViewport(source, {
      label: leafOf(path),
      paint: linePaint(path),
      failed: (error) => (seams.gone(error) ? words.view.missing : words.view.failed),
      interrupted: words.view.interrupted,
    });
    viewport.element.classList.add("fx-lines");
    const digits = () => viewport.element.style.setProperty("--line-digits", String(Math.max(2, String(source.extent.length).length)));
    digits();
    this.stops.push(source.subscribe(digits));
    this.viewport = viewport;
    this.content.replaceChildren(viewport.element);
  }

  /** The pictures the prose names by a path, drawn small by the machine
   *  holding them; a path of its own is read from the file's folder. */
  private placePictures(article: HTMLElement): void {
    const picture = this.options.seams.picture;
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
          image.title = this.options.seams.detail(error);
        },
      );
    }
  }

  /** A link in the prose: a file it names opens as this one did, one named
   *  from the file's folder too; a link within the page goes nowhere, as
   *  the window's own address is the interface's. */
  private link(event: MouseEvent): void {
    const target = event.target as Element;
    const { machine, seams } = this.options;
    const image = target.closest<HTMLImageElement>("img[data-file-path]");
    if (image) {
      event.preventDefault();
      seams.open(machine, image.dataset.filePath!);
      return;
    }
    const link = target.closest<HTMLAnchorElement>("a[href]");
    if (!link) return;
    const named = link.dataset.file ?? relative(link.getAttribute("href") ?? "");
    if (named) {
      event.preventDefault();
      seams.open(machine, link.dataset.file ? named : join(this.folder, named));
      return;
    }
    if ((link.getAttribute("href") ?? "").startsWith("#")) event.preventDefault();
  }

  private get folder(): string {
    return split(this.options.path).folder;
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
