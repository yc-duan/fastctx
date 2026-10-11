import type { Character, CharacterId, ComposerEdit } from "../../world/composer";

export function key(id: CharacterId): string {
  return JSON.stringify([id.operation, id.index]);
}

export function text(characters: readonly Character[]): string {
  return characters.map((character) => character.text).join("");
}

/** Lone UTF-16 surrogates have no Unicode scalar or Rust string representation. */
export function wellFormed(value: string): string {
  return Array.from(value, (character) => {
    const code = character.charCodeAt(0);
    return character.length === 1 && code >= 0xd800 && code <= 0xdfff ? "\ufffd" : character;
  }).join("");
}

export function inserted(edit: ComposerEdit): Character[] {
  return Array.from(edit.insert, (text, index) => ({ id: { operation: edit.operation, index }, text }));
}

export interface Context {
  readonly id: CharacterId;
  readonly previous: Context | null;
}

const contexts = new WeakMap<Character, Context>();

/** Share unchanged prefixes between keystrokes. Keeping a copied prefix per
 *  pending edit would retain quadratic memory while typing offline. */
export function context(characters: readonly Character[], end = characters.length): Context | null {
  let previous: Context | null = null;
  for (let index = 0; index < end; index += 1) {
    const character = characters[index];
    let node = contexts.get(character);
    if (!node || node.previous !== previous) {
      node = { id: character.id, previous };
      contexts.set(character, node);
    }
    previous = node;
  }
  return previous;
}

/** Visible snapshots omit tombstones. An absent anchor uses its last visible neighbours
 *  for the provisional view; the original anchor still goes to the authority unchanged. */
export function position(characters: readonly Character[], after: CharacterId | null, context: Context | null): number {
  if (after === null) return 0;
  const indices = new Map(characters.map((character, index) => [key(character.id), index]));
  const exact = indices.get(key(after));
  if (exact !== undefined) return exact + 1;
  for (let node = context; node; node = node.previous) {
    const index = indices.get(key(node.id));
    if (index !== undefined) return index + 1;
  }
  return 0;
}

export function overlay(characters: readonly Character[], edit: ComposerEdit, context: Context | null): Character[] {
  const at = position(characters, edit.after, context);
  const deleted = new Set(edit.delete.map(key));
  return [
    ...characters.slice(0, at).filter((character) => !deleted.has(key(character.id))),
    ...inserted(edit),
    ...characters.slice(at).filter((character) => !deleted.has(key(character.id))),
  ];
}

/** Textarea offsets are UTF-16; wire offsets are Unicode scalars. A split surrogate
 *  selection expands to include the scalar, rather than producing invalid JSON text. */
export function scalarOffset(value: string, offset: number, edge: "start" | "end"): number {
  let units = 0;
  let scalars = 0;
  for (const character of value) {
    if (units >= offset) break;
    units += character.length;
    if (units > offset && edge === "start") break;
    scalars += 1;
  }
  return scalars;
}

export function utf16Offset(characters: readonly Character[], offset: number): number {
  return textareaText(characters.slice(0, offset)).length;
}

/** Textareas display CR and CRLF as LF. Keep the underlying scalar identities
 *  intact; deleting a displayed CRLF deletes both of its wire characters. */
export function textareaText(characters: readonly Character[]): string {
  return text(characters).replace(/\r\n?/g, "\n");
}

export function documentOffset(characters: readonly Character[], displayedScalars: number): number {
  let index = 0;
  for (let count = 0; count < displayedScalars && index < characters.length; count += 1) {
    const cr = characters[index].text === "\r";
    index += 1;
    if (cr && characters[index]?.text === "\n") index += 1;
  }
  return index;
}

export interface Selection {
  readonly start: number;
  readonly end: number;
  readonly direction: "forward" | "backward" | "none";
}

/** A caret follows its left identity. A range stays inside its two boundary
 *  identities, preserving direction and excluding insertions outside the selection. */
export function mapSelection(
  before: readonly Character[], after: readonly Character[], selection: Selection,
  renamed: ReadonlyMap<string, CharacterId> = new Map(),
): Selection {
  const indices = new Map(after.map((character, index) => [key(character.id), index]));
  const find = (index: number): number | undefined => {
    const id = before[index]?.id;
    return id ? indices.get(key(renamed.get(key(id)) ?? id)) : undefined;
  };
  const boundary = (offset: number, affinity: "left" | "right"): number => {
    for (let distance = 0; distance < before.length; distance += 1) {
      const left = find(offset - 1 - distance);
      const right = find(offset + distance);
      if (affinity === "left") {
        if (left !== undefined) return left + 1;
        if (right !== undefined) return right;
      } else {
        if (right !== undefined) return right;
        if (left !== undefined) return left + 1;
      }
    }
    return 0;
  };
  const collapsed = selection.start === selection.end;
  const start = boundary(selection.start, collapsed && selection.start > 0 ? "left" : "right");
  const end = collapsed ? start : boundary(selection.end, "left");
  return { start: Math.min(start, end), end: Math.max(start, end), direction: selection.direction };
}

export interface InputHint extends Selection {
  readonly type: string;
}

/** Prefer the browser's selected replacement to an ambiguous textual diff ("aaa"
 *  has several indistinguishable deletions). Inspect the actual value for IME,
 *  spellchecking, undo, and engines which supply no beforeinput. */
export function difference(before: string, after: string, hint?: InputHint): { start: number; end: number; insert: string } {
  const a = Array.from(before);
  const b = Array.from(after);
  const replacement = (start: number, end: number): { start: number; end: number; insert: string } | undefined => {
    const length = b.length - (a.length - (end - start));
    if (length < 0 || start < 0 || end > a.length) return undefined;
    if (a.slice(0, start).join("") !== b.slice(0, start).join("")) return undefined;
    if (a.slice(end).join("") !== b.slice(start + length).join("")) return undefined;
    return { start, end, insert: b.slice(start, start + length).join("") };
  };
  if (hint) {
    if (["insertText", "insertFromPaste", "insertFromYank", "insertLineBreak", "insertParagraph", "deleteByCut", "deleteContent"].includes(hint.type)
      || (hint.type.startsWith("delete") && hint.start !== hint.end)) {
      const span = replacement(hint.start, hint.end);
      if (span) return span;
    }
    if (hint.start === hint.end && a.length > b.length) {
      const count = a.length - b.length;
      const span = hint.type.endsWith("Backward") ? replacement(hint.start - count, hint.start)
        : hint.type.endsWith("Forward") ? replacement(hint.start, hint.start + count) : undefined;
      if (span) return span;
    }
  }
  let start = 0;
  const limit = hint ? hint.start : a.length;
  while (start < limit && start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let end = a.length;
  let nextEnd = b.length;
  const minimum = hint ? hint.end : start;
  while (end > Math.max(start, minimum) && nextEnd > start && a[end - 1] === b[nextEnd - 1]) {
    end -= 1;
    nextEnd -= 1;
  }
  return { start, end, insert: b.slice(start, nextEnd).join("") };
}
