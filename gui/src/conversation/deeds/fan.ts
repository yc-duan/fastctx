// One command sent to many machines, opened (`world-gui.md`, 对话): a quiet
// grid, a tile a machine, those it did not end well on first and in the
// colour of what went wrong; a tile pressed shows that machine's command,
// its last lines and how it ended beneath. However many machines, the grid
// keeps to its own height and scrolls in itself.

import type { StepEnd } from "../../world/run-facts";
import { glyph } from "../glyphs";
import { escape } from "../prose";
import { handOnAtEdge } from "../../scroll-handoff";
import type { DeedWords, FanDeed } from "./types";

/** Whether a command sent elsewhere did not end well: it exited other
 *  than 0, was stopped, or did not run through. */
export function troubled(end: StepEnd | null | undefined): boolean {
  return !!end && !(end.kind === "exited" && end.code === 0);
}

export function fanCard(deed: FanDeed, words: DeedWords): HTMLElement {
  const holder = document.createElement("div");
  holder.className = "dd-card dd-fan";
  const order = [...deed.steps].sort((a, b) => Number(troubled(b.end)) - Number(troubled(a.end)));
  holder.innerHTML = `<div class="fan"><div class="fan-grid" role="list">${order
    .map((step) => {
      const state = step.ended === undefined ? " going" : troubled(step.end) ? " troubled" : "";
      const mark = step.ended === undefined ? `<span class="pulse" aria-hidden="true"></span>` : glyph("machine", 14);
      const how = step.end ? words.ended(step.end) : "";
      return `<button class="fan-tile${state}" type="button" role="listitem" data-fan="${escape(step.id)}" aria-pressed="false" title="${escape([step.to, how].filter(Boolean).join("\n"))}">${mark}<span class="fan-name">${escape(step.to)}</span></button>`;
    })
    .join("")}</div><div class="fan-chosen" hidden></div></div>`;
  const chosen = holder.querySelector<HTMLElement>(".fan-chosen")!;
  holder.addEventListener("click", (event) => {
    const tile = (event.target as HTMLElement).closest<HTMLElement>("[data-fan]");
    if (!tile) return;
    event.stopPropagation();
    const pressed = tile.getAttribute("aria-pressed") !== "true";
    for (const other of holder.querySelectorAll<HTMLElement>("[data-fan]")) other.setAttribute("aria-pressed", "false");
    tile.setAttribute("aria-pressed", String(pressed));
    const step = deed.steps.find((one) => one.id === tile.dataset.fan);
    chosen.hidden = !pressed || !step;
    chosen.innerHTML = pressed && step ? sentHtml(step.command, step.tail ?? null, step.end ?? null, words) : "";
  });
  handOnAtEdge(holder.querySelector<HTMLElement>(".fan-grid")!);
  return holder;
}

/** A command sent to a machine: the command, its last lines, and how it
 *  ended where it did not end well. */
export function sentHtml(command: string, tail: string | null, end: StepEnd | null, words: DeedWords): string {
  const ending = troubled(end) && end ? `<span class="dd-wrong">${escape(words.ended(end))}</span>` : "";
  return `<div class="dd-sent"><div class="dd-plain">${glyph("command", 14)}<span>${escape(command)}</span></div>${tail ? `<pre class="dd-said-text" aria-label="${escape(words.lastLines)}">${escape(tail)}</pre>` : ""}${ending}</div>`;
}
