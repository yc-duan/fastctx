// Finding words in what the page does not hold (`world-gui.md`, 对话: 很长
// 的对话). The page reads each part it lacks once, folds it aside, hands the
// shell the displayed text of every block in it as finding reads it here,
// and lets the part go; the shell then counts the words in that text each
// time they change, leaving out what the page holds, which the page finds
// itself (`find-index.ts`). A change's lines, read from the machine that
// keeps them, are read only where the page holds the change: reading every
// change of a long conversation from its machine is not worth a search.

import type { Conversation, FindDoc } from "./conversation";
import type { Block } from "./model";
import type { FarHits, FindPart } from "./find-index";

export class FarFind {
  private indexing: Promise<void> | null = null;
  private stopped = new AbortController();
  /** Parts were not indexed yet at the last search: more may be found. */
  more = false;
  /** Moves each time more is indexed. */
  grown = 0;

  constructor(
    private readonly conversation: Conversation,
    private readonly parts: (block: Block) => Iterable<FindPart>,
    /** More was indexed: what was found may have grown. */
    private readonly grew: () => void,
  ) {}

  /** What the shell counts of `query` where the page holds nothing; reads
   *  in the background what it has no text of yet. */
  async search(query: string): Promise<FarHits[]> {
    const found = await this.conversation.findFar(lower(query));
    if (!found) {
      this.more = false;
      return [];
    }
    this.more = found.unindexed.length > 0;
    if (this.more && !this.indexing) {
      this.indexing = this.index(found.unindexed).finally(() => {
        this.indexing = null;
      });
    }
    return found.ranges.flatMap((range) => range.hits.map(([block, part, count, at]) => ({ block, part, count, at })));
  }

  private async index(ranges: [number, number][]): Promise<void> {
    const signal = this.stopped.signal;
    // The newest first: what is looked for is more often recent.
    for (const [start, end] of [...ranges].reverse()) {
      if (signal.aborted) return;
      const aside = await this.conversation.readAside(start, end);
      if (!aside) return;
      try {
        const docs: FindDoc[] = [];
        let began = performance.now();
        for (const block of aside.fold.blocks()) {
          const first = block.kind === "activity-group" ? block.members[0] : block.kind === "gap" ? undefined : block;
          const at = aside.start + (first?.at ?? 0);
          for (const part of this.parts(block)) {
            if (part.remote) continue;
            const read = part.read(signal);
            const text = typeof read === "string" ? read : await read;
            if (text) docs.push({ block: block.key, part: part.key, text: lower(text), at });
          }
          // Read a frame's worth at a time: the page goes on meanwhile.
          if (performance.now() - began > 8) {
            await frame();
            if (signal.aborted) return;
            began = performance.now();
          }
        }
        if (!(await this.conversation.indexFar(aside.start, aside.end, docs))) return;
      } finally {
        aside.release();
      }
      this.grown += 1;
      this.grew();
      await frame();
    }
  }

  /** Stops reading what the shell has no text of; what it has stays. */
  close(): void {
    this.stopped.abort();
    this.stopped = new AbortController();
  }
}

/** Text as finding compares it (`find-text.ts`). */
function lower(text: string): string {
  return text.toLowerCase().replace(/ς/g, "σ");
}

function frame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}
