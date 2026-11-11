import type { ComposerJournal } from "./document";
import type { Address } from "../../world/wire";

const closing = new Map<string, Set<Promise<void>>>();
export function rememberClosing(key: string, work: Promise<void>): void {
  const [machine, resident] = JSON.parse(key) as [string | null, string];
  const conversation = JSON.stringify([machine, resident]);
  const pending = closing.get(conversation) ?? new Set<Promise<void>>();
  pending.add(work); closing.set(conversation, pending);
  void work.finally(() => { pending.delete(work); if (!pending.size) closing.delete(conversation); }).catch(() => {});
}
/** Navigation in one application must finish handing off its old writer before
 * a new page chooses a recovery key. Other live windows still use Web Locks. */
export async function finishClosing(who: Address): Promise<void> {
  const pending = closing.get(JSON.stringify([who.machine, who.resident]));
  if (pending) await Promise.allSettled(pending);
}

// A failed IndexedDB write cannot survive power loss. Keep the last writer's
// unsaved projection through page navigation without keeping its session alive.
const retained = new Map<string, ReadonlyMap<string, ComposerJournal | null>>();
export function rememberRecovery(key: string, journals: ReadonlyMap<string, ComposerJournal | null>): void {
  retained.set(key, new Map(journals));
}
export function mergeRecovery(key: string, saved: ReadonlyMap<string, ComposerJournal>): ReadonlyMap<string, ComposerJournal> {
  const merged = new Map(saved);
  for (const [name, journal] of retained.get(key) ?? []) {
    if (journal) merged.set(name, journal); else merged.delete(name);
  }
  return merged;
}
export function clearRecovery(key: string): void { retained.delete(key); }
