// A file's lines as a run printed them: the World's own `view`, file by
// file under its headings, and the shell's ways of printing one file
// (`cat`, `sed -n`, `head`, `tail`, `nl`). Only a run that did nothing else
// is read as the file, since a file's lines say nothing of whose they are.

import { wordsOf } from "./script";
import type { FileDeed, FileRow, Found, OutputLines } from "./types";

const SECTION = /^=== (.+) \((.+)\) ===$/;
const SUMMARY = /^=== view \(/;
const NUMBERED = /^(\d+)\t(.*)$/;
const SKIPPED = /^=== lines (\d+)-(\d+) not shown ===$/;
const OF = /\bof (\d+)\b/;

/** The files a World `view` showed, each a deed; null where one of them
 *  was shown in a way not read here (an outline, a table, a picture), so
 *  that the run's own words show it. */
export function viewedFiles(lines: OutputLines, machine: string | null): Found<FileDeed>[] | null {
  const found: Found<FileDeed>[] = [];
  let open: FileDeed | null = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) {
      if (open) open.rows.push({ skipped: null });
      continue;
    }
    if (SUMMARY.test(line)) continue;
    const skipped = SKIPPED.exec(line);
    if (skipped && open) {
      open.rows.push({ skipped: Number(skipped[2]) - Number(skipped[1]) + 1 });
      continue;
    }
    const section = SECTION.exec(line);
    if (section) {
      // Lines of it shown: all of them (`314 lines`, `1 line`) or some
      // (`lines 1-300 of 447`), after what its encoding is, where said.
      if (!/\b(?:lines? \d|\d+ lines?\b)/.test(section[2]!) || /\bnot shown\b/.test(section[2]!)) return null;
      const { at, path } = placeOf(section[1]!);
      open = { kind: "file", command: null, machine: at ?? machine, path, rows: [], of: Number(OF.exec(section[2]!)?.[1] ?? NaN) || null };
      found.push({ at: index, deed: open });
      continue;
    }
    if (line.startsWith("=== ")) return null;
    const numbered = NUMBERED.exec(line);
    if (numbered && open) {
      open.rows.push({ line: Number(numbered[1]), text: numbered[2]! });
      continue;
    }
    if (open && line === "") continue;
    if (open) return null;
  }
  return found.length && found.every((one) => one.deed.rows.length) ? found : null;
}

/** A heading's machine and path: `lab:/srv/x.rs`, `E:/x.rs`, `/srv/x.rs`. */
function placeOf(said: string): { at: string | null; path: string } {
  const match = /^([^:/\\]+):((?:[A-Za-z]:)?[\\/].*)$/.exec(said);
  return match ? { at: match[1]!, path: match[2]! } : { at: null, path: said };
}

/** Where a shell command that prints one file begins in it, and which file;
 *  null where it is not one. */
export function printsFile(command: string): { path: string; first: number | null; numbered: boolean } | null {
  if (/[|><]/.test(command.replace(/'[^']*'|"[^"]*"/g, ""))) return null;
  const words = wordsOf(command);
  const files = (from: number) => words.slice(from).filter((word) => !word.startsWith("-"));
  switch (words[0]) {
    case "cat": {
      const named = files(1);
      return named.length === 1 ? { path: named[0]!, first: 1, numbered: words.includes("-n") } : null;
    }
    case "nl": {
      const named = files(1).filter((word) => !/^\d+$/.test(word));
      return named.length === 1 ? { path: named[0]!, first: 1, numbered: true } : null;
    }
    case "head": {
      const named = files(1).filter((word) => !/^\d+$/.test(word));
      return named.length === 1 ? { path: named[0]!, first: 1, numbered: false } : null;
    }
    case "tail": {
      const named = files(1).filter((word) => !/^\+?\d+$/.test(word));
      const from = words.find((word) => /^\+\d+$/.test(word));
      return named.length === 1 ? { path: named[0]!, first: from ? Number(from.slice(1)) : null, numbered: false } : null;
    }
    case "sed": {
      if (words[1] !== "-n") return null;
      const range = /^(\d+)(?:,(\d+))?p$/.exec(words[2] ?? "");
      const named = words.slice(3);
      return range && named.length === 1 ? { path: named[0]!, first: Number(range[1]), numbered: false } : null;
    }
    default:
      return null;
  }
}

/** The file one shell command printed, from where it began. */
export function printedFile(lines: OutputLines, printed: { path: string; first: number | null; numbered: boolean }, machine: string | null): Found<FileDeed> | null {
  const rows: FileRow[] = [];
  let line = printed.first;
  for (const said of lines) {
    if (said === null) {
      rows.push({ skipped: null });
      line = null;
      continue;
    }
    if (printed.numbered) {
      const numbered = /^\s*(\d+)\t(.*)$/.exec(said);
      if (numbered) {
        rows.push({ line: Number(numbered[1]), text: numbered[2]! });
        continue;
      }
    }
    rows.push({ line, text: said });
    if (line !== null) line += 1;
  }
  while (rows.length && "text" in rows[rows.length - 1]! && !(rows[rows.length - 1] as { text: string }).text) rows.pop();
  if (!rows.length) return null;
  return { at: 0, deed: { kind: "file", command: null, machine, path: printed.path, rows, of: null } };
}
