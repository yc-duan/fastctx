// A repair's layers as it goes through them, one row each, ticked in place
// as the uninstall dialog ticks its steps: a faint dot that breathes while
// a layer is checked, a pencil tick once it is fine, the tick in clay where
// it was mended, a cross where it could not be. How each went is said when
// pointed at, with the repair's own words after it. A row already drawn is
// left as it is while the rest come in.

import { escape } from "../conversation/prose";
import { stepMark } from "../pen/step-mark";
import type { RepairStep } from "../world/repair";
import type { RepairWords } from "./repair-words";
import "./removal.css";
import "./repair-layers.css";

export class RepairLayers {
  readonly element: HTMLOListElement;

  constructor(private readonly words: RepairWords) {
    this.element = document.createElement("ol");
    this.element.className = "removal-list rp-layers";
  }

  update(steps: readonly RepairStep[]): void {
    const words = this.words;
    const kept = new Set<string>();
    steps.forEach((step, at) => {
      kept.add(step.layer);
      let row = this.element.querySelector<HTMLLIElement>(`li[data-layer="${CSS.escape(step.layer)}"]`);
      if (!row) {
        row = document.createElement("li");
        row.dataset.layer = step.layer;
        row.innerHTML = `<span class="removal-mark" aria-hidden="true"></span><span class="removal-what">${escape((words.layers as Record<string, string>)[step.layer] ?? step.layer)}</span>`;
      }
      // In the order the repair went, a row moved only where it is out of it.
      if (this.element.children[at] !== row) this.element.insertBefore(row, this.element.children[at] ?? null);
      const result = step.outcome === "failed" ? "failed" : step.outcome === "checking" ? null : "done";
      const drawn = `${step.outcome}`;
      if (row.dataset.drawn !== drawn) {
        row.dataset.drawn = drawn;
        row.querySelector(".removal-mark")!.innerHTML = stepMark(result);
        if (result) row.dataset.result = result;
        else delete row.dataset.result;
        row.toggleAttribute("data-repaired", step.outcome === "repaired");
        row.dataset.state = step.outcome === "checking" ? "now" : "";
      }
      const said =
        step.outcome === "checking" ? words.checking : step.outcome === "unchanged" ? words.unchanged : step.outcome === "repaired" ? words.fixed : words.broken;
      row.title = step.detail ? words.detail(said, step.detail) : said;
    });
    for (const row of [...this.element.querySelectorAll<HTMLLIElement>("li[data-layer]")]) if (!kept.has(row.dataset.layer!)) row.remove();
  }
}
