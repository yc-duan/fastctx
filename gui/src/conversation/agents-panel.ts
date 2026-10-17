// The host's own agents of a conversation as one picture (`world-gui.md`,
// 对话): a mark in the input's row, with how many still work, and over it a
// panel of lanes, one for each agent. Agents that worked at the same time
// are drawn together on a clock of their own, from when the first of them
// began to when the last ended, or to now; an idle minute apart, they are
// another group, under the one before. A lane is drawn in pen over the time
// its agent worked: a tick where each of its steps began, a failed one in
// red; its head breathing while it works; a check where it finished, a
// cross where it failed or was stopped. An agent another started stands
// under that one. A lane pressed brings the row of the call that started it
// into view, opened.
//
// Each stroke is drawn once, in its own units, and laid on its clock by a
// transform: as the clock runs, lanes stretch and move without being drawn
// again, so the hand that drew them does not shiver.

import type { Words } from "../words";
import type { AgentRun } from "./agents";
import { blot, outline, ruled, seeded, type Point } from "../pen/shapes";
import { Popover } from "./popover";
import { Steady } from "../app/steady";
import { glyph } from "./glyphs";

export interface AgentsPanelOptions {
  words: Words;
  /** Brings the row of the call that started an agent into view, opened. */
  show(run: AgentRun): void;
}

/** A lane's stroke, in its own units before it is laid on its clock. */
const SPAN = 1000;
/** The drawing's height, and the middle its strokes run along. */
const HIGH = 16;
const MID = 8;
/** Room kept at the right of a clock for an end's mark, and at its left so
 *  its strokes begin under its words, in pixels. */
const MARK = 16;
const INSET = 8;
/** How long agents may lie idle between them and still be drawn on one
 *  clock: a minute apart, they are another group. */
const APART = 60_000;
/** How many agents started by agents stand one under another, at most:
 *  deeper ones stand as deep as that. */
const DEEPEST = 6;

interface Lane {
  run: AgentRun;
  element: HTMLElement;
  svg: SVGSVGElement;
  stroke: SVGPathElement;
  ticks: SVGGElement;
  /** Its head, where it works now, breathing in place. */
  head: SVGGElement;
  mark: SVGGElement;
  meta: HTMLElement;
  /** The steps drawn, by their item, each a tick laid on the clock. */
  drawn: Map<string, SVGPathElement>;
  /** What the lane was last drawn from. */
  stamp: number;
  state: string;
}

/** Agents that worked at the same time, on one clock. */
interface Group {
  element: HTMLElement;
  list: HTMLOListElement;
  from: HTMLElement;
  to: HTMLElement;
}

export class AgentsPanel {
  private readonly popover: Popover;
  private readonly steady: Steady;
  private readonly title: HTMLElement;
  private readonly groups: HTMLElement;
  private readonly lanes = new Map<string, Lane>();
  /** The groups drawn, by the first agent of each. */
  private readonly drawnGroups = new Map<string, Group>();
  private runs: AgentRun[] = [];
  private width = 0;
  private timer = 0;
  private readonly sizes: ResizeObserver;

  constructor(
    private readonly button: HTMLButtonElement,
    private readonly pop: HTMLElement,
    private readonly options: AgentsPanelOptions,
  ) {
    pop.innerHTML = `<p class="agents-title"></p><div class="agents-groups"></div>`;
    this.title = pop.querySelector(".agents-title")!;
    this.groups = pop.querySelector(".agents-groups")!;
    // It keeps the size it opened at; an agent sent out while it is open
    // may only make it taller (`../app/steady`).
    this.steady = new Steady(pop, true);
    this.popover = new Popover(pop, button, () => {
      this.stopClock();
      this.steady.free();
    });
    button.addEventListener("click", () => {
      this.popover.toggle();
      if (!this.popover.open) return;
      this.lay();
      this.steady.hold();
      // The latest of them is nearest the input.
      pop.scrollTop = pop.scrollHeight;
    });
    pop.addEventListener("click", (event) => {
      const lane = (event.target as HTMLElement).closest<HTMLElement>(".agent-lane");
      const run = lane ? this.lanes.get(lane.dataset.agent!)?.run : undefined;
      if (!run) return;
      this.popover.hide();
      options.show(run);
    });
    // Its lanes are drawn at the width they are shown at.
    this.sizes = new ResizeObserver(() => {
      const width = this.groups.clientWidth;
      if (width && width !== this.width && this.popover.open) this.lay();
    });
    this.sizes.observe(this.groups);
    button.title = options.words.agents.title;
  }

