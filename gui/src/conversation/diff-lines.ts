// A change's rows as lines a viewer keeps (`../longview`): each row's kind
// and numbers ride before its text, so the rows in view keep them while the
// rest are let go, and a long change is read a screen at a time from the
// machine that holds it. Searching looks in the rows' text only, and says
// where it found it in the line as the viewer holds it.

import hljs from "highlight.js/lib/common";
import type { LineChange, LineCursor, LineExtent, LineMatch, LinePaint, LineSource } from "../longview";
import type { DiffLine } from "../world/run-facts";
import { language, type DiffRow } from "./diff";

/** Between a row's fields; a row's text never holds it, since the writer
 *  shows control characters as `\xNN`. */
const SEP = "\u001f";
const KINDS = { add: "a", del: "d", same: "s", gap: "g" } as const;
const ROWS: Record<string, DiffRow["kind"]> = { a: "add", d: "del", s: "same", g: "gap" };

export function encodeRow(row: DiffRow): string {
  const hidden = row.kind === "gap" && row.hidden !== null && row.hidden !== undefined ? String(row.hidden) : "";
  return [KINDS[row.kind], row.old ?? "", row.new ?? "", hidden, row.text].join(SEP);
}

/** A row and where its text begins in its line. */
export function decodeRow(line: string): { row: DiffRow; at: number } {
  const fields: string[] = [];
  let at = 0;
  for (let field = 0; field < 4; field += 1) {
    const next = line.indexOf(SEP, at);
    fields.push(line.slice(at, next));
    at = next + 1;
  }
  const number = (field: string) => (field === "" ? null : Number(field));
  return {
    row: { kind: ROWS[fields[0]] ?? "same", old: number(fields[1]), new: number(fields[2]), hidden: number(fields[3]), text: line.slice(at) },
    at,
  };
}

/** A row as the writer recorded it; a hunk's row stands for the unchanged
 *  lines before it, counted from the row before it (`previous`), or from
 *  the top of the file where it is the first. */
export function rowOf(line: DiffLine, previous: DiffLine | null, first: boolean): DiffRow {
  switch (line.kind) {
    case "added":
      return { kind: "add", old: null, new: line.after, text: line.text };
    case "removed":
      return { kind: "del", old: line.before, new: null, text: line.text };
    case "context":
      return { kind: "same", old: line.before, new: line.after, text: line.text };
    case "hunk": {
      let hidden: number | null = null;
      if (first) hidden = line.before !== null ? line.before - 1 : null;
      else if (previous?.after != null && line.after !== null) hidden = line.after - previous.after - 1;
      else if (previous?.before != null && line.before !== null) hidden = line.before - previous.before - 1;
      return { kind: "gap", old: null, new: null, text: "", hidden };
    }
  }
}

/** Draws a row: its number, its sign, its text in its language's colours;
 *  a hunk as the unchanged lines it stands for. One column of numbers, each
 *  row's where it stands: a line taken out where it was, every other where
 *  it is now. The column widens to the largest number drawn in its holder;
 *  a change whose rows know no place in the file (`numbered` false) has
 *  none, wherever it is drawn. */
export function diffPaint(path: string, unchanged: (n: number) => string, numbered = true): LinePaint {
  const lang = language(path);
  return (element, line, _index, match) => {
    const { row, at } = decodeRow(line);
    if (!numbered) element.classList.add("unnumbered");
    if (row.kind === "gap") {
      element.classList.add("diff-gap");
      // The first hunk at the top of the file stands for nothing.
      element.textContent = row.hidden === 0 ? "" : row.hidden ? `⋯ ${unchanged(row.hidden)}` : "⋯";
      return;
    }
    element.classList.add("diff-row", `diff-${row.kind}`);
    const lineAt = row.kind === "del" ? row.old : (row.new ?? row.old);
    const widest = String(lineAt ?? "").length;
    const holder = element.parentElement;
    if (holder && widest > Number(holder.style.getPropertyValue("--diff-digits") || 0)) holder.style.setProperty("--diff-digits", String(widest));
    const cell = (className: string, text: string) => {
      const span = document.createElement("span");
      span.className = className;
      span.textContent = text;
      return span;
    };
    const code = document.createElement("code");
    code.className = "diff-code lv-text";
    const start = match ? match.start - at : -1;
    if (match && start >= 0) {
      const mark = document.createElement("mark");
      mark.textContent = row.text.slice(start, match.end - at);
      code.append(row.text.slice(0, start), mark, row.text.slice(match.end - at));
    } else if (lang && row.text) code.innerHTML = hljs.highlight(row.text, { language: lang, ignoreIllegals: true }).value;
    else code.textContent = row.text || " ";
    element.append(
      cell("diff-num", lineAt === null ? "" : String(lineAt)),
      cell("diff-sign", row.kind === "add" ? "+" : row.kind === "del" ? "−" : " "),
      code,
    );
  };
}

