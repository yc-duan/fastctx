// The history a conversation's model works with, a page of its side
// (`world-history.md`, 界面上): its documents one to a line, the name and
// the line that says what it covers, the latest changed first and one taken
// down after the rest, under a box that finds them; pressing one reads it,
// and the way to write one closes the list. In a project it is the
// project's, under the project's name, which leads to the project, and what
// the conversation wrote in its own history before it joined follows it.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { filterHistory, followHistory, historyFind, historyRows } from "../project/history-list";
import type { HistoryWords } from "../project/history-words";
import type { HistoryCatalog } from "../world/history";
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
  /** The box that finds documents: drawn once, so what is typed in it
   *  stays as the documents are drawn again. */
  private readonly find: HTMLElement;
  private readonly list: HTMLElement;
  private catalog: HistoryCatalog | null = null;
  private before: HistoryCatalog | null = null;
  private query = "";
  private drawn = "";

  constructor(private readonly options: HistoryCardOptions) {
    this.element = document.createElement("section");
    this.element.className = "sp-history scroller";
    this.find = document.createElement("div");
    this.list = document.createElement("div");
    this.element.append(this.find, this.list);
    this.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-write-history]")) options.write();
    });
    followHistory(this.element, (query) => {
      this.query = query;
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
    const documents = [...(this.catalog?.documents ?? []), ...(this.before?.documents ?? [])];
    if (!this.find.firstChild && documents.length) this.find.innerHTML = historyFind(words.find, words.noneFound, this.query, documents);
    const title = project
      ? `<h3 class="section-title sp-history-title"><a href="${escape(project.href)}">${escape(words.of(project.name))}</a></h3>`
      : "";
    const listed = historyRows(this.catalog?.documents ?? [], { link, retired: words.retired }, this.query);
    const list = listed || `<p class="quiet">${escape(words.noDocuments)}</p>`;
    const write = `<button class="text-button sp-history-write" type="button" data-write-history>${glyph("plus", 16)}<span>${escape(words.newDocument)}</span></button>`;
    const before = earlier ? historyRows(this.before?.documents ?? [], { link: earlier, retired: words.retired }, this.query) : "";
    const past = before ? `<h3 class="section-title sp-history-title">${escape(words.earlier)}</h3>${before}` : "";
    const html = `${title}${list}${write}${past}`;
    if (html === this.drawn) return;
    this.drawn = html;
    this.list.innerHTML = html;
    filterHistory(this.element, this.query);
  }
}
