import type { Block, Entry } from "./model";
import { isThinking } from "./model";
import { matchingText } from "./find-text";
import type { TextMatch } from "./find-text";
import type { FindPart } from "./find-index";

/** Event searches omit aggregate headings but reuse the display's member parts. */
export function* eventParts(entries: readonly Entry[], blocks: readonly Block[], parts: (block: Block) => Iterable<FindPart>): Iterable<FindPart> {
  for (const entry of entries) {
    const own = blocks.find((block) => block.kind !== "activity-group" && block.key === entry.key);
    const block: Block = own ?? (entry.kind === "tool" || entry.kind === "activity" || isThinking(entry)
      ? { kind: "activity-group", key: entry.key, turn: "turn" in entry ? entry.turn : "", members: [entry], stamp: entry.stamp, agents: false }
      : entry);
    for (const part of parts(block)) if (!part.remote && !part.key.endsWith("/heading")) yield part;
  }
}

/** A very repetitive event remains cancellable without retaining all offsets. */
export async function searchText(text: string, query: string, signal: AbortSignal, ordinal?: number): Promise<number | TextMatch | null> {
  let count = 0; let began = performance.now();
  for (const match of matchingText(text, query)) {
    if (ordinal === count) return match;
    count += 1;
    if (count % 4096 === 0 && performance.now() - began > 8) {
      signal.throwIfAborted();
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve())); began = performance.now();
    }
  }
  signal.throwIfAborted();
  return ordinal === undefined ? count : null;
}
