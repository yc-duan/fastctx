// The rows of what a run did through the World (`trip.ts`), drawn as the
// stretch's other rows are: a glyph in the rail, a verb, what it acted on,
// and, quiet at the end, the machine it happened on. Nothing is added to say
// it is a step of a script: a row is one thing done, and where. A command
// sent to many machines is one row with how many; opened, a quiet grid of
// them, one tile a machine, those that did not end well first. A command
// whose input flowed straight from the row above begins with a small hook
// from it, which says so in words under the pointer. Opened, a command sent elsewhere shows what was sent, its
// last lines and how it ended; a copy, from where to where and how much; a
// look, the files on their machine.

import { bytesSaid } from "../bytes";
import type { CopyStep, OnStep, RunStep, ViewStep, WriteFact } from "../world/run-facts";
import type { Words } from "../words";
import { glyph } from "./glyphs";
import type { LongLines, LongText } from "./long-text";
import { escape } from "./prose";
import { handOnAtEdge } from "../scroll-handoff";
import { commandSaid, leaf, ownWork, tripItems, type TripItem } from "./trip";

/** A row of a run's that is one of its steps. */
export type StepItem = Exclude<TripItem, { kind: "own" } | { kind: "write" }>;

export interface TripDraw {
  words: Words;
  enter: string;
  open: boolean;
}

/** A row's key part for a step: its kind and its first step's id. */
export function stepKey(item: StepItem): string {
  return item.kind === "fan" ? `fan:${item.steps[0]!.id}` : `${item.kind}:${item.step.id}`;
}

export function stepRowHtml(key: string, item: StepItem, draw: TripDraw): string {
  const { words } = draw;
  const said = rowSaid(item, draw);
  const mark = said.running ? `<span class="pulse" aria-hidden="true"></span>` : glyph(said.glyph);
  const fold = `<div class="fold-box"${draw.open ? "" : ' style="height:0" inert'}><div class="fold-content"></div></div>`;
  const pipe = said.pipe ? `<span class="row-fed" title="${escape(words.trip.fed)}">${glyph("fed", 12)}</span>` : "";
  return `<div class="row row-tool row-${said.running ? "running" : "done"}${draw.enter}" data-row="${escape(key)}"><span class="rail">${mark}</span><button class="row-line" type="button" data-open="${escape(key)}" aria-expanded="${draw.open}"><span class="row-action">${escape(words.act(said.act, said.running ? "running" : "done"))}</span><span class="row-rest"><span class="row-object">${pipe}${escape(said.object)}</span><span class="row-meta">${said.meta}<span class="row-chevron">${glyph("chevron", 16)}</span></span></span></button>${fold}</div>`;
}

/** A step's row as find reads it. */
export function stepRowText(item: StepItem, draw: Omit<TripDraw, "enter" | "open">): string {
  const said = rowSaid(item, { ...draw, enter: "", open: false });
  const meta = said.meta.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
  return [draw.words.act(said.act, said.running ? "running" : "done"), said.object, meta].filter(Boolean).join(" ");
}

interface RowSaid {
  act: "command" | "copy" | "read";
  glyph: "command" | "move" | "read";
  object: string;
  meta: string;
  running: boolean;
  /** Its input came straight from the row above. */
  pipe?: boolean;
}

