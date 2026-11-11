import * as Y from "yjs";
import { capture, nativeId as native } from "./capture";
import type { Character } from "../../world/composer";
import { key, wellFormed } from "./characters";
import { insertedCharacters, projectDelta } from "./projection";

const local = Symbol("local composer input");
const remote = Symbol("confirmed composer input");
const root = "text";

export interface ReplicaChange { readonly update: Uint8Array; readonly local: boolean }
export interface RelativeSelection {
  readonly start: Y.RelativePosition;
  readonly end: Y.RelativePosition;
  readonly direction: "forward" | "backward" | "none";
}

/** One plain-text Yjs replica. Sending freezes its bytes without changing the
 * live replica; only the node's committed deletion removes the captured text. */
export class ComposerReplica {
  private readonly doc = new Y.Doc();
  private readonly text = this.doc.getText(root);
  private readonly listeners = new Set<(change: ReplicaChange) => void>();
  private value: readonly Character[] = [];
  private contentCache: string | undefined;
  private disposed = false;

  constructor(readonly id: string, checkpoint?: Uint8Array) {
    if (checkpoint) Y.applyUpdate(this.doc, checkpoint, remote);
    this.value = insertedCharacters(this.text, 0, this.text.length);
    this.text.observe((event) => {
      this.value = projectDelta(this.text, this.value, event.delta);
      this.contentCache = undefined;
    });
    this.doc.on("update", (update: Uint8Array, origin: unknown) => {
      for (const listener of this.listeners) {
        try { listener({ update, local: origin === local }); }
        catch (error) { console.error("a shared document observer failed", error); }
      }
    });
  }

  get characters(): readonly Character[] { return this.value; }
  get content(): string { return this.contentCache ??= this.text.toString(); }
  vector(): Uint8Array { return Y.encodeStateVector(this.doc); }
  snapshot(): Uint8Array { return Y.encodeStateAsUpdate(this.doc); }
  capture(visible: readonly Character[]): { snapshot: Uint8Array; take: Uint8Array } { return capture(visible); }
  missing(vector?: Uint8Array): Uint8Array { return Y.encodeStateAsUpdate(this.doc, vector); }

  apply(update: Uint8Array): void {
    this.assertOpen();
    Y.applyUpdate(this.doc, update, remote);
  }

  subscribe(listener: (change: ReplicaChange) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  /** Start/end count scalars in the browser's actual view, possibly frozen during
   * IME composition. Remote insertions between those identities are not deleted. */
  edit(start: number, end: number, insert: string, base = this.value): readonly Character[] {
    this.assertOpen();
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start || end > base.length) {
      throw new Error("the edit is outside its document");
    }
    insert = wellFormed(insert);
    if (start === end && !insert) return [];
    const removed = new Set(base.slice(start, end).map((character) => key(character.id)));
    let at: number;
    if (start > 0) {
      const previous = base[start - 1];
      const identity = native(previous.id);
      identity.clock += previous.text.length - 1;
      at = this.position(Y.createRelativePositionFromJSON({ tname: root, item: identity, assoc: -1 }));
    } else if (base.length) {
      at = this.position(Y.createRelativePositionFromJSON({ tname: root, item: native(base[0].id), assoc: 0 }));
    } else at = 0;
    const ranges: { start: number; length: number }[] = [];
    let offset = 0;
    if (removed.size) for (const character of this.value) {
      if (removed.has(key(character.id))) {
        const previous = ranges.at(-1);
        if (previous && previous.start + previous.length === offset) previous.length += character.text.length;
        else ranges.push({ start: offset, length: character.text.length });
      }
      offset += character.text.length;
    }
    const removedBefore = ranges.reduce((total, range) => total + Math.min(range.length, Math.max(0, at - range.start)), 0);

    this.doc.transact(() => {
      for (const range of ranges.reverse()) this.text.delete(range.start, range.length);
      this.text.insert(at - removedBefore, insert);
    }, local);
    return insertedCharacters(this.text, at - removedBefore, insert.length);
  }

  /** Cursor positions use UTF-16, like textarea selectionStart/selectionEnd.
   * Endpoints are associated with their left characters, so a remote insertion
   * immediately after the cursor does not pull the cursor through that text. */
  selection(start: number, end: number, direction: RelativeSelection["direction"]): RelativeSelection {
    const content = this.content;
    if (start > end || !boundary(content, start) || !boundary(content, end)) throw new Error("the selection splits a character or is outside its document");
    return { start: Y.createRelativePositionFromTypeIndex(this.text, start, -1), end: Y.createRelativePositionFromTypeIndex(this.text, end, -1), direction };
  }

  resolve(selection: RelativeSelection): { start: number; end: number; direction: RelativeSelection["direction"] } {
    return { start: this.position(selection.start), end: this.position(selection.end), direction: selection.direction };
  }

  dispose(): void { this.disposed = true; this.listeners.clear(); this.doc.destroy(); }
  private assertOpen(): void { if (this.disposed) throw new Error("the shared document is disposed"); }
  private position(relative: Y.RelativePosition): number {
    const absolute = Y.createAbsolutePositionFromRelativePosition(relative, this.doc);
    if (!absolute || absolute.type !== this.text || !boundary(this.content, absolute.index)) throw new Error("the edited character is no longer in this document");
    return absolute.index;
  }
}



function boundary(text: string, at: number): boolean {
  if (!Number.isInteger(at) || at < 0 || at > text.length) return false;
  return at === 0 || at === text.length || text.charCodeAt(at - 1) < 0xd800 || text.charCodeAt(at - 1) > 0xdbff
    || text.charCodeAt(at) < 0xdc00 || text.charCodeAt(at) > 0xdfff;
}
