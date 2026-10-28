// Words waiting for the agent's turn to end, as the node of its machine
// keeps them (`world-gui.md`, 对话): a block above the input, one line each,
// the same on every device. The node sends the first once a turn ends as it
// should, one at a time and never merged; a turn stopped or a word not taken
// holds them, and the node says why. Each can be sent at once into the turn
// under way, changed like the input itself (pasting, attaching, taking a
// file out), taken out, or moved by its grip. Commands the node took to do
// once a turn ends stand among them where the node keeps them, and are
// moved the same way.

import type { ComposerSession, ComposerSessionState } from "./composer";
import { bindComposer, type ComposerDocument } from "./composer";
import type { AttachWords } from "./attach";
import { NodeTray, type AssistantFiles } from "./node-tray";
import type { QueueWords } from "./queue";
import type { Attachment } from "../world/attachments";
import type { ComposerUpdated } from "../world/composer";
import { glyph } from "./glyphs";
import { escape } from "./prose";
import { composing } from "../typing";
import { controlRow, placeControls, withdrawn, type ControlQueueWords } from "./control-queue";
import type { PendingControl } from "../world/control";

interface Editing {
  id: string;
  document: ComposerDocument;
  field: HTMLTextAreaElement;
  tray: NodeTray | null;
  unbind: () => void;
}

export class NodeQueue {
  readonly element: HTMLElement;
  private state: ComposerSessionState | null = null;
  private editing: Editing | null = null;
  private dragging: { id: string; pointer: number; startY: number } | null = null;
  /** The rows drawn last, which rise in no more; none before the first drawing. */
  private rows: Set<string> | null = null;
  private controls: PendingControl[] = [];
  private commands: { words: ControlQueueWords; withdraw: (id: string) => void } | null = null;

