// One document of a history, read (`world-history.md`): a document and
// nothing else, its name over a ruled stroke and its Markdown in the reading
// type, in a column of its own that scrolls. Its versions are a sheet from
// the head, as a page's are: each to look at or take as the newest, and the
// way to take the document down at its foot. Edited where it stands, its
// description and its text in solid fields of the same type, and saved as
// the next version of the one read.

import { whyOf } from "../world/again";
import { Sheet } from "../app/confirm";
import { glyph } from "../conversation/glyphs";
import { escape, render } from "../conversation/prose";
import { Sketch } from "../pen/sketch";
import { composeText, splitHeader, type HistoryAuthor, type HistoryDocument, type HistoryRevision } from "../world/history";
import type { HistoryWords } from "./history-words";
import "../pages/pages.css";
import "./history.css";

export interface HistoryPageOptions {
  words: HistoryWords;
  head(title: string, actions?: string): string;
  /** Where it belongs, said in the head before its name. */
  back: { href: string; name: string };
  name: string;
  close: string;
  read(version: number | null): Promise<HistoryDocument>;
  /** Saves the next version after `expected`. */
  save(text: string, expected: number): Promise<HistoryRevision>;
  /** Asks first, then takes it down. */
  retire(expected: number, done: () => void): void;
  /** Takes a version's text as the newest. */
  restore(version: number): Promise<HistoryRevision>;
  /** Who wrote it, in the interface's words. */
  writer(author: HistoryAuthor): string;
  /** How long ago a moment was, short. */
  ago(at: Date): string;
}

export class HistoryPage {
  readonly element: HTMLElement;
  private readonly doc: HTMLElement;
  private readonly editButton: HTMLButtonElement;
  private readonly sheet = new Sheet("form-sheet page-sheet");
  private shown: HistoryDocument | null = null;
  /** The newest version known, to tell an older one read from it. */
  private newest = 0;
  private editing = false;

