// Words said while the agent is working. They do not cut into its turn:
// they wait above the composer, in order, and the conversation sends the
// first of them once a turn has ended as it should, then the next once the
// turn that one began has ended, never two at once and never merged. A turn that was
// stopped or failed holds them: they are kept, not sent, until the user
// sends one. Each can be sent at once into the turn under way, changed,
// taken out, or moved. They live while the app runs; leaving the
// conversation and coming back finds them, a reload does not.

import type { Attached } from "./attach";
import { glyph } from "./glyphs";
import { escape } from "./prose";
import { composing } from "../typing";

export interface QueueWords {
  /** Above the queue: how many wait, and when they go. */
  waiting(count: number): string;
  /** Above the queue when the last turn was stopped or failed. */
  held: string;
  /** Above the queue when the last of it the World did not take. */
  notTaken: string;
  /** Above the queue when the last of it went where its turn's end will not
   *  be seen. */
  unsure: string;
  sendNow: string;
  sendNowTitle: string;
  edit: string;
  save: string;
  cancel: string;
  remove: string;
  reorder: string;
  files(count: number): string;
  empty: string;
  /** The composer's button when words would wait. */
  queue: string;
  /** Above the queue when the node holds it; its reason is the hover. */
  paused: string;
  /** Closing a queued one being changed: its changes are already kept. */
  done: string;
  /** Beside one the node is sending now. */
  sending: string;
  /** Why one cannot be sent at once while another is on its way. */
  stillGoing: string;
  /** Beside one another device is changing. */
  editingElsewhere: string;
}

/** Why the queue waits on the user: the last turn was stopped or failed;
 *  the last of it was not taken; or it went where the end of its turn will
 *  not be seen. */
export type Held = "stopped" | "not_taken" | "unsure";

export interface Queued {
  key: string;
  text: string;
  /** What arrived of what was attached; for an agent assisting the user,
   *  the machine its files other than images go to. */
  attached: { refs: unknown[]; shown: Attached[]; filesOn?: string | null };
}

export interface QueueActions {
  /** Sends one at once, into the turn under way. */
  now(item: Queued): void;
  /** What is queued changed: the composer redraws what depends on it. */
  changed(): void;
}

/** What waits in each conversation, by its key, while the app runs. */
const kept = new Map<string, Queued[]>();
let serial = 0;

export class SendQueue {
  readonly element: HTMLElement;
  private editing: string | null = null;
  /** Why it waits on the user, when it does. */
  private held: Held | null = null;
  /** The row being dragged, and where the pointer took it. */
  private dragging: { key: string; pointer: number; startY: number } | null = null;

