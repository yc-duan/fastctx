import type { Character } from "../../world/composer";
import { documentOffset, key, mapSelection, scalarOffset, textareaText, utf16Offset, wellFormed } from "./characters";
import type { InputHint, Selection } from "./characters";
import type { ComposerState, ComposerView } from "./document";
import { ComposerDocument } from "./document";
import { changes, completedInput } from "./input";

interface BrowserView {
  readonly characters: readonly Character[];
  readonly epoch: number;
  readonly hint: InputHint;
}

/** Binds only the textarea. The owner feeds snapshots into document.receive,
 *  reconnects with setConnected/retry, and gates sending on state.canSend.
 *  Unbinding commits any active composition and retains the document's journal. */
export function bindComposer(
  textarea: HTMLTextAreaElement,
  document: ComposerDocument,
  onState: (state: ComposerState) => void = () => {},
): () => void {
  let shown = document.view.characters;
  let epoch = document.view.state.epoch;
  let before: BrowserView | undefined;
  let composition: BrowserView | undefined;
  let committedComposition = false;
  let ending: ReturnType<typeof setTimeout> | undefined;
  let beforeTimer: ReturnType<typeof setTimeout> | undefined;
  let applying = false;
  let detached = false;
  let view = document.view;
  const originalReadOnly = textarea.readOnly;
  textarea.value = textareaText(shown);

  const selection = (value = textarea.value): Selection => ({
    start: scalarOffset(value, textarea.selectionStart, "start"),
    end: scalarOffset(value, textarea.selectionEnd, "end"),
    direction: textarea.selectionDirection,
  });
  const capture = (type: string): BrowserView => ({ characters: shown, epoch, hint: { ...selection(), type } });
  const documentSelection = (characters: readonly Character[], range = selection()): Selection => ({
    ...range, start: documentOffset(characters, range.start), end: documentOffset(characters, range.end),
  });

  const render = (next: ComposerView): void => {
    if (detached) return;
    view = next;
    try { onState(next.state); } catch (error) { console.error("a composer input observer failed", error); }
    if (composition || before || detached) return;
    const value = textareaText(next.characters);
    const changed = next.renamed.size || (shown !== next.characters && (shown.length !== next.characters.length
      || shown.some((character, index) => key(character.id) !== key(next.characters[index].id))));
    const range = (): Selection => mapSelection(shown, next.characters, documentSelection(shown), next.renamed);
    applying = true;
    if (textarea.value !== value) {
      const top = textarea.scrollTop;
      const left = textarea.scrollLeft;
      const mapped = range();
      textarea.value = value;
      textarea.setSelectionRange(utf16Offset(next.characters, mapped.start), utf16Offset(next.characters, mapped.end), mapped.direction);
      textarea.scrollTop = top;
      textarea.scrollLeft = left;
    } else if (changed) {
      const mapped = range();
      textarea.setSelectionRange(utf16Offset(next.characters, mapped.start), utf16Offset(next.characters, mapped.end), mapped.direction);
    }
    shown = next.characters;
    epoch = next.state.epoch;
    textarea.readOnly = originalReadOnly || !next.state.targetAvailable || next.state.sending || next.state.closing;
    applying = false;
  };

  const commit = (base: BrowserView, useHint: boolean): void => {
    const value = wellFormed(textarea.value);
    const deltas = changes(textareaText(base.characters), value, useHint ? base.hint : undefined);
    // Give the browser's resulting selection identities before a synchronous
    // document notification remaps it against a newer authoritative snapshot.
    const range = selection(value);
    if (deltas.length) {
      document.setComposing(true);
      const local: Character[] = [];
      let at = 0;
      for (const delta of deltas) {
        const start = documentOffset(base.characters, delta.start);
        const end = documentOffset(base.characters, delta.end);
        const put = document.edit(start, end, delta.insert, base.characters, base.epoch);
        for (const character of base.characters.slice(at, start)) local.push(character);
        for (const character of put ?? []) local.push(character);
        at = end;
      }
      for (const character of base.characters.slice(at)) local.push(character);
      const optimistic = document.view.characters;
      shown = local.length === optimistic.length && local.every((character, index) => key(character.id) === key(optimistic[index].id)) ? optimistic : local;
      epoch = base.epoch;
      const localRange = documentSelection(local, range);
      textarea.setSelectionRange(utf16Offset(local, localRange.start), utf16Offset(local, localRange.end), range.direction);
    } else {
      shown = base.characters;
      epoch = base.epoch;
    }
  };

  const finishComposition = (): void => {
    if (!composition) return;
    if (ending !== undefined) clearTimeout(ending);
    ending = undefined;
    const base = composition;
    // A target-change observer may unbind synchronously during this commit.
    // Clear composition first so that teardown cannot commit the same text twice.
    composition = undefined;
    before = base;
    commit(base, committedComposition);
    before = undefined;
    document.setComposing(false);
    render(document.view);
  };

  const compositionStart = (): void => {
    finishComposition();
    before = undefined;
    composition = capture("insertCompositionText");
    committedComposition = false;
    document.setComposing(true);
  };
  const compositionEnd = (event: CompositionEvent): void => {
    committedComposition = !!event.data;
    // Some engines emit the final input after compositionend. Flush at the next
    // task boundary, after those synchronous events, without touching the IME.
    ending = setTimeout(finishComposition, 0);
  };
  const beforeInput = (event: InputEvent): void => {
    if (applying || composition) return;
    before = capture(event.inputType);
    document.setComposing(true);
    const captured = before;
    if (beforeTimer !== undefined) clearTimeout(beforeTimer);
    beforeTimer = setTimeout(() => {
      beforeTimer = undefined;
      if (before !== captured || composition || detached) return;
      // Native dispatch can run microtasks between beforeinput and its default
      // action. Keep the hint through the whole task, including an input which
      // replaces equal text. Canceled events have no subsequent input.
      before = undefined;
      document.setComposing(false);
      render(view);
    }, 0);
  };
  const input = (event: Event): void => {
    if (applying || composition) return;
    const inferred = event instanceof InputEvent && !before ? completedInput(textareaText(shown), textarea.value, selection(), event.inputType, event.data) : undefined;
    const useHint = before !== undefined || inferred !== undefined;
    const base = before ?? { ...capture(""), ...(inferred ? { hint: inferred } : {}) };
    before = base;
    if (beforeTimer !== undefined) clearTimeout(beforeTimer);
    beforeTimer = undefined;
    if (event instanceof InputEvent && event.isComposing) {
      composition = base;
      document.setComposing(true);
      return;
    }
    commit(base, useHint);
    before = undefined;
    document.setComposing(false);
    render(document.view);
  };

  textarea.addEventListener("beforeinput", beforeInput);
  textarea.addEventListener("input", input);
  textarea.addEventListener("compositionstart", compositionStart);
  textarea.addEventListener("compositionend", compositionEnd);
  const unsubscribe = document.subscribe(render);
  return () => {
    if (detached) return;
    detached = true;
    unsubscribe();
    finishComposition();
    before = undefined;
    document.setComposing(false);
    if (beforeTimer !== undefined) clearTimeout(beforeTimer);
    textarea.removeEventListener("beforeinput", beforeInput);
    textarea.removeEventListener("input", input);
    textarea.removeEventListener("compositionstart", compositionStart);
    textarea.removeEventListener("compositionend", compositionEnd);
    textarea.readOnly = originalReadOnly;
  };
}
