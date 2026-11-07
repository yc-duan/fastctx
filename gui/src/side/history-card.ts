// The history a conversation's model works with, a page of its side
// (`world-history.md`, 界面上): its documents one to a line, the name and
// the line that says what it covers, the latest changed first and one taken
// down after the rest; pressing one reads it, and the way to write one
// closes the list. In a project it is the project's, under the project's
// name, which leads to the project, and what the conversation wrote in its
// own history before it joined follows it.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { HistoryWords } from "../project/history-words";
import type { HistoryCatalog, HistoryRevision } from "../world/history";
import type { SidePage } from "./pane";

export interface HistoryCardOptions {
  words: HistoryWords;
  /** Where a document is read. */
  link(name: string): string;
  write(): void;
  /** The project whose history it is, for a conversation in one. */
  project?: { name: string; href: string };
  /** Where a document of the conversation's own earlier history is read. */
  earlier?(name: string): string;
}

export class HistoryCard implements SidePage {
  readonly element: HTMLElement;
  private catalog: HistoryCatalog | null = null;
  private before: HistoryCatalog | null = null;
  private drawn = "";

  constructor(private readonly options: HistoryCardOptions) {
    this.element = document.createElement("section");
    this.element.className = "sp-history scroller";
    this.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-write-history]")) options.write();
    });
    this.draw();
  }

  show(catalog: HistoryCatalog | null): void {
    this.catalog = catalog;
    this.draw();
  }

  /** The conversation's own history from before it joined its project. */
  showEarlier(catalog: HistoryCatalog | null): void {
    this.before = catalog;
    this.draw();
  }

  private draw(): void {
    const { words, link, project, earlier } = this.options;
    const title = project
      ? `<h3 class="section-title sp-history-title"><a href="${escape(project.href)}">${escape(words.of(project.name))}</a></h3>`
      : "";
    const listed = rows(this.catalog?.documents ?? [], link, words.retired);
    const list = listed || `<p class="quiet">${escape(words.noDocuments)}</p>`;
    const write = `<button class="text-button sp-history-write" type="button" data-write-history>${glyph("plus", 16)}<span>${escape(words.newDocument)}</span></button>`;
    const before = earlier ? rows(this.before?.documents ?? [], earlier, words.retired) : "";
    const past = before ? `<h3 class="section-title sp-history-title">${escape(words.earlier)}</h3>${before}` : "";
    const html = `${title}${list}${write}${past}`;
    if (html === this.drawn) return;
    this.drawn = html;
    this.element.innerHTML = html;
  }
}

/** The documents one to a line, the latest changed first and those taken
 *  down after the rest; none when there are none. */
function rows(documents: HistoryRevision[], link: (name: string) => string, retired: string): string {
  if (!documents.length) return "";
  const sorted = [...documents].sort(
    (a, b) => Number(a.retired === true) - Number(b.retired === true) || Date.parse(b.author.at) - Date.parse(a.author.at),
  );
  return `<ul class="page-links">${sorted
    .map(
      (one) => `<li><a class="page-link" href="${escape(link(one.name))}" title="${escape(one.description)}">
          <span class="page-link-glyph">${glyph("history", 18)}</span>
          <span class="page-link-body">
            <span class="page-link-title">${escape(one.name)}${one.retired ? `<span class="tag">${escape(retired)}</span>` : ""}</span>
            <span class="page-link-about">${escape(one.description)}</span>
          </span>
        </a></li>`,
    )
    .join("")}</ul>`;
}
