import { Sheet } from "../app/confirm";
import type { TextWords } from "./text-words";

/** Preview the immutable file bytes, rather than the input's later contents. */
export async function previewText(file: File, words: TextWords): Promise<void> {
  const text = await file.text();
  const sheet = new Sheet("device-sheet text-preview-sheet");
  sheet.body.innerHTML = `${sheet.head(file.name, words.close)}<textarea class="text-rescue" readonly></textarea>`;
  const input = sheet.body.querySelector<HTMLTextAreaElement>("textarea")!;
  input.setAttribute("aria-label", file.name);
  input.value = text;
  sheet.show();
}
