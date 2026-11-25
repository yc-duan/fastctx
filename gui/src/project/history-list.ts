// A history's documents as a list, on a project's page and in a
// conversation's side: one row each, the latest changed first and those
// taken down after the rest, under a box that finds them by their name and
// what they cover. A row opens its document when pressed, yet is no link:
// a link takes the middle button for itself, and the list would not scroll
// under it the way the rest of the window does.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { HistoryRevision } from "../world/history";

export interface HistoryListOptions {
  /** Where a document is read. */
  link(name: string): string;
  /** The mark of a document taken down. */
  retired: string;
  /** What a row says where the pointer rests on it: what it covers, unless
   *  this says otherwise. */
  title?(one: HistoryRevision): string;
  /** The order the rows come in: the latest changed first by default. */
  changed?(one: HistoryRevision): number;
}

/** Whether `said` holds every word of `query`, case aside. An empty query
 *  finds anything. */
function holds(said: string, query: string): boolean {
  const lower = said.toLowerCase();
  return query.toLowerCase().split(/\s+/).filter(Boolean).every((word) => lower.includes(word));
}

/** Whether `one` is found by `query`: every word of it is in its name or in
 *  what it covers. */
export function historyFound(one: HistoryRevision, query: string): boolean {
  return holds(`${one.name} ${one.description}`, query);
}

/** The documents one to a line, those `query` does not find hidden; none
 *  when there are none. */
export function historyRows(documents: HistoryRevision[], options: HistoryListOptions, query = ""): string {
  if (!documents.length) return "";
  const changed = options.changed ?? ((one: HistoryRevision) => Date.parse(one.author.at));
  const sorted = [...documents].sort((a, b) => Number(a.retired === true) - Number(b.retired === true) || changed(b) - changed(a));
  return `<ul class="page-links">${sorted
    .map((one) => {
      const title = options.title ? options.title(one) : one.description;
      const hidden = historyFound(one, query) ? "" : " hidden";
      return `<li data-history-find="${escape(`${one.name} ${one.description}`)}"${hidden}><div class="page-link" role="link" tabindex="0" data-history-href="${escape(options.link(one.name))}" title="${escape(title)}">
          <span class="page-link-glyph">${glyph("journal", 18)}</span>
          <span class="page-link-body">
            <span class="page-link-title">${escape(one.name)}${one.retired ? `<span class="tag">${escape(options.retired)}</span>` : ""}</span>
            <span class="page-link-about">${escape(one.description)}</span>
          </span>
        </div></li>`;
    })
    .join("")}</ul>`;
}

/** The box that finds documents, holding `query`, and what it says when
 *  it finds none. */
export function historyFind(placeholder: string, none: string, query: string, documents: HistoryRevision[]): string {
  const missing = documents.length > 0 && !documents.some((one) => historyFound(one, query));
  return `<label class="focus-search history-find">${glyph("search", 16)}<input class="focus-filter history-filter" type="search" placeholder="${escape(placeholder)}" aria-label="${escape(placeholder)}" autocomplete="off" spellcheck="false" value="${escape(query)}"></label><p class="quiet history-none"${missing ? "" : " hidden"}>${escape(none)}</p>`;
}

/** Shows only the rows under `root` that `query` finds, and says so where
 *  none is found. */
export function filterHistory(root: HTMLElement, query: string): void {
  let shown = 0;
  let rows = 0;
  for (const row of root.querySelectorAll<HTMLElement>("li[data-history-find]")) {
    rows += 1;
    const found = holds(row.dataset.historyFind ?? "", query);
    row.hidden = !found;
    if (found) shown += 1;
  }
  for (const none of root.querySelectorAll<HTMLElement>(".history-none")) none.hidden = rows === 0 || shown > 0;
}

/** Opens a row's document when it is pressed, or when Enter or Space is
 *  pressed on it; and tells `found` what the box holds as it changes, the
 *  rows filtered already. */
export function followHistory(root: HTMLElement, found: (query: string) => void): void {
  const open = (row: HTMLElement) => {
    const href = row.dataset.historyHref;
    if (href) location.hash = href;
  };
  root.addEventListener("click", (event) => {
    if (event.button !== 0) return;
    const row = (event.target as HTMLElement).closest<HTMLElement>("[data-history-href]");
    if (row) open(row);
  });
  root.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    const row = (event.target as HTMLElement).closest<HTMLElement>("[data-history-href]");
    if (!row || row !== event.target) return;
    event.preventDefault();
    open(row);
  });
  root.addEventListener("input", (event) => {
    const box = (event.target as HTMLElement).closest<HTMLInputElement>(".history-filter");
    if (!box) return;
    filterHistory(root, box.value);
    found(box.value);
  });
}
