// The button that copies a value, beside what it copies: the copy mark
// alone, its name said on hover and to a screen reader. Pressed, it turns
// to a tick for a moment, named for what it did.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";

const resets = new WeakMap<HTMLElement, number>();

/** A button copying `text`, named `label`; `extra` adds attributes. */
export function copyButton(text: string, label: string, extra = ""): string {
  return `<button class="icon-button copy-button" type="button" data-copy="${escape(text)}" data-name="${escape(label)}" aria-label="${escape(label)}" title="${escape(label)}"${extra ? ` ${extra}` : ""}>${glyph("copy", 16)}</button>`;
}

/** Copies what `button` holds, saying `copied` with a tick for a moment;
 *  where the clipboard is refused, nothing changes. */
export async function copyFrom(button: HTMLElement, copied: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(button.dataset.copy ?? "");
  } catch {
    return;
  }
  const name = (said: string, mark: "copy" | "check") => {
    button.setAttribute("aria-label", said);
    button.title = said;
    button.innerHTML = glyph(mark, 16);
    button.classList.toggle("copied", mark === "check");
  };
  name(copied, "check");
  clearTimeout(resets.get(button));
  resets.set(
    button,
    window.setTimeout(() => name(button.dataset.name ?? "", "copy"), 1400),
  );
}
