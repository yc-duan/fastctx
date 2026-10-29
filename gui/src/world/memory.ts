// A project's memory (`world-memory.md`): its journals, each written once
// and never changed, and its decisions, each kept in all its versions, as
// Markdown under a header of a name and a one-line description. Who wrote
// a version, when and where, is the World's stamp beside it, never the
// text's own word.

import { invoke } from "@tauri-apps/api/core";
import { observe } from "./delivery";
import type { Writer } from "./world";

export type MemoryKind = "journal" | "spec";

export interface MemoryAuthor {
  by: Writer;
  /** The machine it was written on. */
  on: string;
  at: string;
  host?: { agent: string; client?: string; version?: string } | null;
  model?: string | null;
  session?: string | null;
}

export interface MemoryRevision {
  kind: MemoryKind;
  name: string;
  description: string;
  version: number;
  author: MemoryAuthor;
}

export interface MemoryCatalog {
  journals: MemoryRevision[];
  specs: MemoryRevision[];
}

export interface MemoryDocument {
  revision: MemoryRevision;
  /** The Markdown as written, its header first. */
  text: string;
  /** Every version, the first first. */
  versions: MemoryRevision[];
}

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

export interface MemorySeam {
  catalog(town: string): Promise<MemoryCatalog>;
  read(town: string, name: string, version: number | null): Promise<MemoryDocument>;
  /** Writes a new journal, or the next version of a decision after
   *  `expected`; fails where another version came first. */
  save(town: string, kind: MemoryKind, name: string, text: string, expected: number | null): Promise<MemoryRevision>;
  /** Follows a project's catalog: as it stands, then after each change; the
   *  id resolves for `unwatch`. */
  watch(town: string, onCatalog: (catalog: MemoryCatalog) => void): Promise<number>;
  /** Memory is on or off for a whole account; null lets the World pick
   *  this computer's only one. */
  enabled(hub: string | null, account: string | null): Promise<boolean>;
  setEnabled(on: boolean, hub: string | null, account: string | null): Promise<void>;
}

export const memory: MemorySeam = {
  catalog: (project) => invoke<MemoryCatalog>("memory_list", { project }),
  read: (project, name, version) => invoke<MemoryDocument>("memory_read", { project, name, version }),
  // A decision is the next version after `expected`; a journal is written once.
  save: (project, kind, name, text, expected) =>
    kind === "spec"
      ? invoke<MemoryRevision>("memory_save_spec", { project, name, text, expected })
      : invoke<MemoryRevision>("memory_add_journal", { project, name, text }),
  watch: (project, onCatalog) => {
    return observe<{ kind: "catalog"; catalog: MemoryCatalog }, number>("memory_watch", { project }, (piece) => {
      // A broken watch keeps the catalog last seen until it comes back.
      if (piece.kind !== "catalog") return;
      try {
        onCatalog(piece.catalog);
      } catch (error) {
        console.error("a memory catalog could not be shown", error);
      }
    });
  },
  enabled: (hub, account) => invoke<{ enabled: boolean }>("memory_settings_read", { hub, account }).then((settings) => settings.enabled),
  setEnabled: (on, hub, account) => invoke<{ enabled: boolean }>("memory_settings_set", { hub, account, enabled: on }).then(() => undefined),
};
