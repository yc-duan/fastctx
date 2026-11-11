import * as Y from "yjs";
import type { Character } from "../../world/composer";

/** Allocates identities only for the inserted UTF-16 span. Retained characters
 * remain the same objects, including snapshots held by IME or a pending seal. */
export function insertedCharacters(text: Y.Text, start: number, length: number): Character[] {
  const result: Character[] = [];
  let offset = 0;
  const end = start + length;
  for (let item = text._start; item && offset < end; item = item.right) {
    if (item.deleted || !item.countable) continue;
    if (!(item.content instanceof Y.ContentString)) throw new Error("the shared document contains non-text content");
    const next = offset + item.length;
    if (next > start) {
      let clock = item.id.clock + Math.max(0, start - offset);
      const value = item.content.str.slice(Math.max(0, start - offset), Math.min(item.length, end - offset));
      for (const scalar of value) {
        result.push({ id: { operation: `yjs:${item.id.client}:${clock}`, index: 0 }, text: scalar }); clock += scalar.length;
      }
    }
    offset = next;
  }
  return result;
}

export function projectDelta(text: Y.Text, previous: readonly Character[], delta: Y.YTextEvent["delta"]): readonly Character[] {
  if (!delta.length) return previous;
  const next: Character[] = [];
  let scalar = 0, offset = 0;
  const consume = (units: number, retain: boolean) => {
    const begin = scalar;
    while (units > 0 && scalar < previous.length) units -= previous[scalar++].text.length;
    if (units !== 0) throw new Error("the shared update splits a text character");
    if (retain) for (let i = begin; i < scalar; i++) next.push(previous[i]);
  };
  for (const part of delta) {
    if (part.retain) { consume(part.retain, true); offset += part.retain; }
    if (part.delete) consume(part.delete, false);
    if (part.insert !== undefined) {
      if (typeof part.insert !== "string") throw new Error("the shared document contains non-text content");
      next.push(...insertedCharacters(text, offset, part.insert.length)); offset += part.insert.length;
    }
  }
  for (; scalar < previous.length; scalar++) next.push(previous[scalar]);
  return next;
}
