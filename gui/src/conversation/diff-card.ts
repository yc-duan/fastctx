// A change to a file, opened under its row: one solid card with a head that
// says which file it is and where, and the change itself beneath, scrolling
// in its box from the start (`long-text.ts`). The head names the machine,
// then the path, its folders quiet and cut from the front so the file's own
// name always shows; at its end, a copy of the change as a unified diff, as
// git writes one, and the way to the whole of it in a popup.

import { glyph } from "./glyphs";
import { escape } from "./prose";
import { decodeRow } from "./diff-lines";
import type { DiffRow } from "./diff";

export interface DiffCardWords {
  copy: string;
  copied: string;
}

export interface DiffCard {
  readonly element: HTMLElement;
  /** Where the change itself goes. */
  readonly body: HTMLElement;
  /** Puts a control at the end of the head, before the copy. */
  act(control: HTMLElement): void;
}

/** How a file was changed, as far as a diff's names say it. */
export type DiffKind = "added" | "deleted" | "modified";

/** A card for the change to `path` on `machine`; `copy` gives its unified
 *  diff when asked. */
export function diffCard(path: string, machine: string | null, words: DiffCardWords, copy: (signal: AbortSignal) => Promise<string>): DiffCard {
  const element = document.createElement("div");
  element.className = "diff-card";
  const cut = path.replace(/\\/g, "/").lastIndexOf("/");
  const folder = cut < 0 ? "" : path.slice(0, cut + 1);
  const name = cut < 0 ? path : path.slice(cut + 1);
  element.innerHTML = `<div class="diff-card-head" title="${escape(machine ? `${machine}  ${path}` : path)}">
      ${machine ? `<span class="diff-card-machine">${glyph("machine", 13)}<span>${escape(machine)}</span></span>` : ""}
      <span class="diff-card-path"><span class="diff-card-folder"><bdi>${escape(folder)}</bdi></span><span class="diff-card-name">${escape(name)}</span></span>
      <span class="diff-card-acts"><button class="icon-button diff-card-copy" type="button" aria-label="${escape(words.copy)}" title="${escape(words.copy)}">${glyph("copy", 15)}</button></span>
    </div><div class="diff-card-body"></div>`;
  const acts = element.querySelector<HTMLElement>(".diff-card-acts")!;
  const button = element.querySelector<HTMLButtonElement>(".diff-card-copy")!;
  let request: AbortController | null = null;
  button.addEventListener("click", () => {
    request?.abort();
    const asked = new AbortController();
    request = asked;
    void copy(asked.signal)
      .then((text) => navigator.clipboard?.writeText(text))
      .then(() => {
        if (asked.signal.aborted) return;
        button.classList.add("copied");
        button.innerHTML = glyph("check", 15);
        button.title = words.copied;
        setTimeout(() => {
          button.classList.remove("copied");
          button.innerHTML = glyph("copy", 15);
          button.title = words.copy;
        }, 1400);
      })
      .catch((error: unknown) => {
        if (!asked.signal.aborted) console.warn("a change could not be copied", error);
      });
  });
  return {
    element,
    body: element.querySelector<HTMLElement>(".diff-card-body")!,
    act: (control) => acts.prepend(control),
  };
}

/** A change as git writes it: its names, then each hunk with where it
 *  starts and how many lines it spans on each side, its lines marked. The
 *  rows are those drawn (`diff-lines.ts`), a gap between hunks. */
export function unifiedDiff(path: string, kind: DiffKind, lines: readonly string[]): string {
  const rows = lines.map((line) => decodeRow(line).row);
  const out = [`diff --git a/${path} b/${path}`, `--- ${kind === "added" ? "/dev/null" : `a/${path}`}`, `+++ ${kind === "deleted" ? "/dev/null" : `b/${path}`}`];
  let hunk: DiffRow[] = [];
  const flush = () => {
    if (!hunk.length) return;
    out.push(header(hunk), ...hunk.map((row) => `${row.kind === "add" ? "+" : row.kind === "del" ? "-" : " "}${row.text}`));
    hunk = [];
  };
  for (const row of rows) {
    if (row.kind === "gap") flush();
    else hunk.push(row);
  }
  flush();
  return `${out.join("\n")}\n`;
}

/** A hunk's `@@ -a,b +c,d @@`: where it begins on each side and how many
 *  lines of that side it holds; a side with none begins at the line before. */
function header(rows: readonly DiffRow[]): string {
  const old = rows.filter((row) => row.kind !== "add");
  const now = rows.filter((row) => row.kind !== "del");
  const start = (side: readonly DiffRow[], number: (row: DiffRow) => number | null, other: (row: DiffRow) => number | null) => {
    const first = side.map(number).find((n) => n !== null);
    if (first != null) return first;
    // Nothing on this side: it stands where the other side's first line
    // would have come, after the line before.
    const across = rows.map(other).find((n) => n !== null);
    return across == null ? 0 : Math.max(0, across - 1);
  };
  const a = start(old, (row) => row.old, (row) => row.new);
  const c = start(now, (row) => row.new, (row) => row.old);
  return `@@ -${a},${old.length} +${c},${now.length} @@`;
}
