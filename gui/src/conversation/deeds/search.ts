// A search through files as rg and grep print it: a path, a line number
// and the line, file by file; a search of one file leaves the path out.

import { wordsOf } from "./script";
import type { Found, OutputLines, SearchDeed } from "./types";

const HIT = /^((?:[A-Za-z]:)?[^:\n]+?):(\d+):(?:\d+:)?(.*)$/;
const LONE_HIT = /^(\d+):(.*)$/;
/** Options that take the word after them. */
const VALUED = new Set(["-g", "--glob", "-t", "--type", "-T", "--type-not", "-A", "-B", "-C", "-m", "--max-count", "-f", "--file", "--include", "--exclude", "-d", "--max-depth", "-M", "--max-columns", "--context", "--after-context", "--before-context"]);

/** What a search command looks for and whether it ignores case; null
 *  where it is not one. */
export function searchOf(command: string): { pattern: string | null; ignoreCase: boolean; one: string | null } | null {
  const words = wordsOf(command);
  let at = 0;
  if (words[0] === "git" && words[1] === "grep") at = 2;
  else if (words[0] === "rg" || words[0] === "grep" || words[0] === "egrep") at = 1;
  else return null;
  let pattern: string | null = null;
  let ignoreCase = false;
  const rest: string[] = [];
  for (let index = at; index < words.length; index += 1) {
    const word = words[index]!;
    if (word === "-e" || word === "--regexp") {
      pattern = words[index + 1] ?? null;
      index += 1;
      continue;
    }
    if (VALUED.has(word)) {
      index += 1;
      continue;
    }
    if (word.startsWith("-")) {
      if (/^-[a-zA-Z]*i/.test(word) || word === "--ignore-case") ignoreCase = true;
      continue;
    }
    if (pattern === null) pattern = word;
    else rest.push(word);
  }
  // One file searched: its lines come without its name.
  const one = rest.length === 1 && /\.[A-Za-z0-9]+$/.test(rest[0]!) ? rest[0]! : null;
  return { pattern, ignoreCase, one };
}

/** The hits among `lines`, file by file, after a search for `of`. */
export function searchHits(lines: OutputLines, of: { pattern: string | null; ignoreCase: boolean; one: string | null }): Found<SearchDeed> | null {
  const files: SearchDeed["files"] = [];
  let at = -1;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (!line) continue;
    const lone = of.one ? LONE_HIT.exec(line) : null;
    const hit = lone ? null : HIT.exec(line);
    if (!lone && !hit) continue;
    const path = lone ? of.one! : hit![1]!;
    const number = Number(lone ? lone[1] : hit![2]);
    const text = lone ? lone[2]! : hit![3]!;
    if (at < 0) at = index;
    let file = files[files.length - 1];
    if (file?.path !== path) {
      file = files.find((one) => one.path === path) ?? { path, hits: [] };
      if (!files.includes(file)) files.push(file);
    }
    file.hits.push({ line: number, text });
  }
  if (!files.length) return null;
  return { at, deed: { kind: "search", command: null, pattern: of.pattern, ignoreCase: of.ignoreCase, files } };
}
