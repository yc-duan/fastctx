// One of a project's journals or decisions, read (`world-memory.md`): its
// name over a ruled stroke, the line that says what it is, who wrote it,
// when and where, then its Markdown in the interface's type, in a region of
// its own that scrolls. A journal is written once and only read. A decision
// keeps every version: they stand down a hand-ruled line beside it, the one
// read ringed, and pressing one reads that version; an earlier one can be
// taken as the newest. Its words are edited where they stand, the header's
// line and its text in solid fields of the same type, and saved as the
// next version of the one that was read.

import { glyph } from "../conversation/glyphs";
import { escape, render } from "../conversation/prose";
import { factRow } from "../design/fact-row";
import { Sketch } from "../pen/sketch";
import { composeText, splitHeader, type MemoryAuthor, type MemoryDocument, type MemoryRevision } from "../world/memory";
import type { MemoryWords } from "./memory-words";
import "./memory.css";

export interface MemoryPageOptions {
  words: MemoryWords;
  head(title: string, actions?: string): string;
  /** The project it belongs to, to go back to. */
  project: { href: string; name: string };
  name: string;
  read(version: number | null): Promise<MemoryDocument>;
  /** Saves the next version of a decision after `expected`. */
  save(text: string, expected: number): Promise<MemoryRevision>;
  /** Who wrote it, in the interface's words, and where it links. */
  writer(author: MemoryAuthor): { name: string; href: string | null };
  /** The moment the page is read at, for its days. */
  now(): Date;
}

export class MemoryPage {
  readonly element: HTMLElement;
  private readonly doc: HTMLElement;
  private readonly rail: HTMLElement;
  private readonly editButton: HTMLButtonElement;
  private shown: MemoryDocument | null = null;
  /** The newest version known, to tell an older one read from it. */
  private newest = 0;
  private editing = false;