  constructor(private readonly options: HistoryPageOptions) {
    const { words } = options;
    this.element = document.createElement("div");
    this.element.className = "hd-page";
    this.element.innerHTML = `
      ${options.head(
        `<a class="crumb" href="${escape(options.back.href)}">${escape(options.back.name)}</a><span class="crumb-sep">/</span><h1 class="page-title hd-head-title">${escape(options.name)}</h1>`,
        `<button class="text-button hd-versions" type="button">${glyph("versions", 16)}<span>${escape(words.versions)}</span></button><button class="text-button hd-edit" type="button" hidden>${glyph("edit", 16)}<span>${escape(words.edit)}</span></button>`,
      )}
      <article class="hd-doc scroller"></article>`;
    this.doc = this.element.querySelector(".hd-doc")!;
    this.editButton = this.element.querySelector(".hd-edit")!;
    this.editButton.addEventListener("click", () => this.edit());
    this.element.querySelector(".hd-versions")!.addEventListener("click", () => this.showVersions());
    this.doc.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLElement>("[data-hd]")?.dataset.hd;
      if (act === "latest") void this.load(null);
      else if (act === "restore") void this.restoreShown();
      else if (act === "save") void this.saveEdit();
      else if (act === "cancel") this.draw();
      else if (act === "reload") void this.load(null);
    });
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
      this.doc.innerHTML = `<p class="hd-said hd-wrong" title="${escape(whyOf(error))}">${escape(this.options.words.unreadable)}</p>`;
    }
  }

  private draw(): void {
    const doc = this.shown;
    if (!doc) return;
    const { words } = this.options;
    const { revision } = doc;
    const older = revision.version < this.newest;
    const retired = !older && revision.retired === true;
    this.editing = false;
    this.editButton.hidden = older || retired;
    const { body } = splitHeader(doc.text);
    const note = older
      ? `<p class="hd-note"><span>${escape(words.older(revision.version))}</span><button class="text-button" type="button" data-hd="latest">${escape(words.latest)}</button><button class="text-button" type="button" data-hd="restore">${escape(words.restore)}</button></p>`
      : retired
        ? `<p class="hd-note"><span>${escape(words.retired)}</span><button class="text-button" type="button" data-hd="restore">${escape(words.bringBack)}</button></p>`
        : "";
    this.doc.innerHTML = `${note}<h2 class="hd-title">${escape(revision.name)}</h2><div class="hd-rule" aria-hidden="true"></div><div class="prose hd-text">${render(body)}</div>`;
    this.drawRule();
  }

  /** The stroke under the name, as long as the name. */
  private drawRule(): void {
    requestAnimationFrame(() => {
      const title = this.doc.querySelector<HTMLElement>(".hd-title");
      const rule = this.doc.querySelector<HTMLElement>(".hd-rule");
      if (!title || !rule) return;
      const range = document.createRange();
      range.selectNodeContents(title);
      const width = Math.max(48, Math.min(this.doc.clientWidth - 8, Math.round(range.getBoundingClientRect().width) + 6));
      rule.innerHTML = new Sketch(`history-${this.options.name}`).rule([2, 4], [width - 2, 3.4], 1.2, "graphite").svg(width, 8, "hd-rule-svg");
    });
  }

  /** Its versions, the newest first, as a page's are: each to look at or
   *  take as the newest; taking it down at the foot. */
  private showVersions(): void {
    const doc = this.shown;
    if (!doc) return;
    const { words } = this.options;
    const { sheet } = this;
    const versions = [...doc.versions].sort((a, b) => b.version - a.version);
    const latest = versions[0];
    sheet.body.innerHTML = `
      ${sheet.head(words.versions, this.options.close)}
      <ol class="page-versions-list">${versions
        .map((one) => {
          const shown = one.version === doc.revision.version;
          const tags = [one === latest ? words.current : "", one.retired ? words.retired : ""].filter(Boolean);
          return `<li class="page-version${shown ? " shown" : ""}">
            <div class="page-version-line"><span class="page-version-n">${escape(words.version(one.version))}</span>${tags.map((tag) => `<span class="tag">${escape(tag)}</span>`).join("")}</div>
            <div class="page-version-by">${escape(words.changed(this.options.writer(one.author), this.options.ago(new Date(one.author.at))))}</div>
            <div class="page-version-go">${shown ? "" : `<button class="text-button" type="button" data-look="${one.version}">${escape(words.look)}</button>`}${
              one === latest ? "" : `<button class="text-button" type="button" data-bring="${one.version}">${escape(words.restore)}</button>`
            }</div>
          </li>`;
        })
        .join("")}</ol>
      <footer class="sheet-foot">${latest && !latest.retired ? `<button class="text-button danger hd-retire" type="button">${escape(words.retire)}</button>` : "<span></span>"}<button class="sheet-go sheet-close" type="button">${escape(this.options.close)}</button></footer>`;
    sheet.body.onsubmit = (event) => event.preventDefault();
    sheet.body.querySelectorAll<HTMLButtonElement>("[data-look]").forEach((button) =>
      button.addEventListener("click", () => {
        sheet.close();
        const version = Number(button.dataset.look);
        void this.load(version === latest?.version ? null : version);
      }),
    );
    sheet.body.querySelectorAll<HTMLButtonElement>("[data-bring]").forEach((button) =>
      button.addEventListener("click", () => {
        sheet.close();
        void this.attempt(() => this.options.restore(Number(button.dataset.bring)));
      }),
    );
    sheet.body.querySelector(".hd-retire")?.addEventListener("click", () => {
      sheet.close();
      if (latest) this.options.retire(latest.version, () => void this.load(null));
    });
    sheet.show();
  }

  /** The description and the text in solid fields of the same type. */
  private edit(): void {
    const doc = this.shown;
    if (!doc || this.editing) return;
    this.editing = true;
    this.editButton.hidden = true;
    const { words } = this.options;
    const { body } = splitHeader(doc.text);
    this.doc.innerHTML = `<h2 class="hd-title">${escape(doc.revision.name)}</h2><div class="hd-rule" aria-hidden="true"></div><input class="hd-field hd-about-field" type="text" spellcheck="false" value="${escape(doc.revision.description)}" aria-label="${escape(words.description)}"><textarea class="hd-field hd-source" spellcheck="false" aria-label="${escape(words.text)}">${escape(body)}</textarea><p class="hd-said" hidden></p><div class="hd-acts"><button class="text-button" type="button" data-hd="cancel">${escape(words.cancel)}</button><button class="sheet-go" type="button" data-hd="save">${escape(words.save)}</button></div>`;
    this.drawRule();
    const source = this.doc.querySelector<HTMLTextAreaElement>(".hd-source")!;
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
    source.setSelectionRange(source.value.length, source.value.length);
  }

  private async saveEdit(): Promise<void> {
    const doc = this.shown;
    if (!doc) return;
    const about = this.doc.querySelector<HTMLInputElement>(".hd-about-field")?.value.trim() || doc.revision.description;
    const body = this.doc.querySelector<HTMLTextAreaElement>(".hd-source")?.value ?? "";
    await this.attempt(() => this.options.save(composeText(doc.revision.name, about, body), doc.revision.version));
  }

  /** The version read becomes the newest; a retired one comes back. */
  private async restoreShown(): Promise<void> {
    const doc = this.shown;
    if (!doc) return;
    await this.attempt(() => this.options.restore(doc.revision.version));
  }

  private async attempt(act: () => Promise<HistoryRevision>): Promise<void> {
    const { words } = this.options;
    const button = this.doc.querySelector<HTMLButtonElement>("[data-hd='save'], [data-hd='restore']");
    if (button) button.disabled = true;
    try {
      const saved = await act();
      this.newest = Math.max(this.newest, saved.version);
      await this.load(null);
    } catch (error) {
      if (button) button.disabled = false;
      const said = this.doc.querySelector<HTMLElement>(".hd-said") ?? this.doc.insertBefore(Object.assign(document.createElement("p"), { className: "hd-said" }), this.doc.firstChild);
      const why = whyOf(error);
      said.hidden = false;
      said.classList.add("hd-wrong");
      said.title = why;
      said.innerHTML = stale(error, why)
        ? `<span>${escape(words.changedElsewhere)}</span><button class="text-button" type="button" data-hd="reload">${escape(words.loadThat)}</button>`
        : escape(words.notSaved);
    }
  }
}

/** Whether a save failed because another version came first: as the World
 *  codes it, or as its words say where an older node gives no code. */
function stale(error: unknown, why: string): boolean {
  const code = typeof error === "object" && error !== null && "code" in error ? String((error as { code?: unknown }).code) : "";
  return /stale|conflict|version/i.test(code) || /\bversion\b|expected|stale|elsewhere/i.test(why);
}
