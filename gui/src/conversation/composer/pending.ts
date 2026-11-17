import * as Y from "yjs";
import type { DocumentTextInput } from "../../world/composer-sync";
import { decode, encode } from "./bytes";

export const MAX_UPDATE_BYTES = 1024 * 1024;
export const MAX_PENDING_BYTES = 8 * 1024 * 1024;
const ENCODED_UPDATE_BYTES = Math.ceil(MAX_UPDATE_BYTES / 3) * 4;
export const WAITING_FOR_SYNC = "this input is waiting for its pending changes to synchronize";

export function pendingIsFull(inputs: readonly DocumentTextInput[]): boolean {
  return inputs.reduce((bytes, input) => bytes + input.update.length, 0) > MAX_PENDING_BYTES - ENCODED_UPDATE_BYTES;
}

/** A transmitted input is immutable. Unsent inputs can merge within one seal's
 * prefix, retaining the first number so late acknowledgements cannot name new data. */
export function compactPending(
  inputs: readonly DocumentTextInput[],
  transmitted: ReadonlySet<number>,
  through: readonly number[],
): DocumentTextInput[] {
  const result: DocumentTextInput[] = [];
  const segment = (number: number) => through.filter((end) => number > end).length;
  for (const input of inputs) {
    const previous = result.at(-1);
    if (previous && !transmitted.has(previous.number) && !transmitted.has(input.number)
      && segment(previous.number) === segment(input.number)) {
      const merged = Y.mergeUpdates([decode(previous.update), decode(input.update)]);
      if (merged.length <= MAX_UPDATE_BYTES) {
        result[result.length - 1] = { ...previous, update: encode(merged) };
        continue;
      }
    }
    result.push(input);
  }
  return result;
}
