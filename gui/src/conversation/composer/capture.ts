import * as Y from "yjs";
import type { Character, CharacterId } from "../../world/composer";

export function nativeId(id: CharacterId): { client: number; clock: number } {
  const match = /^yjs:(\d+):(\d+)$/.exec(id.operation);
  if (!match || id.index !== 0) throw new Error("the character has no shared document identity");
  const client = Number(match[1]), clock = Number(match[2]);
  if (!Number.isSafeInteger(client) || !Number.isSafeInteger(clock) || client < 0 || clock < 0 || clock > 0xffff_ffff) throw new Error("the shared character identity is invalid");
  return { client, clock };
}

/** Fresh body plus a native delete set: encoding depends on captured characters,
 * rather than every client and tombstone retained in the shared document. */
export function capture(visible: readonly Character[]): { snapshot: Uint8Array; take: Uint8Array } {
  const clients = new Map<number, { start: number; end: number }[]>();
  for (const character of visible) {
    const { client, clock } = nativeId(character.id);
    const ranges = clients.get(client) ?? [];
    ranges.push({ start: clock, end: clock + character.text.length });
    clients.set(client, ranges);
  }
  const encoder = new Y.UpdateEncoderV1();
  encoder.writeLen(0); // No inserted structs in this update.
  encoder.writeLen(clients.size);
  for (const [client, ranges] of [...clients].sort(([a], [b]) => b - a)) {
    ranges.sort((a, b) => a.start - b.start);
    const merged: { start: number; end: number }[] = [];
    for (const range of ranges) {
      const last = merged.at(-1);
      if (last && range.start <= last.end) last.end = Math.max(last.end, range.end);
      else merged.push({ ...range });
    }
    encoder.resetDsCurVal();
    encoder.writeClient(client);
    encoder.writeLen(merged.length);
    for (const range of merged) { encoder.writeDsClock(range.start); encoder.writeDsLen(range.end - range.start); }
  }
  const frozen = new Y.Doc();
  try {
    frozen.getText("text").insert(0, visible.map((one) => one.text).join(""));
    return { snapshot: Y.encodeStateAsUpdate(frozen), take: encoder.toUint8Array() };
  } finally { frozen.destroy(); }
}
