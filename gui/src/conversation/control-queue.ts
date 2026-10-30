// Commands the node took to do once the turn under way ends (`world-gui.md`,
// 对话): lines in the queue of words that wait above the input, each where
// the node keeps it among them and moved as they are, the command and
// whether it is being done, waiting ones withdrawable. They are the node's,
// the same on every device and after the page opens again: what it holds
// comes with what the conversation offers, each one's progress with its
// events, and its place with the shared input. Until the shared input opens
// they stand in a block of their own.

import { glyph } from "./glyphs";
import { escape } from "./prose";
import type { PendingControl } from "../world/control";
import type { ComposerCommand } from "../world/composer";

export interface ControlQueueWords {
  /** Beside one being done. */
  nowRunning: string;
  withdraw: string;
}

/** The command a control is, as the composer names it. */
export function controlName(row: PendingControl): string {
  const control = row.control;
  switch (control.do) {
    case "command":
      return control.name;
    default:
      return control.do;
  }
}

/** A control as the queue shows it: whether it has a place to be moved
 *  from. */
export interface PlacedControl {
  row: PendingControl;
  placed: boolean;
}

/** The controls shown before each of the words that wait, and after the
 *  last, as the node does them: one being done first, then one with no
 *  place (taken while no words waited), then each after the word the node
 *  keeps it after. */
export function placeControls(
  words: readonly { id: string }[],
  controls: readonly PendingControl[],
  places: readonly ComposerCommand[] = [],
): PlacedControl[][] {
  const slots: PlacedControl[][] = [...words, null].map(() => []);
  const waiting = controls.filter((row) => row.state === "waiting");
  const placed = new Set(places.map((place) => place.id));
  const at = new Map(words.map((word, index) => [word.id, index + 1]));
  for (const row of controls) if (row.state === "running") slots[0]!.push({ row, placed: false });
  for (const row of waiting) if (!placed.has(row.id)) slots[0]!.push({ row, placed: false });
  for (const place of places) {
    const row = waiting.find((one) => one.id === place.id);
    if (row) slots[(place.after && at.get(place.after)) || 0]!.push({ row, placed: true });
  }
  return slots;
}

/** One control's line: moved by its grip where it has a place, the space
 *  of one kept where it has none. */
export function controlRow({ row, placed }: PlacedControl, words: ControlQueueWords, reorder: string | null): string {
  // A new name waiting for the turn to end says what it will be.
  const name = row.control.do === "rename" ? `/rename ${row.control.name}` : `/${controlName(row)}`;
  const waiting = row.state === "waiting";
  const grip =
    waiting && placed && reorder
      ? `<button class="queue-grip" type="button" aria-label="${escape(reorder)}" title="${escape(reorder)}">${glyph("grip", 14)}</button>`
      : `<span class="queue-spot"></span>`;
  const after = waiting
    ? `<span class="queue-acts"><button class="icon-button" type="button" data-queue="withdraw" aria-label="${escape(words.withdraw)}" title="${escape(words.withdraw)}">${glyph("close", 14)}</button></span>`
    : `<span class="queue-state">${escape(words.nowRunning)}</span>`;
  const key = waiting && placed ? ` data-key="${escape(row.id)}"` : "";
  return `<li class="queue-row queue-command"${key} data-control="${escape(row.id)}">${grip}<span class="queue-text">${escape(name)}</span>${after}</li>`;
}

/** The id of the control whose withdrawal a click asks for. */
export function withdrawn(event: Event): string | undefined {
  const button = (event.target as HTMLElement).closest<HTMLElement>('[data-queue="withdraw"]');
  return button?.closest<HTMLElement>("[data-control]")?.dataset.control;
}

export class ControlQueue {
  readonly element: HTMLElement;
  private drawn = "";

  constructor(
    private readonly words: ControlQueueWords,
    /** Withdraws one still waiting. */
    withdraw: (id: string) => void,
  ) {
    this.element = document.createElement("div");
    this.element.className = "send-queue";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => {
      const id = withdrawn(event);
      if (id) withdraw(id);
    });
  }

  draw(rows: PendingControl[]): void {
    const html = placeControls([], rows)
      .flat()
      .map((placed) => controlRow(placed, this.words, null))
      .join("");
    this.element.hidden = !html;
    const markup = html ? `<ol class="queue-rows">${html}</ol>` : "";
    if (markup === this.drawn) return;
    this.drawn = markup;
    this.element.innerHTML = markup;
  }
}
