// A change as it reads: a diff laid out a line to a row, the old and new
// line numbers beside it where they are known, whole rows tinted for what
// was added or removed, and between hunks how many lines were left alone.
//
// Diffs come in three shapes. Codex reports unified diffs with numbered
// hunks. The World's `apply_patch` keeps the patch's own hunks, whose `@@`
// carries context words, not numbers. The World's `edit` and `write` give
// the old text as removed lines and the new as added ones, with no place in
// the file; a removed block followed by an added one is shown as the lines
// that differ, the rest as context, since that is what the change did.

import hljs from "highlight.js/lib/common";

export interface DiffRow {
  kind: "add" | "del" | "same" | "gap";
  old: number | null;
  new: number | null;
  text: string;
  /** For a gap: how many unchanged lines it stands for, when known. */
  hidden?: number | null;
}

export function parse(diff: string): DiffRow[] {
  const lines = diff.replace(/\r\n/g, "\n").split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  if (!lines.some((line) => line.startsWith("@@"))) return unplaced(lines);
  const rows: DiffRow[] = [];
  let old: number | null = null;
  let next: number | null = null;
  let lastOldEnd: number | null = null;
  for (const line of lines) {
    if (line.startsWith("@@")) {
      const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
      if (hunk) {
        const oldStart = Number(hunk[1]);
        const gap = lastOldEnd === null ? oldStart - 1 : oldStart - lastOldEnd;
        if (gap > 0) rows.push({ kind: "gap", old: null, new: null, text: "", hidden: gap });
        old = oldStart;
        next = Number(hunk[2]);
      } else {
        // A patch's own hunk: somewhere further on, how far is not said.
        if (rows.length) rows.push({ kind: "gap", old: null, new: null, text: "", hidden: null });
        old = null;
        next = null;
      }
      continue;
    }
    if (/^(---|\+\+\+|diff |index |\\)/.test(line)) continue;
    const mark = line[0];
    const text = line.slice(1);
    if (mark === "+") rows.push({ kind: "add", old: null, new: next === null ? null : next++, text });
    else if (mark === "-") rows.push({ kind: "del", old: old === null ? null : old++, new: null, text });
    else if (mark === " " || line === "") {
      rows.push({ kind: "same", old: old === null ? null : old++, new: next === null ? null : next++, text });
    }
    lastOldEnd = old;
  }
  return rows;
}

/** Lines added and removed, as the rows show them. */
export function counts(diff: string): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const row of parse(diff)) {
    if (row.kind === "add") added += 1;
    else if (row.kind === "del") removed += 1;
  }
  return { added, removed };
}

/** Lines with no place in the file: each run of removed lines followed by
 *  added ones becomes the lines that differ between them. */
function unplaced(lines: string[]): DiffRow[] {
  const rows: DiffRow[] = [];
  let index = 0;
  while (index < lines.length) {
    const removed: string[] = [];
    const added: string[] = [];
    while (index < lines.length && lines[index].startsWith("-")) removed.push(lines[index++].slice(1));
    while (index < lines.length && lines[index].startsWith("+")) added.push(lines[index++].slice(1));
    if (!removed.length && !added.length) {
      rows.push({ kind: "same", old: null, new: null, text: lines[index++].slice(1) });
      continue;
    }
    // One at a time: a run of a hundred thousand lines is more than a
    // call's arguments can carry.
    for (const row of between(removed, added)) rows.push(row);
  }
  return rows;
}

/** The rows that turn `before` into `after`, by their longest common run of
 *  lines. A change too large to compare line against line in a moment is
 *  shown as it came, all removed and then all added. */
function between(before: string[], after: string[]): DiffRow[] {
  const row = (kind: "add" | "del" | "same", text: string): DiffRow => ({ kind, old: null, new: null, text });
  // Lines alike at both ends need no table.
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start += 1;
  let end = 0;
  while (
    end < before.length - start &&
    end < after.length - start &&
    before[before.length - 1 - end] === after[after.length - 1 - end]
  ) {
    end += 1;
  }
  const head = before.slice(0, start).map((text) => row("same", text));
  const tail = before.slice(before.length - end).map((text) => row("same", text));
  const a = before.slice(start, before.length - end);
  const b = after.slice(start, after.length - end);
  // The table is a×b cells; past about four million the comparison costs
  // more than the reader gains from it.
  if (a.length * b.length > 4_000_000) {
    return [...head, ...a.map((text) => row("del", text)), ...b.map((text) => row("add", text)), ...tail];
  }
  const width = b.length + 1;
  const table = new Uint32Array((a.length + 1) * width);
  for (let i = a.length - 1; i >= 0; i -= 1) {
    for (let j = b.length - 1; j >= 0; j -= 1) {
      table[i * width + j] =
        a[i] === b[j] ? table[(i + 1) * width + j + 1] + 1 : Math.max(table[(i + 1) * width + j], table[i * width + j + 1]);
    }
  }
  const middle: DiffRow[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      middle.push(row("same", a[i]));
      i += 1;
      j += 1;
    } else if (j < b.length && (i === a.length || table[i * width + j + 1] >= table[(i + 1) * width + j])) {
      middle.push(row("add", b[j]));
      j += 1;
    } else {
      middle.push(row("del", a[i]));
      i += 1;
    }
  }
  // Within a run of changes, removed lines read before added ones.
  const ordered: DiffRow[] = [];
  let run: DiffRow[] = [];
  const flush = () => {
    for (const r of run) if (r.kind === "del") ordered.push(r);
    for (const r of run) if (r.kind === "add") ordered.push(r);
    run = [];
  };
  for (const r of middle) {
    if (r.kind === "same") {
      flush();
      ordered.push(r);
    } else {
      run.push(r);
    }
  }
  flush();
  return [...head, ...ordered, ...tail];
}

const languages: Record<string, string> = {
  rs: "rust", ts: "typescript", tsx: "typescript", js: "javascript", mjs: "javascript", cjs: "javascript", py: "python",
  go: "go", java: "java", kt: "kotlin", c: "c", h: "c", cpp: "cpp", cc: "cpp", hpp: "cpp", cs: "csharp", json: "json",
  toml: "ini", ini: "ini", yaml: "yaml", yml: "yaml", sh: "bash", bash: "bash", ps1: "powershell", css: "css",
  html: "xml", xml: "xml", svg: "xml", sql: "sql", swift: "swift", md: "markdown", rb: "ruby", php: "php", lua: "lua",
};

export function language(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  const name = languages[ext];
  return name && hljs.getLanguage(name) ? name : null;
}

/** Whether any row knows its place in the file. */
export function numbered(rows: DiffRow[]): boolean {
  return rows.some((row) => row.old !== null || row.new !== null);
}
