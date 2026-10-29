// What a conversation holds standing, the first tab of its side
// (`world-gui.md`, 对话的侧面板): a few small paper cards, each only what
// is read at a glance, so that the whole stands in one screen however long
// the plan or however many work with it. The goal, how it stands and how
// long it has been worked at, where its bar above the input does not say
// it already: none yet, one done, or the side over the whole page. The
// plan as its own page draws it, its stops
// strung on an ink line, but only the stop the work stands at with the one
// before and the one after, the line running on past them where the plan
// goes on; then the questions it waits on. Who works with the conversation,
// those at work first, a few at most, one line each. Under the cards, the
// machines and folders it is pointed at. A card with nothing in it is not
// drawn. Pressing the goal, the plan's whole or the whole team opens its
// page in place of the cards, under a way back.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Point } from "../pen/shapes";
import { Sketch } from "../pen/sketch";
import type { Stop } from "../plan/flow";
import type { PlanShown } from "../plan/panel";
import { planShape } from "../plan/shape";
import { stateWord, workedNow } from "./goal-bar";
import { goalMark } from "./marks";
import type { PaneContent, SidePage } from "./pane";
import type { CollabNode, GoalShown } from "./types";
import type { SideWords } from "./words";

type Part = "goal" | "plan" | "team";

/** The stops shown around the one the work stands at, the most of those
 *  who work shown in the team's card, and the most questions. */
const STOPS = 3;
const TEAM = 3;
const ASKS = 2;

/** The few stops the card shows: the one the work stands at, the one
 *  before it and the one after, kept whole at either end of the plan; and
 *  whether the plan goes on before and after them. */
function around(stops: Stop[]): { shown: Stop[]; before: boolean; after: boolean } {
  if (stops.length <= STOPS) return { shown: stops, before: false, after: false };
  const here = stops.findIndex((stop) => stop.where === "here");
  const walked = stops.filter((stop) => stop.where === "walked").length;
  const at = here >= 0 ? here : Math.min(walked, stops.length - 1);
  const start = Math.min(Math.max(0, at - 1), stops.length - STOPS);
  return { shown: stops.slice(start, start + STOPS), before: start > 0, after: start + STOPS < stops.length };
}

/** A checklist as the card shows it: the item ticked last before the first
 *  still open, and the next two open, as stops on the plan's ink line;
 *  how many are ticked; whether items come before and after those shown. */
function ticked(checks: { text: string; done: boolean }[]): { shown: Stop[]; before: boolean; after: boolean; done: number } {
  const done = checks.filter((check) => check.done).length;
  const open = checks.findIndex((check) => !check.done);
  const first = open < 0 ? Math.max(0, checks.length - STOPS) : Math.max(0, open - 1);
  const picked = checks.slice(first, first + STOPS);
  const shown = picked.map((check): Stop => ({ label: check.text, note: "", branch: false, maybe: false, where: check.done ? "walked" : "ahead" }));
  return { shown, before: first > 0, after: first + picked.length < checks.length, done };
}

/** Those who work with the conversation as one list, those at work first,
 *  each group in the tree's own order. */
function atWorkFirst(nodes: CollabNode[]): CollabNode[] {
  const all: CollabNode[] = [];
  const walk = (list: CollabNode[]) => {
    for (const node of list) {
      all.push(node);
      walk(node.children);
    }
  };
  walk(nodes);
  return [...all.filter((node) => node.state === "working"), ...all.filter((node) => node.state !== "working")];
}

export class Overview implements PaneContent {
  readonly element: HTMLElement;
  private readonly cards: HTMLElement;
  private readonly drill: HTMLElement;
  private readonly drillBody: HTMLElement;
  private opened: Part | null = null;
  private team: CollabNode[] = [];
  /** The plan's stops as last drawn, and the size their ink was drawn for. */
  private route: { before: boolean; after: boolean } = { before: false, after: false };
  private inked = "";