function rowSaid(item: StepItem, draw: TripDraw): RowSaid {
  const { words } = draw;
  // Every step names its machine, this computer too: every script says where
  // it runs (`world-run.md`).
  const machine = (name: string) => `<span class="row-machine">${glyph("machine", 12)}${escape(words.on(name))}</span>`;
  switch (item.kind) {
    case "on": {
      // As with any command, its exit code is no outcome of the row; a
      // command stopped, or whose machine went away under it, is.
      const end = item.step.end;
      const outcome = end?.kind === "stopped" ? `<span class="row-outcome">${escape(words.trip.stopped)}</span>` : end?.kind === "lost" ? `<span class="row-outcome wrong">${escape(words.trip.unfinished)}</span>` : "";
      return { act: "command", glyph: "command", object: commandSaid(item.step.command), meta: outcome + machine(item.step.machine), running: end === null, pipe: item.fed };
    }
    case "fan": {
      const troubled = item.steps.filter(troubledStep).length;
      const many = `<span class="row-machine">${glyph("group", 12)}${escape(words.trip.machines(item.steps.length))}</span>`;
      const bad = troubled ? `<span class="row-trouble">${escape(words.trip.troubled(troubled))}</span>` : "";
      return { act: "command", glyph: "command", object: commandSaid(item.steps[0]!.command), meta: many + bad, running: item.steps.some((step) => step.end === null) };
    }
    case "copy": {
      const sources = [...new Set(item.step.from.map((from) => from.machine))];
      // Each machine with its mark, as every row names one.
      const named = (name: string) => `${glyph("machine", 12)}${escape(words.on(name))}`;
      const route = `<span class="row-machine">${sources.map(named).join(escape(words.listSeparator))}<span class="row-route">→</span>${named(item.step.to.machine)}</span>`;
      return { act: "copy", glyph: "move", object: item.step.from.map((from) => leaf(from.path)).join(words.listSeparator), meta: route, running: item.step.ended === null };
    }
    case "view":
      return { act: "read", glyph: "read", object: item.step.paths.map(leaf).join(words.listSeparator), meta: machine(item.step.machine), running: false };
  }
}

/** What a run still going does now, as its stretch's heading says it: the
 *  last of its rows still running, else what the script does of its own;
 *  null where neither is known. */
export function tripNow(script: string, steps: readonly RunStep[], writes: readonly WriteFact[], words: Words): { action: string; object: string } | null {
  const items = tripItems(script, steps, writes);
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index]!;
    if (item.kind === "own" || item.kind === "write") continue;
    const said = rowSaid(item, { words, enter: "", open: false });
    if (said.running) return { action: words.act(said.act, "running"), object: said.object };
  }
  const own = ownWork(script);
  return own === null ? null : { action: words.act("command", "running"), object: own };
}

/** A step that did not end as it should: it exited other than 0, was
 *  stopped, or its machine went away under it. */
function troubledStep(step: OnStep): boolean {
  return step.end !== null && !(step.end.kind === "exited" && step.end.code === 0);
}

/** How a command sent elsewhere ended, and how long it took. */
function endingOf(step: OnStep, words: Words): { said: string; bad: boolean } {
  const took = step.ended ? words.took(Date.parse(step.ended) - Date.parse(step.began)) : "";
  if (!step.end) return { said: took, bad: false };
  if (step.end.kind === "stopped") return { said: words.trip.stopped, bad: true };
  if (step.end.kind === "lost") return { said: words.trip.lost(step.end.why), bad: true };
  return { said: [words.exited(step.end.code), took].filter(Boolean).join("　"), bad: step.end.code !== 0 };
}

/** What a step's row opens on. `long` makes a box of lines the page keeps. */
export function fillStep(item: StepItem, content: HTMLElement, words: Words, long: (text: string | LongLines, label: string) => LongText): LongText[] {
  const longs: LongText[] = [];
  switch (item.kind) {
    case "on":
      content.append(onDetail(item.step, words, (text, label) => track(longs, long(text, label))));
      break;
    case "fan":
      content.append(fanDetail(item.steps, words, longs, long));
      break;
    case "copy":
      content.append(copyDetail(item.step, words));
      break;
    case "view":
      content.append(viewDetail(item.step, words));
      break;
  }
  return longs;
}

function track(longs: LongText[], made: LongText): LongText {
  longs.push(made);
  return made;
}

/** A command sent to a machine: what was sent, above its last lines, and
 *  how it ended at the foot. */
function onDetail(step: OnStep, words: Words, long: (text: string | LongLines, label: string) => LongText): HTMLElement {
  const surface = document.createElement("div");
  surface.className = "surface io";
  const part = (side: "in" | "out", label: string, text: string, end = "") => {
    const holder = document.createElement("div");
    holder.className = `io-part io-${side}`;
    holder.setAttribute("role", "group");
    holder.setAttribute("aria-label", label);
    if (text) holder.append(long(text, label).element);
    if (end) holder.insertAdjacentHTML("beforeend", `<div class="io-foot">${end}</div>`);
    surface.append(holder);
  };
  const ending = endingOf(step, words);
  part("in", words.trip.command, step.command);
  part("out", words.trip.lastLines, step.tail ?? "", ending.said ? `<span class="io-end${ending.bad ? " nonzero" : ""}">${escape(ending.said)}</span>` : "");
  return surface;
}

