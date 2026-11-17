import { difference, scalarOffset } from "./characters";

/** Bound per-character edit storage. Keep in step with
 * fastctx_types::composer::MAX_DOCUMENT_CHARACTERS. */
export const MAX_DOCUMENT_CHARACTERS = 30_000;

export function characterCount(value: string): number {
  let count = 0;
  for (const _ of value) count += 1;
  return count;
}

/** Runs before the editor and local draft listeners, including on engines
 * without beforeinput. Rejected replacements leave the selection intact. */
export function limitComposerInput(
  field: HTMLTextAreaElement,
  refused: (insert: string, typing: boolean) => void,
  current: () => string | undefined = () => undefined,
): () => void {
  let accepted = field.value;
  let start = field.selectionStart;
  let end = field.selectionEnd;
  let direction = field.selectionDirection;
  let captured = false;
  let composition: { value: string; start: number; end: number; direction: "forward" | "backward" | "none" } | undefined;
  let ending: ReturnType<typeof setTimeout> | undefined;
  const capture = () => {
    accepted = field.value;
    start = field.selectionStart;
    end = field.selectionEnd;
    direction = field.selectionDirection;
    captured = true;
  };
  const exceeds = (insert: string) => characterCount(field.value) - characterCount(field.value.slice(field.selectionStart, field.selectionEnd))
    + characterCount(insert.replace(/\r\n?/g, "\n")) > MAX_DOCUMENT_CHARACTERS;
  const paste = (event: ClipboardEvent) => {
    const insert = event.clipboardData?.getData("text/plain");
    if (!field.readOnly && insert && exceeds(insert)) {
      event.preventDefault();
      refused(insert, false);
    }
  };
  const before = (event: InputEvent) => {
    if (event.defaultPrevented || field.readOnly || composition) return;
    capture();
    if (event.isComposing) { compositionStart(); return; }
    const insert = event.data ?? (["insertLineBreak", "insertParagraph"].includes(event.inputType) ? "\n" : null);
    if (event.cancelable && insert && event.inputType.startsWith("insert") && exceeds(insert)) {
      event.preventDefault();
      captured = false;
      refused(insert, characterCount(insert) === 1 && !["insertFromPaste", "insertFromDrop"].includes(event.inputType));
    }
  };
  const input = (event?: Event) => {
    if (composition) return;
    if (event instanceof InputEvent && event.isComposing) {
      composition = { value: captured ? accepted : current() ?? accepted, start, end, direction };
      return;
    }
    const previous = captured ? accepted : current() ?? accepted;
    const value = field.value;
    if (characterCount(value) > MAX_DOCUMENT_CHARACTERS) {
      const hint = captured ? { start: scalarOffset(previous, start, "start"), end: scalarOffset(previous, end, "end"), direction, type: "insertText" } : undefined;
      const delta = difference(previous, value, hint);
      if (delta.insert) {
        field.value = previous;
        field.setSelectionRange(start, end, direction);
        captured = false;
        refused(delta.insert, characterCount(delta.insert) === 1);
        return;
      }
    }
    accepted = value;
    captured = false;
  };
  const compositionStart = () => {
    finishComposition();
    capture();
    composition = { value: accepted, start, end, direction };
    captured = false;
  };
  const finishComposition = () => {
    if (!composition) return;
    if (ending !== undefined) clearTimeout(ending);
    ending = undefined;
    ({ value: accepted, start, end, direction } = composition);
    composition = undefined;
    captured = true;
    const value = field.value;
    input();
    if (field.value !== value) field.dispatchEvent(new Event("input", { bubbles: true }));
  };
  // Let IME keep its temporary spelling, which may be longer than the final
  // scalar. Validate before bindComposer commits at the same task boundary.
  const compositionEnd = () => { ending = setTimeout(finishComposition, 0); };
  field.addEventListener("compositionstart", compositionStart, true);
  field.addEventListener("compositionend", compositionEnd, true);
  field.addEventListener("paste", paste, true);
  field.addEventListener("beforeinput", before, true);
  field.addEventListener("input", input, true);
  return () => {
    finishComposition();
    field.removeEventListener("compositionstart", compositionStart, true);
    field.removeEventListener("compositionend", compositionEnd, true);
    field.removeEventListener("paste", paste, true);
    field.removeEventListener("beforeinput", before, true);
    field.removeEventListener("input", input, true);
  };
}
