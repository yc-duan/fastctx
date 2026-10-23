// What TypeScript's compiler and ESLint said, each as they print it: tsc a
// line a diagnostic (`file(12,5): error TS2339: …`, or the pretty
// `file:12:5 - error TS2339: …` with the source under it), ESLint a file's
// path then a line for each problem under it.

import type { ChecksDeed, Diagnostic, Found, OutputLines } from "./types";

const TSC_PLAIN = /^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.+)$/;
const TSC_PRETTY = /^(.+?):(\d+):(\d+) - (error|warning) (TS\d+): (.+)$/;
const TSC_FOUND = /^Found (\d+) errors?\b/;

/** What tsc found, all of it one deed; a run that found nothing and said
 *  so is a clean one. */
export function tscChecks(lines: OutputLines): Found<ChecksDeed> | null {
  const items: Diagnostic[] = [];
  let at = -1;
  let open: Diagnostic | null = null;
  let none = false;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) {
      open = null;
      continue;
    }
    const head = TSC_PLAIN.exec(line) ?? TSC_PRETTY.exec(line);
    if (head) {
      if (at < 0) at = index;
      open = { level: head[4] as Diagnostic["level"], code: head[5]!, message: head[6]!, place: { path: head[1]!, line: Number(head[2]), column: Number(head[3]) }, snippet: [] };
      items.push(open);
      continue;
    }
    const found = TSC_FOUND.exec(line);
    if (found) {
      if (at < 0) at = index;
      if (found[1] === "0") none = true;
      open = null;
      continue;
    }
    if (open && line.trim()) open.snippet.push(line);
    else open = null;
  }
  if (at < 0 || (!items.length && !none)) return null;
  return { at, deed: { kind: "checks", command: null, items, clean: !items.length } };
}

const ESLINT_PROBLEM = /^\s+(\d+):(\d+)\s+(error|warning)\s+(.+?)(?:\s{2,}(\S+))?\s*$/;
const ESLINT_FILE = /^(?:[A-Za-z]:)?[\\/]\S.*$|^\S+\.(?:[cm]?[jt]sx?|vue|svelte)$/;
const ESLINT_TOTAL = /^✖ \d+ problems?/;

/** What ESLint found, all of it one deed. */
export function eslintChecks(lines: OutputLines): Found<ChecksDeed> | null {
  const items: Diagnostic[] = [];
  let at = -1;
  let file: string | null = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) continue;
    if (ESLINT_TOTAL.test(line)) {
      if (at < 0) at = index;
      file = null;
      continue;
    }
    const problem = ESLINT_PROBLEM.exec(line);
    if (problem && file) {
      if (at < 0) at = index;
      items.push({ level: problem[3] as Diagnostic["level"], code: problem[5] ?? null, message: problem[4]!, place: { path: file, line: Number(problem[1]), column: Number(problem[2]) }, snippet: [] });
      continue;
    }
    if (ESLINT_FILE.test(line.trim()) && !line.startsWith(" ")) file = line.trim();
  }
  if (at < 0 || !items.length) return null;
  return { at, deed: { kind: "checks", command: null, items, clean: false } };
}