  constructor(private readonly options: MemoryPageOptions) {
    const { words } = options;
    this.element = document.createElement("div");
    this.element.className = "mm-page";
    this.element.innerHTML = `
      ${options.head(
        `<a class="crumb" href="${escape(options.project.href)}">${escape(options.project.name)}</a><span class="crumb-sep">/</span><h1 class="page-title mm-head-title">${escape(options.name)}</h1>`,
        `<button class="text-button mm-edit" type="button" hidden>${glyph("edit", 16)}<span>${escape(words.edit)}</span></button>`,
      )}
      <div class="mm-body"><article class="mm-doc scroller"></article><aside class="mm-rail" hidden></aside></div>`;
    this.doc = this.element.querySelector(".mm-doc")!;
    this.rail = this.element.querySelector(".mm-rail")!;
    this.editButton = this.element.querySelector(".mm-edit")!;
    this.editButton.addEventListener("click", () => this.edit());
    this.element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const version = target.closest<HTMLElement>("[data-version]")?.dataset.version;
      if (version) return void this.load(Number(version));
      const act = target.closest<HTMLElement>("[data-mm]")?.dataset.mm;
      if (act === "latest") void this.load(null);
      else if (act === "restore") void this.restore();
      else if (act === "save") void this.saveEdit();
      else if (act === "cancel") this.draw();
      else if (act === "reload") void this.load(null);
    });
    new ResizeObserver(() => this.drawRail()).observe(this.rail);
    void this.load(null);
  }

  private async load(version: number | null): Promise<void> {
    try {
      const doc = await this.options.read(version);
      this.shown = doc;
      this.newest = Math.max(this.newest, ...doc.versions.map((one) => one.version), doc.revision.version);
      this.editing = false;
      this.draw();
    } catch (error) {
      this.doc.innerHTML = `<p class="mm-said mm-wrong" title="${escape(String(error))}">${escape(this.options.words.unreadable)}</p>`;
    }
  }

  private draw(): void {
    const doc = this.shown;
    if (!doc) return;
    const { words } = this.options;
    const { revision } = doc;
    const spec = revision.kind === "spec";
    const older = spec && revision.version < this.newest;
    this.editing = false;
    this.editButton.hidden = !spec || older;
    const { body } = splitHeader(doc.text);
    const writer = this.options.writer(revision.author);
    const facts = [
      escape(words.date(revision.author.at, this.options.now())),
      writer.href ? `<a href="${escape(writer.href)}">${escape(writer.name)}</a>` : escape(writer.name),
      escape(revision.author.on),
      ...(spec ? [escape(words.version(revision.version))] : []),
    ];
    const note = older
      ? `<p class="mm-older"><span>${escape(words.older(revision.version))}</span><button class="text-button" type="button" data-mm="latest">${escape(words.latest)}</button><button class="text-button" type="button" data-mm="restore">${escape(words.restore)}</button></p>`
      : "";
    this.doc.innerHTML = `${note}<h2 class="mm-title">${escape(revision.name)}</h2><div class="mm-rule" aria-hidden="true"></div><p class="mm-about">${escape(revision.description)}</p><p class="mm-facts">${factRow(facts)}</p><div class="prose mm-text">${render(body)}</div>${spec ? this.phoneVersions(doc) : ""}`;
    this.drawRule();
    this.rail.hidden = !spec;
    if (spec) this.drawRail();
  }

  /** The stroke under the name, as long as the name. */
  private drawRule(): void {
    requestAnimationFrame(() => {
      const title = this.doc.querySelector<HTMLElement>(".mm-title");
      const rule = this.doc.querySelector<HTMLElement>(".mm-rule");
      if (!title || !rule) return;
      const range = document.createRange();
      range.selectNodeContents(title);
      const width = Math.max(48, Math.min(this.doc.clientWidth - 8, Math.round(range.getBoundingClientRect().width) + 6));
      rule.innerHTML = new Sketch(`memory-${this.options.name}`).rule([2, 4], [width - 2, 3.4], 1.2, "graphite").svg(width, 8, "mm-rule-svg");
    });
  }

  /** A decision's versions down a ruled line, the newest at the top: a drop
   *  of ink for each, the one read ringed in clay. */
  private drawRail(): void {
    const doc = this.shown;
    if (!doc || doc.revision.kind !== "spec" || this.rail.hidden) return;
    const { words } = this.options;
    const versions = [...doc.versions].sort((a, b) => b.version - a.version);
    this.rail.innerHTML = `<h3 class="mm-rail-title">${escape(words.versions)}</h3><div class="mm-rail-list"><div class="mm-rail-ink" aria-hidden="true"></div>${versions
      .map((one) => {
        const writer = this.options.writer(one.author);
        const current = one.version === doc.revision.version;
        return `<button class="mm-version${current ? " current" : ""}" type="button" data-version="${one.version}"${current ? ' aria-current="true"' : ""}><span class="mm-dot" aria-hidden="true"></span><span class="mm-version-lines"><span>${escape(words.version(one.version))}</span><span class="mm-version-who">${escape(words.date(one.author.at, this.options.now()))}　${escape(writer.name)}</span></span></button>`;
      })
      .join("")}</div>`;
    requestAnimationFrame(() => {
      const list = this.rail.querySelector<HTMLElement>(".mm-rail-list");
      if (!list) return;
      const box = list.getBoundingClientRect();
      const dots = [...list.querySelectorAll<HTMLElement>(".mm-dot")];
      if (!dots.length || box.height === 0) return;
      const at = (dot: HTMLElement): [number, number] => {
        const r = dot.getBoundingClientRect();
        return [r.left - box.left + r.width / 2, r.top - box.top + r.height / 2];
      };
      const sketch = new Sketch(`memory-rail-${this.options.name}`);
      const first = at(dots[0]!);
      const last = at(dots[dots.length - 1]!);
      if (dots.length > 1) sketch.rule([first[0], first[1] + 6], [last[0], last[1] - 6], 1.1, "graphite");
      dots.forEach((dot) => {
        const point = at(dot);
        const current = dot.closest(".current") !== null;
        sketch.blot(point, current ? 2.6 : 2.1, current ? "clay" : "graphite");
        if (current) sketch.ring(point, 6, 1.2, "clay");
      });
      list.querySelector<HTMLElement>(".mm-rail-ink")!.innerHTML = sketch.svg(box.width, box.height, "mm-rail-svg");
    });
  }

  /** On a narrow screen the versions follow the text. */
  private phoneVersions(doc: MemoryDocument): string {
    const { words } = this.options;
    const versions = [...doc.versions].sort((a, b) => b.version - a.version);
    if (versions.length < 2) return "";
    return `<section class="mm-phone-versions"><h3 class="mm-rail-title">${escape(words.versions)}</h3>${versions
      .map(
        (one) =>
          `<button class="mm-version-row${one.version === doc.revision.version ? " current" : ""}" type="button" data-version="${one.version}"><span>${escape(words.version(one.version))}</span><span class="mm-version-who">${escape(words.date(one.author.at, this.options.now()))}　${escape(this.options.writer(one.author).name)}</span></button>`,
      )
      .join("")}</section>`;
  }

  /** The decision's words in solid fields of the same type. */
  private edit(): void {
    const doc = this.shown;
    if (!doc || this.editing) return;
    this.editing = true;
    this.editButton.hidden = true;
    const { words } = this.options;
    const { body } = splitHeader(doc.text);
    this.doc.innerHTML = `<h2 class="mm-title">${escape(doc.revision.name)}</h2><div class="mm-rule" aria-hidden="true"></div><input class="mm-field mm-about-field" type="text" spellcheck="false" value="${escape(doc.revision.description)}" aria-label="${escape(words.description)}"><textarea class="mm-field mm-source" spellcheck="false" aria-label="${escape(words.body)}">${escape(body)}</textarea><p class="mm-said" hidden></p><div class="mm-acts"><button class="text-button" type="button" data-mm="cancel">${escape(words.cancel)}</button><button class="sheet-go" type="button" data-mm="save">${escape(words.save)}</button></div>`;
    this.drawRule();
    const source = this.doc.querySelector<HTMLTextAreaElement>(".mm-source")!;
    const fit = () => {
      source.style.height = "auto";
      source.style.height = `${source.scrollHeight}px`;
    };
    source.addEventListener("input", fit);
    source.addEventListener("keydown", (event) => {
      if (event.key === "Escape") this.draw();
      else if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) void this.saveEdit();
    });
    fit();
    source.focus();
  }

  private async saveEdit(): Promise<void> {
    const doc = this.shown;
    if (!doc) return;
    const about = this.doc.querySelector<HTMLInputElement>(".mm-about-field")?.value.trim() || doc.revision.description;
    const body = this.doc.querySelector<HTMLTextAreaElement>(".mm-source")?.value ?? "";
    await this.saveText(composeText(doc.revision.name, about, body), doc.revision.version);
  }

  /** An earlier version's words become the newest. */
  private async restore(): Promise<void> {
    const doc = this.shown;
    if (!doc) return;
    await this.saveText(doc.text, this.newest);
  }

  private async saveText(text: string, expected: number): Promise<void> {
    const { words } = this.options;
    const button = this.doc.querySelector<HTMLButtonElement>("[data-mm='save'], [data-mm='restore']");
    if (button) button.disabled = true;
    try {
      const saved = await this.options.save(text, expected);
      this.newest = Math.max(this.newest, saved.version);
      await this.load(null);
    } catch (error) {
      if (button) button.disabled = false;
      const said = this.doc.querySelector<HTMLElement>(".mm-said") ?? this.doc.appendChild(Object.assign(document.createElement("p"), { className: "mm-said" }));
      const stale = /version|expected|stale/i.test(String(error));
      said.hidden = false;
      said.classList.add("mm-wrong");
      said.title = String(error);
      said.innerHTML = stale
        ? `<span>${escape(words.changedElsewhere)}</span><button class="text-button" type="button" data-mm="reload">${escape(words.loadThat)}</button>`
        : escape(words.notSaved);
    }
  }
}
