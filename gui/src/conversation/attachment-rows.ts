import type { AttachWords } from "./attach";
import { glyph } from "./glyphs";
import { rememberSize } from "./picture-frame";

/** One attachment keeps its card and image while its upload changes state. */
export interface AttachmentRow {
  key: string;
  name: string;
  image: boolean;
  preview: string | null;
  /** Its bytes' digest, once the World has kept it: what the picture is
   *  known by after it is sent. */
  digest?: string | null;
  state: "waiting" | "sending" | "ready" | "failed";
  fact: string;
  open?: (() => void) | undefined;
  why?: string | null;
  retry?: string | null;
  remove: string;
  progress?: number | null;
}

interface Card {
  element: HTMLLIElement;
  picture: HTMLElement;
  name: HTMLElement;
  text: HTMLElement;
  fact: HTMLElement;
  retry: HTMLButtonElement;
  remove: HTMLButtonElement;
  progress: HTMLElement;
  preview: string | null;
  image: boolean;
  digest: string | null;
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
    const text = document.createElement(row.open ? "button" : "span");
    if (text instanceof HTMLButtonElement) text.type = "button";
    text.className = row.open ? "attach-text attach-open" : "attach-text";
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
    return { element, picture, name, text, fact, retry, remove, progress, preview: null, image: !row.image, digest: null };
  }

  private update(card: Card, row: AttachmentRow): void {
    for (const state of ["waiting", "sending", "ready", "failed"]) card.element.classList.toggle(state, row.state === state);
    if (card.preview !== row.preview || card.image !== row.image) {
      const picture = row.preview ? document.createElement("img") : document.createElement("span");
      picture.className = row.preview ? "attach-thumb" : "attach-thumb attach-glyph";
      if (picture instanceof HTMLImageElement) {
        picture.addEventListener("load", () => this.sized(card));
        picture.src = row.preview!;
        picture.alt = row.name;
      } else picture.innerHTML = glyph(row.image ? "image" : "file", 18);
      card.picture.replaceWith(picture);
      card.picture = picture;
      card.preview = row.preview;
      card.image = row.image;
    }
    if (!!row.open !== (card.text instanceof HTMLButtonElement)) {
      const next = document.createElement(row.open ? "button" : "span");
      if (next instanceof HTMLButtonElement) next.type = "button";
      next.className = row.open ? "attach-text attach-open" : "attach-text";
      next.append(card.name, card.fact);
      card.text.replaceWith(next);
      card.text = next;
    }
    card.text.onclick = row.open ?? null;
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
    if (card.digest !== (row.digest ?? null)) {
      card.digest = row.digest ?? null;
      this.sized(card);
    }
  }

  /** Keeps the size of a card's picture, by the copy the page holds and by
   *  the file's digest: once sent, its frame above the words is as large
   *  as the picture from the start (`picture-frame.ts`). */
  private sized(card: Card): void {
    const picture = card.picture;
    if (!(picture instanceof HTMLImageElement) || !picture.naturalWidth) return;
    rememberSize(card.preview, picture.naturalWidth, picture.naturalHeight);
    rememberSize(card.digest, picture.naturalWidth, picture.naturalHeight);
  }
}
