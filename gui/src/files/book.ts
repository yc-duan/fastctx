// A machine's activity book read as changes of files (`gui-files`): each
// deed is one script `run` ran there, with the files it touched; each file
// touched is one change, made by the deed's session.

import type { DiffRow } from "../conversation/diff";
import type { BookAnswer, Deed, Touched } from "../world/book";
import type { FileChange } from "./model";

/** The lines of one change, where its machine kept them. */
export type RowsOf = (machine: string, deed: Deed, touched: Touched) => (() => Promise<DiffRow[] | null>) | undefined;

/** The changes a book's answer tells of, from `from` on. */
export function bookChanges(answer: BookAnswer, from: number, rows?: RowsOf): FileChange[] {
  if (answer.kind !== "deeds") return [];
  const changes: FileChange[] = [];
  for (const deed of answer.deeds) {
    const at = Date.parse(deed.ended ?? deed.began);
    if (at < from) continue;
    for (const touched of deed.touched ?? []) {
      changes.push({
        id: `${answer.machine}\n${deed.job}\n${deed.began}\n${touched.path}`,
        at,
        machine: answer.machine,
        path: touched.path,
        how: touched.how,
        added: touched.added ?? 0,
        removed: touched.removed ?? 0,
        uncounted: touched.added === undefined && touched.removed === undefined,
        seen: !!touched.seen,
        session: deed.session ?? null,
        agent: deed.agent ?? null,
        from: deed.from ?? null,
        turn: null,
        // What was only seen changed has no lines of its own.
        rows: touched.seen ? undefined : rows?.(answer.machine, deed, touched),
      });
    }
  }
  return changes;
}
