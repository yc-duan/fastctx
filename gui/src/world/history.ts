// A project's or a conversation's history (`world-history.md`): Markdown
// documents, one small topic each, under a header of a name and a one-line
// description, every save a version. Who wrote a version, when and where, is
// the World's stamp beside it, never the text's own word.

import { invoke } from "@tauri-apps/api/core";
import { observe } from "./delivery";
import type { Writer } from "./world";

/** Whose history: a project's, or a conversation's outside any project. */
export type HistoryScope = { kind: "project"; project: string } | { kind: "conversation"; resident: string; machine: string };

export interface HistoryAuthor {
  by: Writer;
  /** The machine it was written on; left out where the reader may not use it. */
  on?: string;
  at: string;
  host?: { agent: string; client?: string; version?: string } | null;
  model?: string | null;
}

export interface HistoryRevision {
  name: string;
  description: string;
  version: number;
  /** Taken down from the interface: models are no longer given it. */
  retired?: true;
  author: HistoryAuthor;
}

export interface HistoryCatalog {
  /** Each document's newest version, the latest changed first. */
  documents: HistoryRevision[];
}

export interface HistoryDocument {
  revision: HistoryRevision;
  /** The Markdown as written, its header first. */
  text: string;
  /** Every version, the first first. */
  versions: HistoryRevision[];
}

/** A document's names: lower-case letters, digits and hyphens. */
export const NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** The header's description and what follows it; text without a header
 *  is all body. */
export function splitHeader(text: string): { description: string | null; body: string } {
  const lines = text.split("\n");
  if (lines[0]?.trim() !== "---") return { description: null, body: text };
  let description: string | null = null;
  for (let at = 1; at < lines.length; at++) {
    const line = lines[at]!.replace(/\r$/, "");
    if (line === "---") return { description, body: lines.slice(at + 1).join("\n").replace(/^\n+/, "") };
    const colon = line.indexOf(":");
    if (colon > 0 && line.slice(0, colon) === "description") description = line.slice(colon + 1).trim();
  }
  return { description: null, body: text };
}

/** A document's text, as the World keeps it: the header, then the body. */
export function composeText(name: string, description: string, body: string): string {
  return `---\nname: ${name}\ndescription: ${description.replace(/\s+/g, " ").trim()}\n---\n\n${body.replace(/^\n+/, "")}`;
}

export interface HistorySeam {
  catalog(scope: HistoryScope): Promise<HistoryCatalog>;
  read(scope: HistoryScope, name: string, version: number | null): Promise<HistoryDocument>;
  /** Writes a new document (`expected` null), or the next version after
   *  `expected`; fails where another version came first. */
  save(scope: HistoryScope, name: string, text: string, expected: number | null): Promise<HistoryRevision>;
  retire(scope: HistoryScope, name: string, expected: number): Promise<HistoryRevision>;
  /** Takes a version's text as the newest, bringing a retired one back. */
  restore(scope: HistoryScope, name: string, version: number): Promise<HistoryRevision>;
  /** Follows the catalog: as it stands, then after each change; the id
   *  resolves for `unwatch`. */
  watch(scope: HistoryScope, onCatalog: (catalog: HistoryCatalog) => void): Promise<number>;
}

export const history: HistorySeam = {
  catalog: (scope) => invoke<HistoryCatalog>("history_list", { scope }),
  read: (scope, name, version) => invoke<HistoryDocument>("history_read", { scope, name, version }),
  save: (scope, name, text, expected) => invoke<HistoryRevision>("history_save", { scope, name, text, expected }),
  retire: (scope, name, expected) => invoke<HistoryRevision>("history_retire", { scope, name, expected }),
  restore: (scope, name, version) => invoke<HistoryRevision>("history_restore", { scope, name, version }),
  watch: (scope, onCatalog) =>
    observe<{ kind: "catalog"; catalog: HistoryCatalog }, number>("history_watch", { scope }, (piece) => {
      // A broken watch keeps the catalog last seen until it comes back.
      if (piece.kind !== "catalog") return;
      try {
        onCatalog(piece.catalog);
      } catch (error) {
        console.error("a history catalog could not be shown", error);
      }
    }),
};
