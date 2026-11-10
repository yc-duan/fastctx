import { Sheet } from "../app/confirm";
import { escape } from "./prose";
import type { TextWords } from "./text-words";

/** A failed upload may release an outbox only after its words are recoverable. */
export async function attachText(
  text: string,
  words: TextWords,
  upload: (file: File) => Promise<void>,
  note: HTMLElement,
): Promise<void> {
  const say = (text: string) => showTextNotice(note, text);
  const file = new File([text], words.file, { type: "text/plain;charset=utf-8" });
  say(words.saving);
  try {
    await upload(file);
    say(words.attached);
  } catch {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // A user gesture can grant clipboard access that an automatic rescue
      // did not have. Closing leaves a way back to the full text in the notice.
      await copyText(text, words, say, note);
    }
    say(words.copied);
  }
}

/** A deleted identity could not be safely re-applied to a fresh node view. */
export async function copyRejectedText(text: string, words: TextWords, note: HTMLElement): Promise<void> {
  const say = (text: string) => showTextNotice(note, text);
  say(words.changedSaving);
  try { await navigator.clipboard.writeText(text); }
  catch { await copyText(text, { ...words, file: words.changedText, rescue: words.changedRescue }, say, note); }
  say(words.changedCopied);
}

function copyText(text: string, words: TextWords, say: (text: string) => void, note: HTMLElement): Promise<void> {
  const sheet = new Sheet("device-sheet confirm-sheet");
  sheet.body.innerHTML = `${sheet.head(words.file, words.close)}<p class="confirm-what">${escape(words.rescue)}</p>
    <textarea class="text-rescue" readonly aria-label="${escape(words.file)}"></textarea>
    <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.copy)}</button></footer>`;
  const input = sheet.body.querySelector<HTMLTextAreaElement>("textarea")!;
  input.value = text;
  sheet.show();
  input.select();
  const recovery = document.createElement("div");
  recovery.className = "composer-text-recovery";
  recovery.hidden = true;
  const explanation = document.createElement("span");
  explanation.textContent = words.rescue;
  const retry = document.createElement("button");
  retry.type = "button";
  retry.className = "text-button";
  retry.textContent = words.copy;
  retry.onclick = () => { recovery.hidden = true; sheet.show(); input.select(); };
  recovery.append(explanation, retry);
  note.after(recovery);
  return new Promise((resolve) => {
    let copied = false;
    const finish = () => {
      copied = true;
      recovery.remove();
      sheet.close();
      resolve();
    };
    sheet.dialog.addEventListener("close", () => {
      if (copied) return;
      say(words.rescue);
      recovery.hidden = false;
    });
    // Explicit copying also works when this WebView denies the asynchronous
    // clipboard API. Only a full selection preserves the outbox text; keep
    // it alive until the native copy completes.
    input.addEventListener("copy", (event) => {
      if (event.isTrusted && !event.defaultPrevented && input.selectionStart === 0 && input.selectionEnd === text.length) setTimeout(finish, 0);
    });
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      void navigator.clipboard.writeText(text).then(() => {
        finish();
      }, () => { input.focus(); input.select(); });
    };
  });
}

export function showTextNotice(note: HTMLElement, text: string): void {
  note.textContent = text;
  note.hidden = !text;
}