  /** The agents as the conversation now knows them. */
  update(runs: AgentRun[]): void {
    this.runs = runs;
    const going = runs.filter((run) => run.going).length;
    const { words } = this.options;
    this.button.hidden = runs.length === 0;
    if (!runs.length) this.popover.hide();
    this.button.classList.toggle("going", going > 0);
    this.button.setAttribute("aria-label", going ? `${words.agents.title} ${words.agents.running(going)}` : words.agents.title);
    const html = `${glyph("delegate", 16)}${going ? `<span class="agents-count">${going}</span>` : ""}`;
    if (this.button.innerHTML !== html) this.button.innerHTML = html;
    if (this.popover.open) {
      this.lay();
      this.steady.settle();
    }
  }

  destroy(): void {
    this.stopClock();
    this.sizes.disconnect();
  }

  /** Lays every lane on its group's clock as it now stands; while one
   *  works, again each second. */
  private lay(): void {
    const { words } = this.options;
    const now = Date.now();
    const working = this.runs.filter((run) => run.going).length;
    const said = working ? `${words.agents.title} ${words.agents.running(working)}` : words.agents.title;
    if (this.title.textContent !== said) this.title.textContent = said;
    this.width = this.groups.clientWidth;
    const groups = groupsOf(this.runs, now);
    // Lanes and groups no longer shown are let go; the rest stand in order.
    const known = new Set(this.runs.map((run) => run.id));
    for (const [id, lane] of this.lanes) {
      if (known.has(id)) continue;
      lane.element.remove();
      this.lanes.delete(id);
    }
    const kept = new Set(groups.map((group) => group.runs[0]!.id));
    for (const [key, group] of this.drawnGroups) {
      if (kept.has(key)) continue;
      group.element.remove();
      this.drawnGroups.delete(key);
    }
    let previousGroup: Element | null = null;
    for (const { runs, from, to, going } of groups) {
      const group = this.group(runs[0]!.id);
      if ((previousGroup ? previousGroup.nextElementSibling : this.groups.firstElementChild) !== group.element) {
        if (previousGroup) previousGroup.after(group.element);
        else this.groups.prepend(group.element);
      }
      previousGroup = group.element;
      let previous: Element | null = null;
      const room = Math.max(40, this.width - INSET - MARK);
      const x = (at: number) => INSET + ((at - from) / (to - from)) * room;
      for (const run of ordered(runs)) {
        let lane = this.lanes.get(run.id);
        if (!lane) {
          lane = this.lane(run);
          this.lanes.set(run.id, lane);
        }
        // The conversation was read again: the agent is told anew.
        if (lane.run !== run) {
          lane.run = run;
          lane.stamp = -1;
          lane.state = "";
        }
        if ((previous ? previous.nextElementSibling : group.list.firstElementChild) !== lane.element) {
          if (previous) previous.after(lane.element);
          else group.list.prepend(lane.element);
        }
        previous = lane.element;
        this.place(lane, x, now, runs);
      }
      const fromSaid = clockTime(from, words.english);
      const toSaid = going ? words.agents.now : words.took(to - from);
      if (group.from.textContent !== fromSaid) group.from.textContent = fromSaid;
      if (group.to.textContent !== toSaid) group.to.textContent = toSaid;
    }
    if (working && this.popover.open) this.startClock();
    else this.stopClock();
  }

  /** A group's place, made the first time it is shown. */
  private group(key: string): Group {
    let group = this.drawnGroups.get(key);
    if (group) return group;
    const element = document.createElement("section");
    element.className = "agents-group";
    element.innerHTML = `<ol class="agents-lanes"></ol><p class="agents-axis"><span class="agents-from"></span><span class="agents-to"></span></p>`;
    group = {
      element,
      list: element.querySelector("ol")!,
      from: element.querySelector(".agents-from")!,
      to: element.querySelector(".agents-to")!,
    };
    this.drawnGroups.set(key, group);
    return group;
  }

