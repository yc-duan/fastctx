// What changed in the World between two pieces of its watch, as the shell
// works it out (`gui/src-tauri/src/world_delta.rs`), put into the World the
// page holds. What did not change keeps the very objects it was, so what is
// drawn from it is drawn again only where something changed.

/** The value now; an object's changed fields and those gone; a list's
 *  entries changed, added and gone by the field that names them, and its
 *  order when that is not the old one with the added after it. */
export type Change =
  | { v: unknown }
  | { o?: Record<string, Change>; x?: string[] }
  | { l: { k: string; c?: Record<string, Change>; a?: unknown[]; x?: string[]; order?: string[] } };

export function applyChange(old: unknown, change: Change): unknown {
  if ("v" in change) return change.v;
  if ("l" in change) {
    const { k, c = {}, a = [], x = [], order } = change.l;
    const name = (entry: unknown) => String((entry as Record<string, unknown>)[k]);
    const gone = new Set(x);
    const kept = (Array.isArray(old) ? old : [])
      .filter((entry) => !gone.has(name(entry)))
      .map((entry) => {
        const changed = c[name(entry)];
        return changed ? applyChange(entry, changed) : entry;
      });
    const all = [...kept, ...a];
    if (!order) return all;
    const byName = new Map(all.map((entry) => [name(entry), entry]));
    return order.map((one) => byName.get(one));
  }
  const copy: Record<string, unknown> = { ...(old as Record<string, unknown>) };
  for (const [field, inner] of Object.entries(change.o ?? {})) copy[field] = applyChange(copy[field], inner);
  for (const field of change.x ?? []) delete copy[field];
  return copy;
}
