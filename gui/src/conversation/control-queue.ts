// Commands the node took to do once the turn under way ends (`world-gui.md`,
// 对话): a line each above the input, in the look of the words that wait
// there, the command and where it stands, waiting ones withdrawable. They
// are the node's, the same on every device and after the page opens again:
// what it holds comes with what the conversation offers, and each one's
// progress with its events.

import { glyph } from "./glyphs";
import { escape } from "./prose";
import type { PendingControl } from "../world/control";

export interface ControlQueueWords {
  /** Beside one waiting for the turn to end, and one being done. */
  waitsTurn: string;
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

export class ControlQueue {
  readonly element: HTMLElement;
  private drawn = "";

  constructor(
    private readonly words: ControlQueueWords,
    /** Withdraws one still waiting. */
    withdraw: (id: string) => void,
  ) {
    this.element = document.createElement("div");
    this.element.className = "send-queue control-queue";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => {
      const id = (event.target as HTMLElement).closest<HTMLElement>("[data-control-withdraw]")?.dataset.controlWithdraw;
      if (id) withdraw(id);
    });
  }

  draw(rows: PendingControl[]): void {
    const { words } = this;
    const shown = rows.filter((row) => row.state === "waiting" || row.state === "running");
    const html = shown
      .map((row) => {
        const name = `/${controlName(row)}`;
        const acts =
          row.state === "waiting"
            ? `<span class="queue-acts"><button class="icon-button" type="button" data-control-withdraw="${escape(row.id)}" aria-label="${escape(words.withdraw)}" title="${escape(words.withdraw)}">${glyph("close", 14)}</button></span>`
            : "";
        return `<li class="queue-row" data-key="${escape(row.id)}"><span class="queue-text">${escape(name)}</span><span class="queue-state">${escape(row.state === "waiting" ? words.waitsTurn : words.nowRunning)}</span>${acts}</li>`;
      })
      .join("");
    this.element.hidden = !html;
    const markup = html ? `<ol class="queue-rows">${html}</ol>` : "";
    if (markup === this.drawn) return;
    this.drawn = markup;
    this.element.innerHTML = markup;
  }
}
