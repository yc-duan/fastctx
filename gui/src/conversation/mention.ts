// Mentioning a file in the words: `@` and a few letters ask the agent's
// machine for files in the project's places, and the one chosen goes into
// the words as its path, for the agent to read with its own hands.

import { escape } from "./prose";

export interface FoundFile {
  /** The file's own absolute path on the agent's machine. */
  path: string;
}

export interface FileWords {
  /** Said before anything is typed after `@`. */
  hint: string;
  searching: string;
  none: string;
}

/** The `@` mention being typed just before the caret: what follows the
 *  `@`, and where the `@` stands. A `@` inside a word is not one. */
export function mentionAt(text: string, caret: number): { query: string; from: number } | null {
  const before = text.slice(0, caret);
  const found = /(?:^|\s)@([^\s@`]*)$/.exec(before);
  if (!found) return null;
  const query = found[1]!;
  return { query, from: before.length - query.length - 1 };
}

/** What a chosen file puts into the words. */
export function mentioned(file: FoundFile): string {
  return `\`${file.path}\` `;
}

/** A found file in the menu: its name, then the directory it is in. */
export function fileRow(file: FoundFile): string {
  const cut = Math.max(file.path.lastIndexOf("/"), file.path.lastIndexOf("\\"));
  return `<span class="menu-name">${escape(file.path.slice(cut + 1))}</span><span class="menu-what mention-dir">${escape(file.path.slice(0, cut + 1))}</span>`;
}
