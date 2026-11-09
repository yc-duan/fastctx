import type { AttachWords } from "./attach";
import { glyph } from "./glyphs";

/** One attachment keeps its card and image while its upload changes state. */
export interface AttachmentRow {
  key: string;
  name: string;
  image: boolean;
  preview: string | null;
  state: "waiting" | "sending" | "ready" | "failed";
  fact: string;
  why?: string | null;
  retry?: string | null;
  remove: string;
  progress?: number | null;
}

interface Card {
  element: HTMLLIElement;
  picture: HTMLElement;
  name: HTMLElement;
  fact: HTMLElement;
  retry: HTMLButtonElement;
  remove: HTMLButtonElement;
  progress: HTMLElement;
  preview: string | null;
  image: boolean;
}

/** Shared by the local input and node-owned drafts, including queued edits. */
export class AttachmentRows {
  private readonly cards = new Map<string, Card>();

  constructor(private readonly element: HTMLElement, private readonly words: AttachWords) {}

  draw(rows: readonly AttachmentRow[]): void {
    const kept = new Set(rows.map((row) => row.key));
    for (const [key, card] of this.cards) {
      if (kept.has(key)) continue;
      card.element.remove();
      this.cards.delete(key);
    }
    for (const row of rows) {
      let card = this.cards.get(row.key);
      if (!card) {
        card = this.create(row);
        this.cards.set(row.key, card);
        this.element.append(card.element);
      }
      this.update(card, row);
    }
    this.element.hidden = this.cards.size === 0;
  }

  private create(row: AttachmentRow): Card {
    const element = document.createElement("li");
    element.className = "attach-item fresh";
    element.dataset.item = row.key;
    element.addEventListener("animationend", (event) => {
      if (event.animationName === "attach-in") element.classList.remove("fresh");
    });
    const picture = document.createElement("span");
    const text = document.createElement("span");
    text.className = "attach-text";
    const name = document.createElement("span");
    name.className = "attach-name";
    const fact = document.createElement("span");
    fact.className = "attach-fact";
    text.append(name, fact);
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "attach-act";
    retry.textContent = this.words.retry;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "attach-remove";
    remove.title = this.words.remove;
    remove.setAttribute("aria-label", this.words.remove);
    remove.innerHTML = glyph("close", 14);
    const progress = document.createElement("span");
    progress.className = "attach-progress";
    element.append(picture, text, retry, remove, progress);
    return { element, picture, name, fact, retry, remove, progress, preview: null, image: !row.image };
  }

  private update(card: Card, row: AttachmentRow): void {
    for (const state of ["waiting", "sending", "ready", "failed"]) card.element.classList.toggle(state, row.state === state);
    if (card.preview !== row.preview || card.image !== row.image) {
      const picture = row.preview ? document.createElement("img") : document.createElement("span");
      picture.className = row.preview ? "attach-thumb" : "attach-thumb attach-glyph";
      if (picture instanceof HTMLImageElement) {
        picture.src = row.preview!;
        picture.alt = row.name;
      } else picture.innerHTML = glyph(row.image ? "image" : "file", 18);
      card.picture.replaceWith(picture);
      card.picture = picture;
      card.preview = row.preview;
      card.image = row.image;
    }
    card.name.textContent = row.name;
    card.name.title = row.name;
    card.fact.textContent = row.fact;
    card.fact.hidden = !row.fact;
    card.fact.classList.toggle("wrong", row.state === "failed");
    if (row.why) card.fact.title = row.why;
    else card.fact.removeAttribute("title");
    card.retry.hidden = !row.retry;
    if (row.retry) card.retry.dataset.retry = row.retry;
    else delete card.retry.dataset.retry;
    card.remove.dataset.remove = row.remove;
    card.progress.hidden = row.progress == null;
    if (row.progress != null) card.progress.style.width = `${Math.max(0, Math.min(100, row.progress))}%`;
  }
}
