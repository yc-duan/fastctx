import type { Card } from "./cards/cards";
import { cardsIn, type Bunch } from "./cards/bunch";
import type { Block, Entry } from "./model";

/** Keys belong to the held window, including rows derived from its entries.
 * Virtual scrolling within that window does not discard open state. */
export function retainedRows(entries: readonly Entry[], blocks: readonly Block[], cards: Map<string, Card>, bunches: Map<string, Bunch>): Set<string> {
  const own = new Set(entries.map((entry) => entry.key));
  const keys = new Set([...own, ...blocks.map((block) => block.key)]);
  for (const entry of entries) {
    if (entry.kind === "tool") for (const write of entry.tool.run?.writes ?? []) keys.add(`${entry.key}~${write.id}`);
    if (entry.kind === "prose" && entry.prose === "thinking") keys.add(`${entry.key}~words`);
  }
  for (const [key, card] of cards) {
    if (card.sources.every((source) => own.has(source.entry))) keys.add(key);
    else cards.delete(key);
  }
  for (const [key, bunch] of bunches) {
    if ([...cardsIn(bunch)].every((card) => card.sources.every((source) => own.has(source.entry)))) keys.add(key);
    else bunches.delete(key);
  }
  return keys;
}

export function retainKeys(state: Map<string, unknown> | Set<string>, keys: ReadonlySet<string>): void {
  for (const key of state.keys()) if (!keys.has(key)) state.delete(key);
}
