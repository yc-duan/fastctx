// The facts a World run leaves, for specimens: what it printed as the model
// got it, the files it wrote and their changes, held as the machines would
// hold them, so the interface reads them through the same seams.

import { MemoryLines } from "../longview";
import type { DiffLine, OutputSpan, RunFacts, WriteFact } from "../world/run-facts";

const RECORDS = "/home/me/.fastctx/world/towns/mark50/residents/claude-2b1c";

/** Lines the specimen machines hold, by path: changes' rows, outputs. */
const held = new Map<string, () => string[]>();

/** The lines at `path` on a specimen machine; an output still being written
 *  grows a line a second. */
export function specimenLines(path: string): MemoryLines & { close(): Promise<void> } {
  const lines = held.get(path)?.();
  if (!lines) throw { message: "the source does not exist", code: "source_missing" };
  const growing = path.endsWith("j-k3x9a2.log");
  const source = new MemoryLines(lines, !growing);
  let timer = 0;
  if (growing) {
    let n = 0;
    timer = window.setInterval(() => {
      n += 1;
      lines.push(`test joints::limits::case_${String(n).padStart(3, "0")} ... ok`);
      source.replace([...lines], false, lines.length - 1);
    }, 1000);
  }
  return Object.assign(source, { close: async () => clearInterval(timer) });
}

export const hunk = (before: number, after: number): DiffLine => ({ kind: "hunk", before, after, text: "", ending: "" });
export const same = (before: number, after: number, text: string): DiffLine => ({ kind: "context", before, after, text, ending: "\n" });
export const cut = (before: number, text: string): DiffLine => ({ kind: "removed", before, after: null, text, ending: "\n" });
export const put = (after: number, text: string): DiffLine => ({ kind: "added", before: null, after, text, ending: "\n" });

/** A file a run wrote, its change kept beside the run's record. */
export function wrote(operation: string, index: number, machine: string, path: string, kind: WriteFact["kind"], rows: DiffLine[]): WriteFact {
  const id = `${operation}-${index}`;
  const diff = `${RECORDS}/calls/${operation}.writes/${id}.jsonl`;
  held.set(diff, () => rows.map((row) => JSON.stringify(row)));
  return {
    id,
    machine,
    path,
    kind,
    added: rows.filter((row) => row.kind === "added").length,
    removed: rows.filter((row) => row.kind === "removed").length,
    diff: { machine, path: diff },
  };
}

/** What a write printed of itself, as the model read it. */
export function noted(command: "edit" | "write", operation: string, first: number, text: string): OutputSpan {
  return { note: { command, write: operation }, piece: { kind: "text", first_line: first, text } };
}

export function printed(first: number, text: string): OutputSpan {
  return { piece: { kind: "text", first_line: first, text } };
}

export function leftOut(first: number, last: number): OutputSpan {
  return { piece: { kind: "omitted", first_line: first, last_line: last } };
}

/** A run's facts: where it ran and how it ended, its output kept in a log
 *  of `lines` lines (made up here when asked for), what it wrote. */
export function ran(
  machine: string,
  job: string,
  state: RunFacts["state"],
  spans: OutputSpan[],
  writes: WriteFact[] = [],
  log?: { lines: number; cut: boolean; make: () => string[] },
): RunFacts {
  const path = `${RECORDS}/out/${job}.log`;
  if (log) held.set(path, log.make);
  return {
    machine,
    state,
    job,
    output: log ? { machine, path, bytes: log.lines * 60, lines: log.lines, cut: log.cut } : null,
    visited: [],
    writes,
    spans,
  };
}