  /** A lane drawn for an agent: its words, and its strokes in their own
   *  units, laid on its clock by `place`. */
  private lane(run: AgentRun): Lane {
    const element = document.createElement("li");
    element.className = "agent-lane";
    element.dataset.agent = run.id;
    element.innerHTML = `<button class="lane-what" type="button"><span class="lane-line"><span class="lane-kind" hidden></span><span class="lane-task"></span><span class="lane-meta"></span></span><svg class="lane-drawing" height="${HIGH}" aria-hidden="true"><g class="lane-ink" filter="url(#pen-marker-lite)"><path class="lane-stroke"/><g class="lane-ticks"></g><g class="lane-head-at"><path class="lane-head"/></g><g class="lane-mark"></g></g></svg></button>`;
    const svg = element.querySelector("svg")!;
    const random = seeded(`lane:${run.id}`);
    const stroke = svg.querySelector<SVGPathElement>(".lane-stroke")!;
    stroke.setAttribute("d", ruled([0, MID], [SPAN, MID], 3, random));
    svg.querySelector<SVGPathElement>(".lane-head")!.setAttribute("d", blot([0, 0], 3.4, random));
    return {
      run,
      element,
      svg,
      stroke,
      ticks: svg.querySelector<SVGGElement>(".lane-ticks")!,
      head: svg.querySelector<SVGGElement>(".lane-head-at")!,
      mark: svg.querySelector<SVGGElement>(".lane-mark")!,
      meta: element.querySelector<HTMLElement>(".lane-meta")!,
      drawn: new Map(),
      stamp: -1,
      state: "",
    };
  }

  /** Lays a lane on its clock: its stroke over the time it worked, its
   *  steps' ticks, its head or its end's mark. */
  private place(lane: Lane, x: (at: number) => number, now: number, runs: readonly AgentRun[]): void {
    const { run } = lane;
    const { words } = this.options;
    if (lane.stamp !== run.stamp) {
      lane.stamp = run.stamp;
      this.words(lane, runs);
      for (const [item, tick] of lane.drawn) {
        if (run.stepOf.has(item)) continue;
        tick.remove();
        lane.drawn.delete(item);
      }
      // Ticks for steps not drawn yet; each is drawn once.
      for (const step of run.steps) {
        let tick = lane.drawn.get(step.item);
        if (!tick) {
          tick = document.createElementNS("http://www.w3.org/2000/svg", "path");
          const random = seeded(`tick:${step.item}`);
          const lean = (random() - 0.5) * 1.6;
          const points: Point[] = [
            [lean, MID - 5.5],
            [lean * 0.4, MID],
            [-lean * 0.3, MID + 5.5],
          ];
          tick.setAttribute("d", outline(points, 1.6, random, { pressure: 0.25, taper: 0.3 }));
          lane.ticks.append(tick);
          lane.drawn.set(step.item, tick);
        }
        tick.classList.toggle("failed", !!step.ended?.failed);
      }
    }
    if (lane.state !== run.state) {
      lane.state = run.state;
      lane.element.className = `agent-lane state-${run.state}`;
      lane.mark.innerHTML = run.going || run.state === "unknown" ? "" : endMark(run);
    }
    const [began, end] = spanOf(run, now);
    const x0 = x(began);
    // Even a moment's work is a stroke one can see.
    const x1 = Math.max(x0 + 3, x(end));
    if (lane.svg.getAttribute("width") !== String(this.width)) {
      lane.svg.setAttribute("width", String(this.width));
      lane.svg.setAttribute("viewBox", `0 0 ${this.width} ${HIGH}`);
    }
    lane.stroke.setAttribute("transform", `translate(${round(x0)} 0) scale(${(x1 - x0) / SPAN} 1)`);
    for (const step of run.steps) lane.drawn.get(step.item)?.setAttribute("transform", `translate(${round(Math.min(x1, Math.max(x0, x(step.at))))} 0)`);
    lane.head.setAttribute("transform", `translate(${round(x1)} ${MID})`);
    lane.head.style.display = run.going ? "" : "none";
    lane.mark.setAttribute("transform", `translate(${round(x1 + 8)} ${MID}) scale(1.25)`);
    // The clock of one that works moves with its group's.
    if (run.going && run.began !== null) {
      const steps = run.tools ?? run.steps.length;
      const said = [steps ? words.steps(steps) : "", words.took(now - run.began)].filter(Boolean).join(" ");
      if (lane.meta.textContent !== said) lane.meta.textContent = said;
    }
  }

  /** A lane's words: what its agent is, what it was asked, how it stands. */
  private words(lane: Lane, runs: readonly AgentRun[]): void {
    const { run } = lane;
    const { words } = this.options;
    const kind = lane.element.querySelector<HTMLElement>(".lane-kind")!;
    const kindSaid = run.name ?? run.kind ?? "";
    if (kind.textContent !== kindSaid) kind.textContent = kindSaid;
    kind.hidden = !kindSaid;
    const task = lane.element.querySelector<HTMLElement>(".lane-task")!;
    const taskSaid = run.task ?? run.name ?? run.kind ?? run.id;
    if (task.textContent !== taskSaid) task.textContent = taskSaid;
    const button = lane.element.querySelector<HTMLElement>(".lane-what")!;
    button.title = `${words.agents.goTo}: ${run.prompt ?? taskSaid}`;
    lane.element.style.setProperty("--depth", String(depthOf(run, runs)));
    if (run.going) return;
    const steps = run.tools ?? run.steps.length;
    const took = run.duration ?? (run.ended !== null && run.began !== null ? run.ended - run.began : null);
    const said =
      run.state === "failed"
        ? words.agents.failed
        : run.state === "stopped"
          ? words.agents.stopped
          : [steps ? words.steps(steps) : "", took !== null ? words.took(took) : ""].filter(Boolean).join(" ");
    if (lane.meta.textContent !== said) lane.meta.textContent = said;
  }

