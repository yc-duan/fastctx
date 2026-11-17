// A timeline read from a file of its events, one a line, as the shell reads
// one from the copy a computer holds (`src-tauri/src/replica.rs`): for
// looking at a real conversation in a plain browser. A page is read as the
// shell reads one — at most so many events, begun where a stretch begins
// and ended where one ends — after a wait, as a read takes one; and the
// last events of the file can be held back, to come one after another as
// the records a timeline gains.

import type { Cut, Edge, Found, PartRead, PartSource } from "../conversation/conversation";
import { before, begins, member, type Place, type ReadFrom, type ReplicaPart, type ReplicaRecord, type Shown } from "../conversation/replica";

/** The most events one page holds (`fastctx_types::timeline_pages`). */
const PAGE_EVENTS = 400;

export class LocalTimeline implements PartSource {
  readonly generation = 1;
  /** How many of its events the timeline holds so far. */
  private held: number;

  constructor(private readonly events: Shown[], private readonly delay = 0, hold = 0) {
    this.held = Math.max(0, events.length - hold);
  }

  /** The part the page opens on. */
  latest(): ReplicaPart {
    return this.read({ from: "latest" });
  }

  /** The next event held back, as a record the timeline gained; none once
   *  all came. */
  next(): ReplicaRecord | null {
    const shown = this.events[this.held];
    if (!shown) return null;
    this.held += 1;
    return { record: "is", shown };
  }

  private at(place: Place): number {
    let low = 0;
    let high = this.held;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (before(this.events[middle]!.place, place)) low = middle + 1;
      else high = middle;
    }
    return low;
  }

  private read(from: ReadFrom): ReplicaPart {
    let start: number;
    let end: number;
    if (from.from === "latest") {
      end = this.held;
      start = Math.max(0, end - PAGE_EVENTS);
    } else if (from.from === "before") {
      end = this.at(from.place);
      start = Math.max(0, end - PAGE_EVENTS);
    } else {
      start = this.at(from.place);
      while (start < this.held && !before(from.place, this.events[start]!.place)) start += 1;
      end = Math.min(this.held, start + PAGE_EVENTS);
    }
    let read = this.events.slice(start, end);
    const more = { older: from.from === "after" ? read.length > 0 : start > 0, newer: from.from === "latest" ? false : from.from === "before" ? read.length > 0 : end < this.held };
    if (from.from !== "after" && more.older) {
      const at = read.findIndex((shown) => begins(shown.event));
      if (at > 0) read = read.slice(at);
    }
    if (from.from === "after" && more.newer) {
      let last = -1;
      for (let index = read.length - 1; index >= 0; index -= 1) if (begins(read[index]!.event)) { last = index; break; }
      if (last >= 0) {
        const first = read.slice(last + 1).findIndex((shown) => member(shown.event));
        if (first >= 0 && last + 1 + first > 0) read = read.slice(0, last + 1 + first);
      }
    }
    return {
      generation: this.generation,
      events: read,
      older: more.older ? read[0]?.place ?? null : null,
      newer: more.newer ? read[read.length - 1]?.place ?? null : null,
      preparing: false,
    };
  }

  async page(generation: number, from: ReadFrom): Promise<ReplicaPart | null> {
    if (this.delay > 0) await new Promise((resolve) => setTimeout(resolve, this.delay));
    return generation === this.generation ? this.read(from) : null;
  }

  async part(_generation: number, _edge: Edge): Promise<PartRead | null> { return null; }
  async locate(): Promise<number | null> { return null; }
  async cut(): Promise<Cut | null> { return null; }
  async index(): Promise<boolean> { return false; }
  async find(): Promise<Found | null> { return null; }
}

/** The events of a timeline kept one a line. */
export function timelineLines(text: string): Shown[] {
  return text.split("\n").filter(Boolean).map((line) => {
    const { key, place, event, deferred } = JSON.parse(line) as Shown;
    return deferred ? { key, place, event, deferred } : { key, place, event };
  });
}
