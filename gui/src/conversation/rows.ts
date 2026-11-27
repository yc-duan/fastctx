// What a conversation's page shows, as rows to compare: what each thing is
// and what it says of itself, never its words. The page checks itself with
// these (`self-check.ts`), and so does the bench that runs recordings of
// what pages were given (`scripts/conversation-bench`).

import { isThinking, runsOf, type Block, type Entry } from "./model";

export interface Row {
  /** The thing, by turn and item: a run of thinking by all its parts. */
  id: string;
  kind: string;
  turn: string;
  said: string;
}

function entryRow(one: Entry, turn: string): Row {
  // Words sent from the page keep the key they were drawn with once heard;
  // read anew they have their record's. Both carry how they were sent.
  const item = one.kind === "heard" && (one.delivery || one.messageId) ? `heard:${one.delivery || one.messageId}`
    : "item" in one && typeof one.item === "string" ? one.item
    : one.key;
  const own = "turn" in one && typeof one.turn === "string" && one.turn ? one.turn : turn;
  const said =
    one.kind === "prose" ? `${one.prose}${one.writing ? " writing" : ""}${one.prose === "thinking" ? ` took=${one.took ?? "?"}` : ""}`
    : one.kind === "tool" ? `tool ${one.ended ? one.ended.use : "running"}`
    : one.kind === "compaction" ? `compaction ${one.count ?? 1}`
    : one.kind;
  return { id: `${own}/${item}`, kind: one.kind, turn: own, said };
}

/** The rows `blocks` show, in order. A stretch's end is a row of its own,
 *  so where stretches split shows. */
export function rowsOf(blocks: readonly Block[]): Row[] {
  const rows: Row[] = [];
  for (const block of blocks) {
    if (block.kind === "activity-group") {
      for (const run of runsOf(block.members)) {
        if (isThinking(run[0]!)) {
          const thought = run as Extract<Entry, { kind: "prose" }>[];
          const writing = thought.some((one) => one.writing);
          const took = thought.every((one) => one.took !== undefined) ? thought.reduce((sum, one) => sum + (one.took ?? 0), 0) : null;
          rows.push({
            id: thought.map((one) => `${one.turn}/${one.item}`).join("+"),
            kind: "thought-row",
            turn: block.turn,
            said: writing ? "Thinking…" : took === null ? "Thought" : `Thought for ${Math.round(took / 1000)}s`,
          });
          continue;
        }
        for (const one of run) rows.push(entryRow(one, block.turn));
      }
      rows.push({ id: `group/${block.key}`, kind: "group-end", turn: block.turn, said: "" });
      continue;
    }
    if (block.kind === "gap") {
      rows.push({ id: block.key, kind: "gap", turn: "", said: "" });
      continue;
    }
    if (block.kind === "say") {
      rows.push({ id: `${block.turn}/${block.item}`, kind: "say", turn: block.turn, said: block.status });
      continue;
    }
    rows.push(entryRow(block as Entry, ""));
  }
  return rows;
}

/** Where two lists of rows first differ, stretches compared by where they
 *  end, not by the keys they keep; -1 where they do not. */
export function firstDifference(a: readonly Row[], b: readonly Row[]): number {
  const same = (x: Row, y: Row) => x.kind === y.kind && x.said === y.said && (x.kind === "group-end" || x.id === y.id);
  const length = Math.max(a.length, b.length);
  for (let at = 0; at < length; at += 1) {
    const x = a[at];
    const y = b[at];
    if (!x || !y || !same(x, y)) return at;
  }
  return -1;
}