  private startClock(): void {
    if (this.timer) return;
    this.timer = window.setInterval(() => {
      if (!this.popover.open || !this.pop.isConnected) this.stopClock();
      else this.lay();
    }, 1000);
  }

  private stopClock(): void {
    clearInterval(this.timer);
    this.timer = 0;
  }
}

/** When an agent began and when it ended, or now while it works. */
function spanOf(run: AgentRun, now: number): [number, number] {
  const began = run.began ?? now;
  const end = run.going ? now : (run.ended ?? run.steps.at(-1)?.ended?.at ?? began);
  return [began, Math.max(began, end)];
}

/** Agents that worked at the same time, or less than an idle minute apart,
 *  each group on its own clock, the earliest first. */
function groupsOf(runs: readonly AgentRun[], now: number): { runs: AgentRun[]; from: number; to: number; going: boolean }[] {
  const spans = runs.map((run) => ({ run, span: spanOf(run, now) })).sort((a, b) => a.span[0] - b.span[0]);
  const groups: { runs: AgentRun[]; from: number; to: number; going: boolean }[] = [];
  for (const { run, span } of spans) {
    const last = groups.at(-1);
    if (last && span[0] - last.to <= APART) {
      last.runs.push(run);
      last.to = Math.max(last.to, span[1]);
      last.going ||= run.going;
    } else groups.push({ runs: [run], from: span[0], to: span[1], going: run.going });
  }
  // A clock of nothing yet is a second long, so a first lane has length.
  for (const group of groups) if (group.to - group.from < 1000) group.to = group.from + 1000;
  return groups;
}

/** A finished agent's check, or the cross of one that failed or was
 *  stopped. */
function endMark(run: AgentRun): string {
  const random = seeded(`mark:${run.id}`);
  const stroke = (points: Point[], width: number) => `<path d="${outline(points, width, random, { pressure: 0.2, taper: 0.15 })}"/>`;
  if (run.state === "done") {
    return stroke(
      [
        [-3.5, 0.5],
        [-1.2, 3],
        [4, -4],
      ],
      2,
    );
  }
  return (
    stroke(
      [
        [-3.2, -3.2],
        [3.2, 3.2],
      ],
      1.9,
    ) +
    stroke(
      [
        [3.2, -3.2],
        [-3.2, 3.2],
      ],
      1.9,
    )
  );
}

/** Agents in the order they began, each one another started right under
 *  that one. */
function ordered(runs: readonly AgentRun[]): AgentRun[] {
  const ids = new Set(runs.map((run) => run.id));
  const under = new Map<string, AgentRun[]>();
  const roots: AgentRun[] = [];
  const began = (a: AgentRun, b: AgentRun) => (a.began ?? 0) - (b.began ?? 0);
  for (const run of runs) {
    if (run.parent && run.parent !== run.id && ids.has(run.parent)) {
      const children = under.get(run.parent) ?? [];
      children.push(run);
      under.set(run.parent, children);
    } else roots.push(run);
  }
  const out: AgentRun[] = [];
  const seen = new Set<string>();
  const walk = (run: AgentRun) => {
    if (seen.has(run.id)) return;
    seen.add(run.id);
    out.push(run);
    for (const child of (under.get(run.id) ?? []).sort(began)) walk(child);
  };
  for (const run of roots.sort(began)) walk(run);
  // Agents whose parents start each other are shown all the same.
  for (const run of runs) walk(run);
  return out;
}

/** How many agents above an agent started it, one under another. */
function depthOf(run: AgentRun, runs: readonly AgentRun[]): number {
  let depth = 0;
  let parent = run.parent;
  const seen = new Set([run.id]);
  while (parent && !seen.has(parent) && depth < DEEPEST) {
    seen.add(parent);
    const found = runs.find((one) => one.id === parent);
    if (!found) break;
    depth += 1;
    parent = found.parent;
  }
  return depth;
}

function clockTime(at: number, english: boolean): string {
  return new Date(at).toLocaleTimeString(english ? "en" : "zh-CN", { hour: "2-digit", minute: "2-digit" });
}

const round = (n: number) => Math.round(n * 10) / 10;
