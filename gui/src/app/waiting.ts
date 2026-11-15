// Waiting, wherever it is shown (user 2026-11-15: a loading state is
// designed, never a bare line that reads like a fault). Three dots that
// breathe in turn beside what is awaited, said; and where what comes has a
// known shape, that shape drawn faintly, breathing: a few lines, rows, a bar
// over rows, a card. It comes in only after a moment, so what arrives at
// once never flashes it; it is one status to assistive technology; it stays
// still where the interface moves less (`motion.ts`).

import { escape } from "../conversation/prose";

/** The shape of what is awaited, drawn while it is. */
export type Shape = "lines" | "rows" | "bar-rows" | "card" | "none";

export interface Waiting {
  /** What is awaited, said. */
  label: string;
  shape?: Shape;
  /** How many lines or rows the shape has. */
  rows?: number;
}

/** A waiting block, for where a page's or a section's content will be. */
export function waiting({ label, shape = "lines", rows = 3 }: Waiting): string {
  return `<div class="wait-block" role="status" aria-live="polite" aria-busy="true"><p class="wait-said">${dots()}<span>${escape(label)}</span></p>${drawn(shape, rows)}</div>`;
}

/** Waiting said within a line, or in a bar of facts. */
export function waitingInline(label: string): string {
  return `<span class="wait-inline" role="status" aria-live="polite">${dots()}<span>${escape(label)}</span></span>`;
}

/** Says `label` awaited in `element`, in place: a wait said again, as a
 *  read's progress is, keeps its dots where they are rather than starting
 *  them, and their coming in, over. */
export function sayWaiting(element: HTMLElement, label: string): void {
  const said = element.querySelector<HTMLElement>(":scope > .wait-inline > span:last-child");
  if (said) {
    if (said.textContent !== label) said.textContent = label;
  } else element.innerHTML = waitingInline(label);
}

/** The same, as an element, for code that builds elements. */
export function waitingElement(spec: Waiting): HTMLElement {
  const holder = document.createElement("div");
  holder.innerHTML = waiting(spec);
  return holder.firstElementChild as HTMLElement;
}

function dots(): string {
  return `<span class="wait-dots" aria-hidden="true"><span></span><span></span><span></span></span>`;
}

/** Lines of uneven length, as text has. */
const WIDTHS = [92, 78, 85, 64, 88, 71, 80];

function drawn(shape: Shape, rows: number): string {
  if (shape === "none") return "";
  const lines = (count: number) =>
    Array.from({ length: count }, (_, at) => `<span class="wsk wsk-line" style="--w:${WIDTHS[at % WIDTHS.length]}%;--i:${at}"></span>`).join("");
  // A row as a list's are: its mark, its name, what it says at its end.
  const listed = (count: number, from: number) =>
    `<span class="wsk-rows">${Array.from(
      { length: count },
      (_, at) => `<span class="wsk-row" style="--w:${WIDTHS[at % WIDTHS.length]}%;--i:${at + from}"><span class="wsk wsk-mark"></span><span class="wsk wsk-name"></span><span class="wsk wsk-end"></span></span>`,
    ).join("")}</span>`;
  const inner =
    shape === "bar-rows"
      ? `<span class="wsk wsk-bar" style="--i:0"></span>${listed(rows, 1)}`
      : shape === "rows"
        ? listed(rows, 0)
        : shape === "card"
          ? `<span class="wsk wsk-card" style="--i:0"></span>`
          : lines(rows);
  return `<div class="wait-shape" aria-hidden="true">${inner}</div>`;
}
