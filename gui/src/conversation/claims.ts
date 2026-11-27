// Which of what a page was shown as it happened its timeline's records stand
// for, where the two name the same thing differently: Claude's plugin names
// what it streams by turn, step and block, and a compaction by a name of its
// own, while the records read from the host's file name items by their row.

import type { SessionEvent } from "../world/events";

/** The item an event told as it happens is of, by turn and id. */
export function itemOf(event: SessionEvent): string | null {
  switch (event.event) {
    case "began":
    case "updated":
    case "finished":
      return `${event.turn}/${event.item.id}`;
    case "grew":
      return `${event.turn}/${event.item}`;
    default:
      return null;
  }
}

/** Whether `id` names a draft of `turn`, as Claude's plugin names what it
 *  streams (`[turn, step, block, kind]`), not as a record names an item. */
export function draftOf(turn: string, id: string): boolean {
  if (!id.startsWith(`[${JSON.stringify(turn)},`)) return false;
  try {
    const parts: unknown = JSON.parse(id);
    return Array.isArray(parts) && parts.length === 4;
  } catch {
    return false;
  }
}

/** When a compaction told by `event` happened, where it tells one ended. */
export function compactionAt(event: SessionEvent): number | null {
  if (event.event !== "finished" || event.item.kind !== "compaction") return null;
  const at = Date.parse(event.item.started_at ?? event.at);
  return Number.isFinite(at) ? at : null;
}

/** How long after a compaction shown as it happened began its record may
 *  say it happened: a compaction reads the whole conversation again. */
const COMPACTING_AT_MOST = 20 * 60_000;

/** How much earlier than a compaction shown as it happened its record may
 *  say it happened, the two clocks apart. */
const COMPACTED_BEFORE = 5_000;

/** How far apart the beginning of a draft of thinking, as it streamed, and
 *  its record's, as Claude Code measured it, may be and still be one. */
const DRAFT_BEGAN_WITHIN = 3_000;

/** When the compactions a timeline recorded happened. Each stands for the
 *  earliest compaction shown as it happened, before it and a while before
 *  at most, that none stands for yet. */
export class RecordedCompactions {
  private times: number[] = [];

  constructor(private readonly most: number) {}

  /** Notes the compaction `event` records, where it is one. */
  add(event: SessionEvent): void {
    const at = compactionAt(event);
    if (at === null || this.times.includes(at)) return;
    this.times.push(at);
    this.times.sort((a, b) => a - b);
    if (this.times.length > this.most) this.times.shift();
  }

  /** The compactions among `shown`, by turn and id, a recorded one stands for. */
  claimed(shown: readonly SessionEvent[]): Set<string> {
    const compactions = shown
      .flatMap((event) => {
        const at = compactionAt(event);
        const item = itemOf(event);
        return at === null || item === null ? [] : [{ at, item }];
      })
      .sort((a, b) => a.at - b.at);
    const claimed = new Set<string>();
    let next = 0;
    for (const one of compactions) {
      while (next < this.times.length && this.times[next]! < one.at - COMPACTED_BEFORE) next += 1;
      const recorded = this.times[next];
      if (recorded === undefined) break;
      if (recorded > one.at + COMPACTING_AT_MOST) continue;
      claimed.add(one.item);
      next += 1;
    }
    return claimed;
  }
}

/** A draft given up for the record that stands for it. */
export interface Claim {
  turn: string;
  draft: string;
  record: string;
}

/** Drafts shown as they streamed whose end never came, as when the watch
 *  broke as they ended, each matched to the record among `events` that
 *  stands for it: of the same turn and kind, whose thinking began when the
 *  draft's did, as Claude Code measured it, or whose words begin with the
 *  draft's. Only a draft its turn went on past is matched, one still the
 *  latest thing its turn began may still be streaming; and no record whose
 *  item another draft's end already `named`. */
export function orphansOf(
  ahead: readonly SessionEvent[],
  events: readonly SessionEvent[],
  recorded: (item: string) => boolean,
  named: (item: string) => boolean,
): Claim[] {
  const drafts = new Map<string, { turn: string; id: string; kind: string; began: number; text: string }>();
  for (const event of ahead) {
    if (event.event !== "began" && event.event !== "grew") continue;
    const id = event.event === "began" ? event.item.id : event.item;
    if (!draftOf(event.turn, id)) continue;
    const key = `${event.turn}/${id}`;
    if (recorded(key)) continue;
    const held = drafts.get(key);
    if (event.event === "began") {
      if (event.item.kind !== "thinking" && event.item.kind !== "message") continue;
      const began = Date.parse(event.item.started_at ?? event.at);
      const text = "text" in event.item && typeof event.item.text === "string" ? event.item.text : "";
      drafts.set(key, { turn: event.turn, id, kind: event.item.kind, began, text: (held?.text ?? "") + text });
    } else if (held && event.part === "text") held.text += event.text;
  }
  if (!drafts.size) return [];
  const latest = new Map<string, number>();
  for (const event of [...events, ...ahead]) {
    if (event.event !== "began" && event.event !== "finished" && event.event !== "updated") continue;
    if (draftOf(event.turn, event.item.id)) continue;
    const at = Date.parse(event.item.started_at ?? event.at);
    if (Number.isFinite(at) && at > (latest.get(event.turn) ?? -Infinity)) latest.set(event.turn, at);
  }
  // The records a draft may stand for: ended items of its kind.
  const records = events.flatMap((event) => {
    if (event.event !== "finished" || draftOf(event.turn, event.item.id) || named(`${event.turn}/${event.item.id}`)) return [];
    if (event.item.kind === "thinking") {
      const began = Date.parse(event.at) - (event.item.duration_ms ?? 0);
      return [{ turn: event.turn, id: event.item.id, kind: "thinking", began, text: event.item.text }];
    }
    if (event.item.kind === "message") return [{ turn: event.turn, id: event.item.id, kind: "message", began: Date.parse(event.at), text: event.item.text }];
    return [];
  });
  const taken = new Set<string>();
  const claims: Claim[] = [];
  for (const draft of [...drafts.values()].sort((a, b) => a.began - b.began)) {
    if (!((latest.get(draft.turn) ?? -Infinity) > draft.began + 1_000)) continue;
    const fits = records.filter((record) => {
      if (record.turn !== draft.turn || record.kind !== draft.kind || taken.has(record.id)) return false;
      if (draft.text.trim()) return record.text.startsWith(draft.text);
      return draft.kind === "thinking" ? Math.abs(record.began - draft.began) <= DRAFT_BEGAN_WITHIN : record.began >= draft.began - 1_000;
    });
    const record = fits.sort((a, b) => Math.abs(a.began - draft.began) - Math.abs(b.began - draft.began))[0];
    if (!record) continue;
    taken.add(record.id);
    claims.push({ turn: draft.turn, draft: draft.id, record: record.id });
  }
  return claims;
}
