// The plan's other pictures: a timeline on a ruled rail, a tree whose
// branches are drawn in ink, a decision put to the user, and a note that
// says a risk or a good thing in a line or two. Words stay the interface's
// own type; only the rails, the branches and the marks are drawn.

import { Sketch } from "../pen/sketch";
import { escape } from "../conversation/prose";
import type { Ask, NoteTone, TimelineRow, TreeRow } from "./source";

export interface BlockWords {
  decide: string;
  suggested: string;
  answered: (option: string) => string;
}

export function timelineElement(rows: TimelineRow[], seed: string): HTMLElement {
  const root = document.createElement("div");
  root.className = "pb-timeline";
  root.dataset.seed = seed;
  root.innerHTML = `<div class="pb-ink" aria-hidden="true"></div>${rows
    .map(
      (row) =>
        `<div class="pb-tl-row${row.strong ? " strong" : ""}"><span class="pb-tl-when">${escape(row.when)}</span><span class="pb-tl-dot"></span><span class="pb-tl-body"><span class="pb-tl-title">${escape(row.title)}</span>${
          row.detail ? `<span class="pb-tl-detail">${escape(row.detail)}</span>` : ""
        }</span></div>`,
    )
    .join("")}`;
  return root;
}

/** The rail through the dots, a drop of ink on each, clay on the one that
 *  matters. */
export function drawTimeline(root: HTMLElement): void {
  const dots = [...root.querySelectorAll<HTMLElement>(".pb-tl-dot")];
  if (!dots.length) return;
  const box = root.getBoundingClientRect();
  const centers = dots.map((dot) => {
    const r = dot.getBoundingClientRect();
    return [r.left - box.left + r.width / 2, r.top - box.top + Math.min(r.height / 2, 11)] as const;
  });
  const sketch = new Sketch(`timeline-${root.dataset.seed ?? ""}`);
  const first = centers[0]!;
  const last = centers[centers.length - 1]!;
  if (centers.length > 1) sketch.rule([first[0], first[1]], [last[0], last[1]], 1.1, "graphite");
  dots.forEach((dot, at) => {
    const strong = dot.parentElement!.classList.contains("strong");
    sketch.blot([centers[at]![0], centers[at]![1]], strong ? 4.6 : 3.4, strong ? "clay" : "graphite");
  });
  root.querySelector<HTMLElement>(".pb-ink")!.innerHTML = sketch.svg(root.clientWidth, root.clientHeight, "pb-ink-svg");
}

export function treeElement(rows: TreeRow[], seed: string): HTMLElement {
  const root = document.createElement("div");
  root.className = "pb-tree";
  root.dataset.seed = seed;
  root.innerHTML = `<div class="pb-ink" aria-hidden="true"></div>${rows
    .map(
      (row) =>
        `<div class="pb-tree-row" data-depth="${row.depth}" style="--depth:${row.depth}"><span class="pb-tree-label">${escape(row.label)}</span>${
          row.note ? `<span class="pb-tree-note">${escape(row.note)}</span>` : ""
        }</div>`,
    )
    .join("")}`;
  return root;
}

/** Each branch from under its parent to the part it holds. */
export function drawTree(root: HTMLElement): void {
  const rows = [...root.querySelectorAll<HTMLElement>(".pb-tree-row")];
  const box = root.getBoundingClientRect();
  const sketch = new Sketch(`tree-${root.dataset.seed ?? ""}`);
  rows.forEach((row, at) => {
    const depth = Number(row.dataset.depth);
    if (depth === 0) return;
    let parent = at - 1;
    while (parent >= 0 && Number(rows[parent]!.dataset.depth) >= depth) parent -= 1;
    if (parent < 0) return;
    const p = rows[parent]!.querySelector<HTMLElement>(".pb-tree-label")!.getBoundingClientRect();
    const c = row.querySelector<HTMLElement>(".pb-tree-label")!.getBoundingClientRect();
    const x = p.left - box.left + 6;
    const y0 = p.bottom - box.top + 1;
    const y1 = c.top - box.top + c.height / 2;
    sketch.rule([x, y0], [x, y1 - 3], 1, "graphite");
    sketch.rule([x, y1 - 3], [c.left - box.left - 5, y1], 1, "graphite");
  });
  root.querySelector<HTMLElement>(".pb-ink")!.innerHTML = sketch.svg(root.clientWidth, root.clientHeight, "pb-ink-svg");
}

