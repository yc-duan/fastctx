// The history of a conversation outside any project, a page of the side
// (`world-history.md`, 界面上): its documents one to a line, the name and
// the line that says what it covers, the latest changed first; pressing one
// reads it, and the way to write one closes the list.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { HistoryWords } from "../project/history-words";
import type { HistoryCatalog } from "../world/history";
import type { SidePage } from "./pane";

export interface HistoryCardOptions {
  words: HistoryWords;
  /** Where a document is read. */
  link(name: string): string;
  write(): void;
}

export class HistoryCard implements SidePage {
  readonly element: HTMLElement;
  private drawn = "";

  constructor(private readonly options: HistoryCardOptions) {
    this.element = document.createElement("section");
    this.element.className = "sp-history scroller";
    this.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-write-history]")) options.write();
    });
    this.show(null);
  }

  show(catalog: HistoryCatalog | null): void {
    const { words, link } = this.options;
    const documents = [...(catalog?.documents ?? [])].sort(
      (a, b) => Number(a.retired === true) - Number(b.retired === true) || Date.parse(b.author.at) - Date.parse(a.author.at),
    );
    const write = `<button class="text-button sp-history-write" type="button" data-write-history>${glyph("plus", 16)}<span>${escape(words.newDocument)}</span></button>`;
    const list = documents.length
      ? `<ul class="page-links">${documents
          .map(
            (one) => `<li><a class="page-link" href="${escape(link(one.name))}">
          <span class="page-link-glyph">${glyph("history", 18)}</span>
          <span class="page-link-body">
            <span class="page-link-title">${escape(one.name)}${one.retired ? `<span class="tag">${escape(words.retired)}</span>` : ""}</span>
            <span class="page-link-about">${escape(one.description)}</span>
          </span>
        </a></li>`,
          )
          .join("")}</ul>`
      : `<p class="quiet">${escape(words.noDocuments)}</p>`;
    const html = `${list}${write}`;
    if (html === this.drawn) return;
    this.drawn = html;
    this.element.innerHTML = html;
  }
}