/** Where `query` is in a row's text, from `from` (an offset in the row's
 *  line) in `direction`; null where it is not. */
function within(line: string, query: string, from: number | undefined, direction: 1 | -1): LineMatch | null {
  const { row, at } = decodeRow(line);
  if (row.kind === "gap") return null;
  let found: number;
  if (direction === 1) found = row.text.indexOf(query, Math.max(0, (from ?? at) - at));
  else {
    const before = from === undefined ? row.text.length : from - at;
    if (before < 0) return null;
    found = row.text.lastIndexOf(query, before);
  }
  return found === -1 ? null : { line: 0, start: at + found, end: at + found + query.length };
}

/** A change's rows held here. */
export class DiffLines implements LineSource {
  constructor(private readonly lines: readonly string[]) {}

  get extent(): LineExtent {
    return { length: this.lines.length, complete: true };
  }

  async read(start: number, count: number, signal: AbortSignal): Promise<readonly string[]> {
    signal.throwIfAborted();
    return this.lines.slice(start, start + count);
  }

  async find(query: string, from: LineCursor, direction: 1 | -1, signal: AbortSignal): Promise<LineMatch | null> {
    for (let line = from.line; line >= 0 && line < this.lines.length; line += direction) {
      signal.throwIfAborted();
      const found = within(this.lines[line], query, line === from.line ? from.offset : undefined, direction);
      if (found) return { ...found, line };
    }
    return null;
  }

  subscribe(): () => void {
    return () => {};
  }
}

/** How a change's rows are read where they are kept: the raw records, one
 *  JSON object a line, as `RemoteLines` reads them. */
export interface RawLines extends LineSource {
  close(): Promise<void>;
}

/** Rows read from the machine holding the change, a screen at a time: each
 *  read takes the row before it too, so a hunk can say how many lines it
 *  skips. A first hunk at the very top stands for nothing and is not shown.
 *  A search reads the change in chunks and looks in the rows' text. */
export class RemoteDiff implements LineSource {
  private constructor(
    private readonly raw: RawLines,
    /** Records before the first row shown. */
    private readonly skip: number,
  ) {}

  static async open(raw: RawLines, signal: AbortSignal): Promise<RemoteDiff> {
    if (!raw.extent.length) return new RemoteDiff(raw, 0);
    const [first] = await raw.read(0, 1, signal);
    const record = JSON.parse(first!) as DiffLine;
    return new RemoteDiff(raw, record.kind === "hunk" && record.before === 1 ? 1 : 0);
  }

  get extent(): LineExtent {
    const { length, complete } = this.raw.extent;
    return { length: Math.max(0, length - this.skip), complete };
  }

  async read(start: number, count: number, signal: AbortSignal): Promise<readonly string[]> {
    const first = start + this.skip;
    const from = Math.max(0, first - 1);
    const records = (await this.raw.read(from, count + first - from, signal)).map((line) => JSON.parse(line) as DiffLine);
    const lines: string[] = [];
    for (let index = first - from; index < records.length; index += 1) {
      lines.push(encodeRow(rowOf(records[index]!, records[index - 1] ?? null, from + index === 0)));
    }
    return lines;
  }

  async find(query: string, from: LineCursor, direction: 1 | -1, signal: AbortSignal): Promise<LineMatch | null> {
    // A chunk is a request's worth of rows, read and searched here.
    const CHUNK = 2048;
    const length = this.extent.length;
    let line = from.line;
    let offset = from.offset;
    while (line >= 0 && line < length) {
      const start = direction === 1 ? line : Math.max(0, line - CHUNK + 1);
      const end = direction === 1 ? Math.min(length, line + CHUNK) : line + 1;
      const lines = await this.read(start, end - start, signal);
      for (let at = line; at >= start && at < end; at += direction) {
        const found = within(lines[at - start]!, query, at === line ? offset : undefined, direction);
        if (found) return { ...found, line: at };
      }
      line = direction === 1 ? end : start - 1;
      offset = undefined;
    }
    return null;
  }

  subscribe(changed: (change: LineChange) => void): () => void {
    return this.raw.subscribe(changed);
  }

  close(): Promise<void> {
    return this.raw.close();
  }
}

/** The rows a change keeps with the conversation, as lines a viewer
 *  holds: read as `RemoteDiff` reads them from the machine that keeps them,
 *  a first hunk at the very top left out. */
export function keptDiff(records: readonly DiffLine[]): string[] {
  const skip = records[0]?.kind === "hunk" && records[0].before === 1 ? 1 : 0;
  const lines: string[] = [];
  for (let index = skip; index < records.length; index += 1) {
    lines.push(encodeRow(rowOf(records[index]!, records[index - 1] ?? null, index === 0)));
  }
  return lines;
}

/** A change held here, as lines and the source that searches them. */
export function memoryDiff(rows: DiffRow[]): { lines: string[]; source: DiffLines } {
  const lines = rows.map(encodeRow);
  return { lines, source: new DiffLines(lines) };
}