/** One command sent to many machines: a tile each, those that did not end
 *  well first; pressed, a tile shows that machine's command, last lines and
 *  ending beneath the grid. */
function fanDetail(steps: readonly OnStep[], words: Words, longs: LongText[], long: (text: string | LongLines, label: string) => LongText): HTMLElement {
  const holder = document.createElement("div");
  holder.className = "fan";
  // The boxes of the machine shown, let go when another is chosen.
  let shown: LongText[] = [];
  const order = [...steps].sort((a, b) => Number(troubledStep(b)) - Number(troubledStep(a)));
  holder.innerHTML = `<div class="fan-grid" role="list">${order
    .map((step) => {
      const ending = endingOf(step, words);
      const state = step.end === null ? " going" : troubledStep(step) ? " troubled" : "";
      const mark = step.end === null ? `<span class="pulse" aria-hidden="true"></span>` : glyph("machine", 14);
      return `<button class="fan-tile${state}" type="button" role="listitem" data-fan="${escape(step.id)}" aria-pressed="false" title="${escape([words.on(step.machine), ending.said].filter(Boolean).join("\n"))}">${mark}<span class="fan-name">${escape(words.on(step.machine))}</span></button>`;
    })
    .join("")}</div><div class="fan-chosen" hidden></div>`;
  const chosen = holder.querySelector<HTMLElement>(".fan-chosen")!;
  holder.addEventListener("click", (event) => {
    const tile = (event.target as HTMLElement).closest<HTMLElement>("[data-fan]");
    if (!tile) return;
    event.stopPropagation();
    const pressed = tile.getAttribute("aria-pressed") !== "true";
    for (const other of holder.querySelectorAll<HTMLElement>("[data-fan]")) other.setAttribute("aria-pressed", "false");
    tile.setAttribute("aria-pressed", String(pressed));
    chosen.replaceChildren();
    chosen.hidden = !pressed;
    for (const gone of shown) {
      gone.destroy();
      longs.splice(longs.indexOf(gone), 1);
    }
    shown = [];
    const step = steps.find((one) => one.id === tile.dataset.fan);
    if (pressed && step) chosen.append(onDetail(step, words, (text, label) => track(shown, track(longs, long(text, label)))));
  });
  handOnAtEdge(holder.querySelector<HTMLElement>(".fan-grid")!);
  return holder;
}

/** Files copied between machines: from where, to where, how much, and
 *  what could not be. */
function copyDetail(step: CopyStep, words: Words): HTMLElement {
  const holder = document.createElement("div");
  holder.className = "step-facts";
  const place = (machine: string, path: string) => `${stepMachine(machine, words)}<span class="step-path">${escape(path)}</span>`;
  holder.innerHTML = `<dl>
      <dt>${escape(words.trip.from)}</dt><dd>${step.from.map((from) => `<div>${place(from.machine, from.path)}</div>`).join("")}</dd>
      <dt>${escape(words.trip.to)}</dt><dd><div>${place(step.to.machine, step.to.path)}</div></dd>
    </dl>
    <p class="step-sum">${escape(words.trip.copied(step.files, bytesSaid(step.bytes)))}</p>
    ${step.problems.length ? `<p class="step-problems"><span>${escape(words.trip.problems)}</span>${step.problems.map((problem) => `<span>${escape(problem)}</span>`).join("")}</p>` : ""}`;
  return holder;
}

/** Files looked at on a machine: each in full. */
function viewDetail(step: ViewStep, words: Words): HTMLElement {
  const holder = document.createElement("div");
  holder.className = "step-facts";
  holder.innerHTML = step.paths.map((path) => `<div>${stepMachine(step.machine, words)}<span class="step-path">${escape(path)}</span></div>`).join("");
  return holder;
}

/** A machine in a step's facts, with its mark as every row names one. */
function stepMachine(machine: string, words: Words): string {
  return `<span class="step-machine">${glyph("machine", 12)}${escape(words.on(machine))}</span>`;
}
