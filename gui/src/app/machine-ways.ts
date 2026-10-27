// How several machines chosen together update (`world-upgrade.md`, 别的机器
// 的更新): whether each takes new releases by itself, and whether it passes
// packages on to the network, each set on, off or left as each one has it.
// Those that cannot be set now are counted with why; the sheet closes once
// every one that can was asked, and what each said stands on its row.

import { escape } from "../conversation/prose";
import { Sheet } from "./confirm";
import type { BatchWords } from "./machine-batch-words";

/** On, off, or as each one has it. */
export type Way = boolean | null;

export interface Ways {
  /** How many it is applied to, and of the rest why they are left. */
  count: number;
  skipped: string[];
  apply(auto: Way, relays: Way): Promise<void>;
}

export class WaysSheet {
  private sheet: Sheet | null = null;
  private ways: Ways | null = null;
  private auto: Way = null;
  private relays: Way = null;
  private said = "";

  constructor(
    private readonly words: BatchWords,
    private readonly close: string,
  ) {}

  open(ways: Ways): void {
    this.ways = ways;
    this.auto = null;
    this.relays = null;
    this.said = "";
    const sheet = this.sheet ?? this.make();
    this.draw();
    sheet.show();
  }

  private make(): Sheet {
    const sheet = new Sheet("device-sheet ways-sheet");
    this.sheet = sheet;
    sheet.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const choice = target.closest<HTMLButtonElement>("[data-way]");
      if (choice) {
        const value: Way = choice.dataset.value === "on" ? true : choice.dataset.value === "off" ? false : null;
        if (choice.dataset.way === "auto") this.auto = value;
        else this.relays = value;
        this.draw();
        return;
      }
      if (target.closest("[data-ways-go]")) this.go();
    });
    return sheet;
  }

  private go(): void {
    const { sheet, ways } = this;
    if (!sheet || !ways || sheet.busy || (this.auto === null && this.relays === null)) return;
    sheet.busy = true;
    this.draw();
    ways.apply(this.auto, this.relays).then(
      () => {
        sheet.busy = false;
        sheet.close();
      },
      (error: unknown) => {
        sheet.busy = false;
        this.said = error instanceof Error ? error.message : String(error);
        this.draw();
      },
    );
  }

  private draw(): void {
    const { sheet, ways, words } = this;
    if (!sheet || !ways) return;
    const row = (way: "auto" | "relays", label: string, now: Way) =>
      `<div class="choice-row"><span class="choice-label">${escape(label)}</span><div class="choice-options" role="radiogroup" aria-label="${escape(label)}">${([true, false, null] as Way[])
        .map((value) => {
          const chosen = value === now;
          const name = value === true ? words.on : value === false ? words.off : words.keep;
          return `<button type="button" class="choice${chosen ? " chosen" : ""}" role="radio" aria-checked="${chosen}" data-way="${way}" data-value="${value === true ? "on" : value === false ? "off" : "keep"}">${escape(name)}</button>`;
        })
        .join("")}</div></div>`;
    const skipped = ways.skipped.length ? `<ul class="ways-skipped">${ways.skipped.map((one) => `<li>${escape(one)}</li>`).join("")}</ul>` : "";
    const ready = this.auto !== null || this.relays !== null;
    sheet.body.innerHTML = `${sheet.head(words.waysTitle(ways.count), this.close)}
      ${row("auto", words.auto, this.auto)}${row("relays", words.relays, this.relays)}
      ${skipped}
      <p class="sheet-note" role="alert"${this.said ? "" : " hidden"}>${escape(this.said)}</p>
      <div class="sheet-foot mr-go"><button class="sheet-go" type="button" data-ways-go${sheet.busy || !ready ? " disabled" : ""}>${escape(words.apply(ways.count))}</button></div>`;
  }
}