/** A decision for the user, set as a question on paper: the question with
 *  a marker's light sweep beneath it, its options a column of lines, the
 *  suggested one noted in the accent. Pressing one lays the clay wash under
 *  it and tells the conversation, as the user's own words. */
export function askElement(ask: Ask, words: BlockWords, seed: string, answer: (question: string, option: string) => Promise<void> | void): HTMLElement {
  const root = document.createElement("div");
  root.className = "pb-ask";
  root.dataset.seed = seed;
  root.setAttribute("role", "group");
  root.setAttribute("aria-label", words.decide);
  root.innerHTML = `<div class="pb-ink" aria-hidden="true"></div><p class="pb-ask-question"><span>${escape(ask.question)}</span></p><div class="pb-ask-options">${ask.options
    .map(
      (option, at) =>
        `<button class="pb-ask-option${option.suggested ? " suggested" : ""}" type="button" data-at="${at}"><span class="pb-ask-text">${escape(option.text)}${
          option.suggested ? `<span class="pb-ask-tag">${escape(words.suggested)}</span>` : ""
        }${option.note ? `<span class="pb-ask-note">${escape(option.note)}</span>` : ""}</span></button>`,
    )
    .join("")}</div>`;
  root.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(".pb-ask-option");
    if (!button || root.classList.contains("answered")) return;
    const option = ask.options[Number(button.dataset.at)]!;
    root.classList.add("answered");
    button.classList.add("chosen");
    root.setAttribute("aria-label", words.answered(option.text));
    // Chosen once it went in; otherwise it can be picked again.
    void Promise.resolve(answer(ask.question, option.text)).catch(() => {
      root.classList.remove("answered");
      button.classList.remove("chosen");
      root.setAttribute("aria-label", words.decide);
    });
  });
  return root;
}

/** The sweep beneath the question. */
export function drawAsk(root: HTMLElement): void {
  const box = root.getBoundingClientRect();
  const sketch = new Sketch(`ask-${root.dataset.seed ?? ""}`);
  const question = root.querySelector<HTMLElement>(".pb-ask-question > span");
  if (question) {
    for (const line of question.getClientRects()) {
      sketch.wash(line.left - box.left - 4, line.top - box.top + line.height * 0.52, line.width + 8, line.height * 0.46, "accent", 3);
    }
  }
  root.querySelector<HTMLElement>(".pb-ink")!.innerHTML = sketch.svg(root.clientWidth, root.clientHeight, "pb-ink-svg");
}

/** A risk, a good thing or a fact to keep in mind, opened with the old
 *  reference mark ※ (a tick for a good thing) and no box around it. */
export function noteElement(tone: NoteTone, html: string, seed: string): HTMLElement {
  const root = document.createElement("div");
  root.className = `pb-note ${tone}`;
  const ink = tone === "warn" ? "clay" : "graphite";
  const sketch = new Sketch(`note-${seed}`);
  if (tone === "ok") sketch.stroke([[3, 8.5], [6.5, 12], [13, 4.5]], 1.6, ink, 0.14);
  else
    sketch
      .stroke([[4.2, 4.2], [11.8, 11.8]], 1.35, ink, 0.12)
      .stroke([[11.8, 4.2], [4.2, 11.8]], 1.35, ink, 0.12)
      .blot([8, 2.4], 1.05, ink)
      .blot([8, 13.6], 1.05, ink)
      .blot([2.4, 8], 1.05, ink)
      .blot([13.6, 8], 1.05, ink);
  root.innerHTML = `<span class="pb-note-mark">${sketch.svg(16, 16, "pb-note-svg")}</span><div class="pb-note-body prose">${html}</div>`;
  return root;
}