  constructor(
    private readonly words: SideWords,
    private readonly pages: { goal: SidePage | null; plan: SidePage; collab: SidePage; env: HTMLElement | null },
    private readonly pressed: (node: CollabNode) => void,
  ) {
    this.element = document.createElement("section");
    this.element.className = "ov-page";
    const card = (name: string, kind: string, count = false) =>
      `<section class="ov-card ov-${kind}" hidden><header class="ov-head"><span class="ov-name">${escape(name)}</span>${count ? '<span class="ov-count"></span>' : ""}</header><div class="ov-body"></div></section>`;
    this.element.innerHTML = `<div class="ov-cards">${pages.goal ? card(words.pages.goal, "goal") : ""}${card(words.pages.plan, "plan", true)}${card(words.pages.collab, "team", true)}<footer class="ov-foot"><div class="ov-env" hidden></div><p class="ov-hint">${glyph("panel", 16)}<span>${escape(words.dropHint)}</span></p></footer></div><div class="ov-drill" hidden><header class="ov-drill-head"><button class="ov-back" type="button">${glyph("back", 16)}<span class="ov-back-name"></span></button></header><div class="ov-drill-body"></div></div>`;
    this.cards = this.element.querySelector(".ov-cards")!;
    this.drill = this.element.querySelector(".ov-drill")!;
    this.drillBody = this.element.querySelector(".ov-drill-body")!;
    if (pages.env) {
      const env = this.element.querySelector<HTMLElement>(".ov-env")!;
      env.append(pages.env);
      env.hidden = false;
    }
    this.element.querySelector(".ov-back")!.addEventListener("click", () => this.back());
    this.cards.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const open = target.closest<HTMLElement>("[data-open]")?.dataset.open;
      if (open === "goal" || open === "plan" || open === "team") {
        this.open(open);
        return;
      }
      const id = target.closest<HTMLElement>(".ov-member[data-id]")?.dataset.id;
      const node = id ? this.team.find((one) => one.id === id) : null;
      if (node) this.pressed(node);
    });
    new ResizeObserver(() => this.ink()).observe(this.element.querySelector(".ov-plan .ov-body")!);
  }

  showGoal(goal: GoalShown | null): void {
    const card = this.element.querySelector<HTMLElement>(".ov-goal");
    if (!card) return;
    card.hidden = false;
    // A goal worked toward stands on its bar above the input; the card says
    // it again only where the side lies over the bar.
    card.toggleAttribute("data-on-bar", !!goal && goal.status !== "complete");
    const body = card.querySelector<HTMLElement>(".ov-body")!;
    const words = this.words.goal;
    if (!goal) {
      body.innerHTML = `<button class="ov-row ov-goal-none" type="button" data-open="goal"><span class="ov-mark">${glyph("goal", 16)}</span><span class="ov-words">${escape(words.start)}</span></button>`;
      return;
    }
    const now = Date.now();
    const when = goal.status === "paused" && goal.paused ? words.paused(goal.paused.by, goal.paused.until === null ? null : words.at(goal.paused.until, now)) : words.worked(workedNow(goal, now));
    body.innerHTML = `<button class="ov-row ov-goal-now" type="button" data-open="goal" data-status="${goal.status}" title="${escape(goal.objective)}"><span class="ov-mark"></span><span class="ov-lines"><span class="ov-words">${escape(goal.objective)}</span><span class="ov-meta"><span>${escape(stateWord(goal, words))}</span><span>${escape(when)}</span></span></span></button>`;
    body.querySelector(".ov-mark")!.innerHTML = goalMark(goal.status, `ov-${goal.version}-${goal.status}`, 16, "ov-goal-svg");
  }

  /** The checklist's last ticked item and the next two, or else the
   *  first stops of its flow; and the questions. */
  showPlan(plan: PlanShown | null): void {
    const card = this.element.querySelector<HTMLElement>(".ov-plan")!;
    const shape = plan?.source ? planShape(plan.source) : null;
    card.hidden = !shape || (shape.checks.length === 0 && shape.stops.length === 0 && shape.asks.length === 0 && !shape.title);
    if (!shape || card.hidden) {
      if (this.opened === "plan" && !shape) this.back();
      return;
    }
    const checked = shape.checks.length ? ticked(shape.checks) : null;
    card.querySelector(".ov-count")!.textContent = checked
      ? this.words.planCount(checked.done, shape.checks.length)
      : "";
    const { shown, before, after } = checked ?? around(shape.stops);
    this.route = { before, after };
    const stops = shown
      .map((stop) => `<li class="ov-stop" data-where="${stop.where || "ahead"}"${stop.maybe ? " data-maybe" : ""}><span class="ov-dot"></span><span class="ov-lines"><span class="ov-words">${escape(stop.label)}</span>${stop.note ? `<span class="ov-note">${escape(stop.note)}</span>` : ""}</span></li>`)
      .join("");
    const asks = shape.asks
      .slice(0, ASKS)
      .map((ask) => `<li><button class="ov-row ov-ask" type="button" data-open="plan"><span class="ov-mark">${glyph("ask", 16)}</span><span class="ov-words">${escape(ask)}</span></button></li>`)
      .join("");
    const title = shown.length === 0 && shape.title ? `<p class="ov-title">${escape(shape.title)}</p>` : "";
    card.querySelector(".ov-body")!.innerHTML = `${title}${stops ? `<div class="ov-route"${before ? " data-before" : ""}${after ? " data-after" : ""}><div class="ov-ink" aria-hidden="true"></div><ol class="ov-stops">${stops}</ol></div>` : ""}${asks ? `<ul class="ov-asks">${asks}</ul>` : ""}<button class="text-button ov-whole" type="button" data-open="plan">${escape(this.words.planOpen)}</button>`;
    this.inked = "";
    requestAnimationFrame(() => this.ink());
  }

  /** Who works with the conversation: its card shows while anyone does,
   *  those at work first, a few at most; the whole tree is a press away. */
  showTeam(nodes: CollabNode[]): void {
    const card = this.element.querySelector<HTMLElement>(".ov-team")!;
    const all = atWorkFirst(nodes);
    this.team = all.slice(0, TEAM);
    card.hidden = all.length === 0;
    card.querySelector(".ov-count")!.textContent = all.length ? String(all.length) : "";
    if (!all.length) {
      if (this.opened === "team") this.back();
      return;
    }
    const words = this.words.collab;
    const rows = this.team
      .map((node) => {
        const end =
          node.state === "working"
            ? '<span class="cp-pulse"></span>'
            : node.state === "done"
              ? glyph("check", 16)
              : node.state === "failed"
                ? glyph("cross", 16)
                : node.state === "stopped"
                  ? '<span class="cp-stopped"></span>'
                  : "";
        return `<button class="ov-member ${node.state}" type="button" data-id="${escape(node.id)}" title="${escape([node.name, words.states[node.state], node.task].filter(Boolean).join("\n"))}"><span class="cp-seal">${glyph(node.agent, 14)}</span><span class="ov-member-name">${escape(node.name)}</span><span class="ov-member-task">${escape(node.task)}</span><span class="cp-end" aria-label="${escape(words.states[node.state])}">${end}</span></button>`;
      })
      .join("");
    // The tree says who sent whom; it is offered where the card leaves that
    // out, or leaves out anyone.
    const whole = all.length > this.team.length || nodes.some((node) => node.children.length > 0);
    card.querySelector(".ov-body")!.innerHTML = `${rows}${whole ? `<button class="text-button ov-whole" type="button" data-open="team">${escape(this.words.teamOpen)}</button>` : ""}`;
  }

  /** Whether other conversations are open beside this one: the way to bring
   *  one in is said only until one is. */
  showTabs(any: boolean): void {
    this.element.querySelector<HTMLElement>(".ov-hint")!.hidden = any;
  }

  /** The goal's, the plan's or the team's page, in place of the cards. */
  open(part: Part): void {
    const page = part === "goal" ? this.pages.goal : part === "plan" ? this.pages.plan : this.pages.collab;
    if (!page) return;
    this.opened = part;
    this.drill.querySelector(".ov-back-name")!.textContent = part === "goal" ? this.words.pages.goal : part === "plan" ? this.words.pages.plan : this.words.pages.collab;
    this.drill.querySelector(".ov-back")!.setAttribute("aria-label", this.words.back);
    this.drillBody.replaceChildren(page.element);
    this.cards.hidden = true;
    this.drill.hidden = false;
    this.element.scrollTop = 0;
    page.shown?.();
  }

  back(): void {
    this.opened = null;
    this.drill.hidden = true;
    this.drillBody.replaceChildren();
    this.cards.hidden = false;
    this.inked = "";
    this.ink();
  }

  shown(): void {
    if (this.opened) (this.opened === "goal" ? this.pages.goal : this.opened === "plan" ? this.pages.plan : this.pages.collab)?.shown?.();
    else this.ink();
  }

  /** The plan's ink line through the stops shown, run on to the card's
   *  edge where the plan goes on, and each stop's mark: walked in the
   *  quieter ink, the one the work stands at circled in clay, those ahead
   *  small rings; a way not yet sure is dashed. Drawn when the stops' size
   *  is known. */
  private ink(): void {
    const route = this.element.querySelector<HTMLElement>(".ov-route");
    if (!route) return;
    const width = route.clientWidth;
    const height = route.clientHeight;
    const key = `${width}x${height}`;
    if (width === 0 || key === this.inked) return;
    this.inked = key;
    const box = route.getBoundingClientRect();
    const rows = [...route.querySelectorAll<HTMLElement>(".ov-stop")];
    const marks = rows.map((row) => {
      const dot = row.querySelector<HTMLElement>(".ov-dot")!.getBoundingClientRect();
      return { row, at: [dot.left - box.left + dot.width / 2, dot.top - box.top + dot.height / 2] as Point };
    });
    if (!marks.length) return;
    const sketch = new Sketch("ov-route");
    const x = marks[0]!.at[0];
    const line = (from: number, to: number, maybe: boolean) => {
      if (to - from < 2) return;
      if (maybe) sketch.dashed([x, from], [x, to], 1.2, "graphite", 3.5, 3.5);
      else sketch.rule([x, from], [x, to], 1.25, "graphite");
    };
    if (this.route.before) line(0, marks[0]!.at[1] - 7, false);
    marks.forEach((mark, at) => {
      const next = marks[at + 1];
      if (next) line(mark.at[1] + 7, next.at[1] - 7, next.row.hasAttribute("data-maybe"));
    });
    if (this.route.after) line(marks[marks.length - 1]!.at[1] + 7, height, false);
    for (const mark of marks) {
      const where = mark.row.dataset.where;
      if (where === "here") sketch.blot(mark.at, 3.4, "clay").ring(mark.at, 8.5, 1.4, "clay");
      else if (where === "walked") sketch.blot(mark.at, 3.1, "graphite");
      else sketch.ring(mark.at, 3.6, 1.15, "graphite");
    }
    route.querySelector<HTMLElement>(".ov-ink")!.innerHTML = sketch.svg(width, height, "ov-ink-svg");
  }
}
