import { difference } from "./characters";
import type { InputHint, Selection } from "./characters";

export interface TextChange {
  readonly start: number;
  readonly end: number;
  readonly insert: string;
}

type Run = { same: readonly string[] } | { before: readonly string[]; after: readonly string[] };

/** When an engine supplies input without beforeinput, its committed data and
 *  final caret can still disambiguate repeated text. Verify the unchanged ends
 *  before using that inferred replacement. */
export function completedInput(before: string, after: string, range: Selection, type: string, data: string | null): InputHint | undefined {
  if (range.start !== range.end) return;
  const a = Array.from(before);
  const b = Array.from(after);
  const insertion = data ?? (["insertLineBreak", "insertParagraph"].includes(type) ? "\n" : null);
  const length = type.startsWith("delete") ? 0 : type.startsWith("insert") && insertion !== null ? Array.from(insertion.replace(/\r\n?/g, "\n")).length : undefined;
  if (length === undefined) return;
  const start = range.start - length;
  const end = start + a.length + length - b.length;
  if (start < 0 || end < start || end > a.length) return;
  if (a.slice(0, start).join("") !== b.slice(0, start).join("") || a.slice(end).join("") !== b.slice(start + length).join("")) return;
  return { start, end, direction: range.direction, type: length ? "insertText" : "deleteContent" };
}

/** A native selected replacement is intentional even when its text is equal.
 *  For other transformations, preserve unchanged runs: undo and drag can change
 *  several spans, and recreating the text between them would change its identity. */
export function changes(before: string, after: string, hint?: InputHint): TextChange[] {
  const span = difference(before, after, hint);
  if (hint && !["historyUndo", "historyRedo", "insertFromDrop", "insertTranspose", ""].includes(hint.type)) return span.start !== span.end || span.insert ? [span] : [];
  const a = Array.from(before);
  const b = Array.from(after);
  const runs = compare(a, b);
  const result: TextChange[] = [];
  let at = 0;
  for (const run of runs) {
    if ("same" in run) at += run.same.length;
    else {
      const previous = result.at(-1);
      if (previous?.end === at) result[result.length - 1] = { start: previous.start, end: at + run.before.length, insert: previous.insert + run.after.join("") };
      else result.push({ start: at, end: at + run.before.length, insert: run.after.join("") });
      at += run.before.length;
    }
  }
  return result;
}

function compare(a: readonly string[], b: readonly string[]): Run[] {
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head += 1;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail += 1;
  const start: Run[] = head ? [{ same: a.slice(0, head) }] : [];
  const end: Run[] = tail ? [{ same: a.slice(a.length - tail) }] : [];
  const left = a.slice(head, a.length - tail);
  const right = b.slice(head, b.length - tail);
  if (!left.length || !right.length) return [...start, ...(left.length || right.length ? [{ before: left, after: right }] : []), ...end];
  const alphabet = new Set(left);
  if (!right.some((scalar) => alphabet.has(scalar))) return [...start, { before: left, after: right }, ...end];
  const [x, y] = middle(left, right);
  if ((x === 0 && y === 0) || (x === left.length && y === right.length)) throw new Error("the text comparison did not advance");
  return [...start, ...compare(left.slice(0, x), right.slice(0, y)), ...compare(left.slice(x), right.slice(y)), ...end];
}

/** Myers' two-ended shortest-path search uses linear auxiliary space, avoiding
 *  a character-by-character matrix for large pasted text. */
function middle(a: readonly string[], b: readonly string[]): [number, number] {
  const distance = Math.ceil((a.length + b.length) / 2);
  const offset = distance + 1;
  const forward = new Int32Array(2 * distance + 3).fill(-1);
  const backward = new Int32Array(forward.length).fill(-1);
  forward[offset + 1] = backward[offset + 1] = 0;
  const delta = a.length - b.length;
  const odd = delta % 2 !== 0;
  for (let d = 0; d <= distance; d += 1) {
    for (let k = -d; k <= d; k += 2) {
      const index = offset + k;
      let x = k === -d || (k !== d && forward[index - 1] < forward[index + 1]) ? forward[index + 1] : forward[index - 1] + 1;
      let y = x - k;
      while (x < a.length && y < b.length && x >= 0 && y >= 0 && a[x] === b[y]) { x += 1; y += 1; }
      forward[index] = x;
      const reverse = offset + delta - k;
      if (odd && reverse >= 0 && reverse < backward.length && backward[reverse] !== -1 && x >= a.length - backward[reverse]) return [x, y];
    }
    for (let k = -d; k <= d; k += 2) {
      const index = offset + k;
      let x = k === -d || (k !== d && backward[index - 1] < backward[index + 1]) ? backward[index + 1] : backward[index - 1] + 1;
      let y = x - k;
      while (x < a.length && y < b.length && x >= 0 && y >= 0 && a[a.length - 1 - x] === b[b.length - 1 - y]) { x += 1; y += 1; }
      backward[index] = x;
      const direct = offset + delta - k;
      if (!odd && direct >= 0 && direct < forward.length && forward[direct] !== -1 && forward[direct] >= a.length - x) return [forward[direct], forward[direct] - (delta - k)];
    }
  }
  throw new Error("the text comparison found no path");
}