  constructor(
    private readonly key: string,
    private readonly words: QueueWords,
    private readonly actions: QueueActions,
  ) {
    this.element = document.createElement("div");
    this.element.className = "send-queue";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => this.click(event));
    this.element.addEventListener("keydown", (event) => this.keyed(event));
    this.element.addEventListener("pointerdown", (event) => this.grab(event));
    this.draw();
  }

  get items(): Queued[] {
    return kept.get(this.key) ?? [];
  }

  private set items(items: Queued[]) {
    if (items.length) kept.set(this.key, items);
    else kept.delete(this.key);
  }

  get count(): number {
    return this.items.length;
  }

  /** The one to send next, unless it is being changed. */
  head(): Queued | null {
    const first = this.items[0];
    return first && first.key !== this.editing ? first : null;
  }

  add(text: string, attached: Queued["attached"]): void {
    serial += 1;
    this.items = [...this.items, { key: `queued-${serial}`, text, attached }];
    this.draw();
  }

  /** Takes one out to be sent. */
  take(key: string): Queued | null {
    const item = this.items.find((candidate) => candidate.key === key) ?? null;
    if (item) this.items = this.items.filter((candidate) => candidate !== item);
    this.draw();
    return item;
  }

  /** Whether the queue waits on the user, and why. */
  hold(held: Held | null): void {
    if (held === this.held) return;
    this.held = held;
    this.draw();
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-queue]");
    if (!button) return;
    const key = button.closest<HTMLElement>("[data-key]")?.dataset.key;
    if (!key) return;
    switch (button.dataset.queue) {
      case "now": {
        const item = this.take(key);
        if (item) this.actions.now(item);
        return;
      }
      case "edit":
        this.editing = key;
        this.draw();
        this.element.querySelector<HTMLTextAreaElement>(".queue-edit")?.focus();
        return;
      case "save":
        this.save(key);
        return;
      case "cancel":
        this.editing = null;
        this.draw();
        return;
      case "remove":
        this.items = this.items.filter((item) => item.key !== key);
        if (this.editing === key) this.editing = null;
        this.draw();
    }
  }

  private save(key: string): void {
    const field = this.element.querySelector<HTMLTextAreaElement>(".queue-edit");
    const item = this.items.find((candidate) => candidate.key === key);
    if (!field || !item) return;
    const text = field.value.trim();
    if (!text && !item.attached.refs.length) {
      field.setAttribute("aria-invalid", "true");
      field.placeholder = this.words.empty;
      return;
    }
    this.items = this.items.map((candidate) => (candidate === item ? { ...item, text } : candidate));
    this.editing = null;
    this.draw();
  }

  private keyed(event: KeyboardEvent): void {
    const target = event.target as HTMLElement;
    // Changing one: Enter keeps it, Escape leaves it as it was.
    if (target.classList.contains("queue-edit")) {
      if (composing(event)) return;
      const key = target.closest<HTMLElement>("[data-key]")?.dataset.key;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        this.editing = null;
        this.draw();
      } else if (event.key === "Enter" && !event.shiftKey && key) {
        event.preventDefault();
        this.save(key);
      }
      return;
    }
    // Alt with an arrow moves the row that has focus.
    const row = target.closest<HTMLElement>("[data-key]");
    if (!row || !event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    event.preventDefault();
    this.move(row.dataset.key!, event.key === "ArrowUp" ? -1 : 1);
    this.element.querySelector<HTMLElement>(`[data-key="${row.dataset.key}"] .queue-grip`)?.focus();
  }

  private move(key: string, by: number): void {
    const items = [...this.items];
    const at = items.findIndex((item) => item.key === key);
    const to = Math.max(0, Math.min(items.length - 1, at + by));
    if (at < 0 || at === to) return;
    const [item] = items.splice(at, 1);
    items.splice(to, 0, item!);
    this.items = items;
    this.draw();
  }

  /** Rows are moved by their grip with the pointer: the row follows it,
   *  and lands where it is let go. */
  private grab(event: PointerEvent): void {
    const grip = (event.target as HTMLElement).closest<HTMLElement>(".queue-grip");
    const row = grip?.closest<HTMLElement>("[data-key]");
    if (!grip || !row || this.editing) return;
    event.preventDefault();
    grip.setPointerCapture(event.pointerId);
    this.dragging = { key: row.dataset.key!, pointer: event.pointerId, startY: event.clientY };
    row.classList.add("dragging");
    const moved = (move: PointerEvent) => {
      if (!this.dragging || move.pointerId !== this.dragging.pointer) return;
      row.style.transform = `translateY(${move.clientY - this.dragging.startY}px)`;
    };
    const dropped = (up: PointerEvent) => {
      grip.removeEventListener("pointermove", moved);
      grip.removeEventListener("pointerup", dropped);
      grip.removeEventListener("pointercancel", dropped);
      const dragging = this.dragging;
      this.dragging = null;
      row.classList.remove("dragging");
      row.style.transform = "";
      if (!dragging || up.type === "pointercancel") return;
      // Where it lands: among the other rows, by their middles.
      const rows = [...this.element.querySelectorAll<HTMLElement>("[data-key]")].filter((other) => other !== row);
      const before = rows.filter((other) => {
        const box = other.getBoundingClientRect();
        return box.top + box.height / 2 < up.clientY;
      }).length;
      const items = this.items.filter((item) => item.key !== dragging.key);
      const item = this.items.find((candidate) => candidate.key === dragging.key);
      if (!item) return;
      items.splice(before, 0, item);
      this.items = items;
      this.draw();
    };
    grip.addEventListener("pointermove", moved);
    grip.addEventListener("pointerup", dropped);
    grip.addEventListener("pointercancel", dropped);
  }

  draw(): void {
    const { words } = this;
    const items = this.items;
    this.element.hidden = items.length === 0;
    const head = this.held
      ? `<div class="queue-head held">${escape(this.held === "stopped" ? words.held : this.held === "unsure" ? words.unsure : words.notTaken)}</div>`
      : `<div class="queue-head">${escape(words.waiting(items.length))}</div>`;
    const rows = items
      .map((item) => {
        const files = item.attached.shown.length ? `<span class="queue-files">${escape(words.files(item.attached.shown.length))}</span>` : "";
        if (item.key === this.editing) {
          return `<li class="queue-row editing" data-key="${item.key}"><textarea class="queue-edit" rows="2" aria-label="${escape(words.edit)}">${escape(item.text)}</textarea>${files}<span class="queue-acts"><button class="text-button" type="button" data-queue="save">${escape(words.save)}</button><button class="text-button" type="button" data-queue="cancel">${escape(words.cancel)}</button></span></li>`;
        }
        const said = item.text.replace(/\s+/g, " ").trim();
        return `<li class="queue-row" data-key="${item.key}"><button class="queue-grip" type="button" aria-label="${escape(words.reorder)}" title="${escape(words.reorder)}">${glyph("grip", 14)}</button><span class="queue-text" title="${escape(item.text)}">${escape(said)}</span>${files}<span class="queue-acts"><button class="text-button queue-now" type="button" data-queue="now" title="${escape(words.sendNowTitle)}">${glyph("guide", 14)}<span>${escape(words.sendNow)}</span></button><button class="icon-button" type="button" data-queue="edit" aria-label="${escape(words.edit)}" title="${escape(words.edit)}">${glyph("edit", 14)}</button><button class="icon-button" type="button" data-queue="remove" aria-label="${escape(words.remove)}" title="${escape(words.remove)}">${glyph("close", 14)}</button></span></li>`;
      })
      .join("");
    this.element.innerHTML = `${head}<ol class="queue-rows">${rows}</ol>`;
    this.actions.changed();
  }
}