  constructor(
    private readonly words: QueueWords,
    private readonly attachWords: AttachWords | null,
    private readonly session: ComposerSession,
    /** Something could not be done: said by the composer. */
    private readonly failed: (why: string) => void,
    private readonly assistantFiles: AssistantFiles | null = null,
    /** A line sent at once: the page shows it below the turn's line until
     *  the session takes it in (walkthrough items 9, 10). */
    private readonly sentNow: ((id: string, text: string, attachments: readonly Attachment[], sending: Promise<ComposerUpdated>) => void) | null = null,
  ) {
    this.element = document.createElement("div");
    this.element.className = "send-queue";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => this.click(event));
    this.element.addEventListener("keydown", (event) => this.keyed(event));
    this.element.addEventListener("pointerdown", (event) => this.grab(event));
    this.element.addEventListener("paste", (event) => {
      const files = [...(event.clipboardData?.files ?? [])];
      if (!files.length || !this.editing) return;
      if (!event.clipboardData?.getData("text/plain")) event.preventDefault();
      this.editing.tray?.addFiles(files);
    });
  }

  /** Shows the commands the node holds among the words, each withdrawable
   *  while it waits. */
  showControls(words: ControlQueueWords, withdraw: (id: string) => void): void {
    this.commands = { words, withdraw };
  }

  setControls(rows: PendingControl[]): void {
    this.controls = rows;
    this.draw();
  }

  /** How many wait. */
  get count(): number {
    return this.state?.composer.queue.length ?? 0;
  }

  private act(done: Promise<unknown>): void {
    done.catch((error: unknown) => this.failed(String(error instanceof Error ? error.message : error)));
  }

  private click(event: Event): void {
    const control = withdrawn(event);
    if (control) {
      this.commands?.withdraw(control);
      return;
    }
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-queue]");
    if (!button) return;
    if (button.dataset.queue === "pick" && this.editing) {
      this.element.querySelector<HTMLInputElement>(".queue-pick:not(.queue-shot)")?.click();
      return;
    }
    if (button.dataset.queue === "shot" && this.editing) {
      this.element.querySelector<HTMLInputElement>(".queue-shot")?.click();
      return;
    }
    const id = button.closest<HTMLElement>("[data-key]")?.dataset.key;
    if (!id) return;
    switch (button.dataset.queue) {
      case "now": {
        const item = this.state?.composer.queue.find((one) => one.id === id);
        const sending = this.session.sendQueued(id);
        if (item) this.sentNow?.(id, item.draft.text, item.draft.attachments, sending);
        this.act(sending);
        return;
      }
      case "edit":
        void this.edit(id);
        return;
      case "done":
        void this.finish();
        return;
      case "remove":
        if (this.editing?.id === id) this.close();
        this.act(this.session.deleteQueued(id));
    }
  }

  /** Changing one: the node holds it back from being sent meanwhile, and
   *  the field is the input's own kind, its words shared live. */
  private async edit(id: string): Promise<void> {
    if (this.editing) await this.finish();
    try {
      const document = await this.session.editQueued(id);
      const field = window.document.createElement("textarea");
      field.className = "queue-edit";
      field.rows = 2;
      field.setAttribute("aria-label", this.words.edit);
      const tray = this.attachWords ? new NodeTray(this.attachWords, this.session, document, this.failed, this.assistantFiles) : null;
      this.editing = { id, document, field, tray, unbind: bindComposer(field, document) };
      this.draw();
      field.focus();
    } catch (error) {
      this.failed(String(error instanceof Error ? error.message : error));
    }
  }

  private async finish(): Promise<void> {
    const editing = this.editing;
    if (!editing) return;
    try {
      editing.unbind();
      editing.tray?.settle();
      await this.session.finishEditing(editing.document);
      if (this.editing === editing) this.editing = null;
      this.draw();
    } catch (error) {
      editing.unbind = bindComposer(editing.field, editing.document);
      this.failed(String(error instanceof Error ? error.message : error));
    }
  }

  private close(): void {
    this.editing?.unbind();
    this.editing = null;
  }

  private keyed(event: KeyboardEvent): void {
    const target = event.target as HTMLElement;
    if (target.classList.contains("queue-edit")) {
      if (composing(event)) return;
      if (event.key === "Escape" || (event.key === "Enter" && !event.shiftKey)) {
        event.preventDefault();
        event.stopPropagation();
        void this.finish();
      }
      return;
    }
    const row = target.closest<HTMLElement>("[data-key]");
    if (!row || !event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    event.preventDefault();
    // Words and commands, as they stand.
    const ids = this.movable().map((one) => one.dataset.key!);
    const at = ids.indexOf(row.dataset.key!);
    const to = event.key === "ArrowUp" ? at - 1 : at + 2;
    if (at < 0 || to < 0 || to > ids.length) return;
    this.act(this.session.moveQueued(row.dataset.key!, ids[to] ?? null));
  }

  /** A row is moved by its grip: it follows the pointer and lands where it
   *  is let go, among the others by their middles. */
  private grab(event: PointerEvent): void {
    const grip = (event.target as HTMLElement).closest<HTMLElement>(".queue-grip");
    const row = grip?.closest<HTMLElement>("[data-key]");
    if (!grip || !row || this.editing) return;
    event.preventDefault();
    grip.setPointerCapture(event.pointerId);
    this.dragging = { id: row.dataset.key!, pointer: event.pointerId, startY: event.clientY };
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
      const others = this.movable().filter((other) => other !== row);
      const next = others.find((other) => {
        const box = other.getBoundingClientRect();
        return box.top + box.height / 2 >= up.clientY;
      });
      this.act(this.session.moveQueued(dragging.id, next?.dataset.key ?? null));
    };
    grip.addEventListener("pointermove", moved);
    grip.addEventListener("pointerup", dropped);
    grip.addEventListener("pointercancel", dropped);
  }

  /** The lines that can be moved, in their order. */
  private movable(): HTMLElement[] {
    return [...this.element.querySelectorAll<HTMLElement>(".queue-row[data-key]:not(.unsure)")];
  }

  draw(state: ComposerSessionState | null = this.state): void {
    this.state = state;
    const { words } = this;
    const composer = state?.composer;
    // Words on their way are said in the conversation, not kept here.
    const queue = (composer?.queue ?? []).filter((item) => !item.sending);
    const unobserved = composer?.unobserved ?? [];
    // The one being changed is gone from the queue: it was sent or removed.
    if (this.editing && !queue.some((item) => item.id === this.editing!.id)) this.close();
    const places = this.commands ? placeControls(queue, this.controls, composer?.commands) : [];
    const commands = places.flat();
    this.element.hidden = queue.length === 0 && unobserved.length === 0 && commands.length === 0;
    if (this.element.hidden) {
      this.element.innerHTML = "";
      delete this.element.dataset.drawn;
      return;
    }
    const waiting = queue.length + commands.filter((one) => one.row.state === "waiting").length;
    const head = composer?.held
      ? `<div class="queue-head held" title="${escape(composer.held)}">${escape(words.paused)}</div>`
      : waiting
        ? `<div class="queue-head">${escape(words.waiting(waiting))}</div>`
        : "";
    const before = (index: number) => (places[index] ?? []).map((one) => controlRow(one, this.commands!.words, words.reorder)).join("");
    const said = (text: string) => text.replace(/\s+/g, " ").trim();
    const files = (count: number) => (count ? `<span class="queue-files">${escape(words.files(count))}</span>` : "");
    const rows = queue
      .map((item, index) => {
        const count = item.draft.attachments.length + item.draft.uploads.length;
        if (this.editing?.id === item.id) {
          return `${before(index)}<li class="queue-row editing" data-key="${escape(item.id)}"><span class="queue-edit-slot"></span><span class="queue-tray-slot"></span><span class="queue-acts">${this.attachWords ? `<button class="icon-button" type="button" data-queue="pick" aria-label="${escape(this.attachWords.attach)}" title="${escape(this.attachWords.attach)}">${glyph("attach", 14)}</button><input class="queue-pick" type="file" multiple hidden>${
            // On a phone the camera is one tap away here too.
            matchMedia("(pointer: coarse)").matches
              ? `<button class="icon-button" type="button" data-queue="shot" aria-label="${escape(this.attachWords.photo)}" title="${escape(this.attachWords.photo)}">${glyph("camera", 14)}</button><input class="queue-shot queue-pick" type="file" accept="image/*" capture="environment" hidden>`
              : ""
          }` : ""}<button class="text-button" type="button" data-queue="done">${escape(words.done)}</button></span></li>`;
        }
        const state = item.editing ? ` <span class="queue-state">${escape(words.editingElsewhere)}</span>` : "";
        const wrong = item.error ? ` title="${escape(item.error)}"` : "";
        return `${before(index)}<li class="queue-row${item.error ? " wrong" : ""}" data-key="${escape(item.id)}"${wrong}><button class="queue-grip" type="button" aria-label="${escape(words.reorder)}" title="${escape(words.reorder)}">${glyph("grip", 14)}</button><span class="queue-text" title="${escape(item.draft.text)}">${escape(said(item.draft.text))}</span>${files(count)}${state}<span class="queue-acts"><button class="text-button queue-now" type="button" data-queue="now" title="${escape(words.sendNowTitle)}">${glyph("guide", 14)}<span>${escape(words.sendNow)}</span></button><button class="icon-button" type="button" data-queue="edit" aria-label="${escape(words.edit)}" title="${escape(words.edit)}">${glyph("edit", 14)}</button><button class="icon-button" type="button" data-queue="remove" aria-label="${escape(words.remove)}" title="${escape(words.remove)}">${glyph("close", 14)}</button></span></li>`;
      })
      .join("");
    // Sent where the World cannot tell whether they arrived: kept apart,
    // never sent again by themselves.
    const unsure = unobserved
      .map(
        (item) =>
          `<li class="queue-row unsure" data-key="${escape(item.id)}"><span class="queue-text" title="${escape(item.draft.text)}">${escape(said(item.draft.text))}</span><span class="queue-state">${escape(words.unsure)}</span><span class="queue-acts"><button class="icon-button" type="button" data-queue="remove" aria-label="${escape(words.remove)}" title="${escape(words.remove)}">${glyph("close", 14)}</button></span></li>`,
      )
      .join("");
    const html = `${head}<ol class="queue-rows">${rows}${before(queue.length)}${unsure}</ol>`;
    if (this.element.dataset.drawn !== html) {
      this.element.dataset.drawn = html;
      this.element.innerHTML = html;
      // A row rises into the queue the first time it is drawn, not the
      // rows there as the page opened, and never again.
      const shown = new Set<string>();
      for (const row of this.element.querySelectorAll<HTMLElement>(".queue-row[data-key]")) {
        const key = row.dataset.key ?? "";
        shown.add(key);
        if (this.rows && !this.rows.has(key)) row.classList.add("fresh");
      }
      this.rows = shown;
      if (this.editing) {
        this.element.querySelector(".queue-edit-slot")?.replaceWith(this.editing.field);
        if (this.editing.tray) {
          this.element.querySelector(".queue-tray-slot")?.replaceWith(this.editing.tray.element);
        }
        for (const pick of this.element.querySelectorAll<HTMLInputElement>(".queue-pick")) {
          pick.addEventListener("change", () => {
            if (pick.files && this.editing) this.editing.tray?.addFiles(pick.files);
            pick.value = "";
          });
        }
      }
    }
    if (this.editing && state) this.editing.tray?.draw(state);
  }
}
