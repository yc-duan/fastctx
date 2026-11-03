// The side's first page (`world-gui.md`, 对话的侧面板): the conversation in
// focus at a glance, so the most is seen for the least done. A card for each
// of its parts with anything to say, each folding away under its name: the
// goal, what it is and how it stands; the plan, read whole in its card,
// which takes what height the others leave; the colleagues it asked and the
// host's agents it sent, those at work first, a short line each; its
// history, the latest written. Pressing what a card shows opens it: the goal
// and the history as pages of the side, a colleague beside the
// conversation, one of the host's agents, or everyone who works with it, in
// a sheet.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { PlanShape } from "../plan/shape";
import type { HistoryCatalog } from "../world/history";
import { anyGoing, going, order, shortLine, workedFor } from "./collab-page";
import type { SidePage } from "./pane";
import type { CollabNode, GoalShown } from "./types";
import type { SideWords } from "./words";

/** The lines a card of those who work with the conversation shows, before
 *  the rest are left to the sheet. */
const SHORT = 4;
/** The documents the history's card names. */
const LATEST = 3;
/** Where this device keeps the cards folded away. */
const FOLDED = "side-home-folded";

export interface HomeSeams {
  /** One of the side's pages: the goal's, the history. */
  page(part: "goal" | "history"): void;
  /** One who works with the conversation: a colleague beside it, one of the
   *  host's agents read in a sheet. */
  node(node: CollabNode): void;
  /** Everyone who works with it, in a sheet. */
  team(): void;
  /** Where a document of the history is read. */
  link(name: string): string;
}

type Card = "goal" | "plan" | "colleagues" | "agents" | "history";

function folded(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(FOLDED) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

function keepFolded(cards: Set<string>): void {
  try {
    localStorage.setItem(FOLDED, JSON.stringify([...cards]));
  } catch {
    // Folding is this device's convenience; without storage it lasts the page.
  }
}

export class SideHome implements SidePage {
  readonly element: HTMLElement;
  private goalShown: GoalShown | null = null;
  /** The goal stands on its bar above the input. */
  private barred = false;
  private planShown: PlanShape | null = null;
  private planned = false;
  private nodes: CollabNode[] = [];
  private catalog: HistoryCatalog | null = null;
  private readonly fold = folded();
  /** Those on short lines, in the order their times stand on the page. */
  private lined: CollabNode[] = [];
  private timer: number | null = null;
  private seen = false;

  /** `plan` is the plan's own page, drawn in its card. */
  constructor(
    private readonly words: SideWords,
    private readonly seams: HomeSeams,
    private readonly goals: boolean,
    private readonly planPage: HTMLElement,
  ) {
    this.element = document.createElement("section");
    this.element.className = "sh-page";
    this.element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const head = target.closest<HTMLElement>(".sh-head");
      if (head) {
        const card = head.dataset.card!;
        const shut = !this.fold.has(card);
        if (shut) this.fold.add(card);
        else this.fold.delete(card);
        keepFolded(this.fold);
        // Folded where it stands, so it eases shut or open as a
        // conversation's steps do; the next drawing finds it as it is.
        const section = head.closest<HTMLElement>(".sh-card");
        section?.classList.toggle("shut", shut);
        section?.querySelector(":scope > .sh-fold")?.classList.toggle("open", !shut);
        head.setAttribute("aria-expanded", String(!shut));
        return;
      }
      const open = target.closest<HTMLElement>("[data-open]")?.dataset.open;
      if (open === "team") return this.seams.team();
      if (open === "goal" || open === "history") return this.seams.page(open);
      const id = target.closest<HTMLElement>("[data-id]")?.dataset.id;
      const node = id ? this.find(this.nodes, id) : null;
      if (node) this.seams.node(node);
    });
    // The times of those at work move only while the page is seen.
    new IntersectionObserver((entries) => {
      this.seen = entries.some((entry) => entry.isIntersecting);
      this.tick();
    }).observe(this.element);
    this.draw();
  }

  /** `barred`: the goal stands on its bar above the input. */
  goal(goal: GoalShown | null, barred: boolean): void {
    this.goalShown = goal;
    this.barred = barred;
    this.draw();
  }

  plan(source: string | null, shape: PlanShape | null): void {
    // A plan takes whatever form its work asks for: any text is a plan.
    this.planned = !!source?.trim();
    this.planShown = shape;
    this.draw();
  }

  team(nodes: CollabNode[]): void {
    this.nodes = nodes;
    this.draw();
  }

  history(catalog: HistoryCatalog | null): void {
    this.catalog = catalog;
    this.draw();
  }

  private draw(): void {
    const now = Date.now();
    this.lined = [];
    const ordered = order(this.nodes, now);
    const cards = [
      this.goalCard(now),
      this.planCard(),
      this.teamCard("colleagues", ordered.filter((node) => node.kind === "colleague" || node.kind === "side"), now),
      this.teamCard("agents", ordered.filter((node) => node.kind === "subagent"), now),
      this.historyCard(),
    ].filter(Boolean);
    // Said where no card stands, the goal's left to its bar among them.
    this.element.innerHTML = `${cards.join("")}<p class="sh-none">${escape(this.words.home.none)}</p>`;
    // The plan's page is one element, kept as it is drawn anew around it.
    this.element.querySelector(".sh-plan")?.append(this.planPage);
    this.tick();
  }

  /** A card under its name, folding away when the name is pressed; how it
   *  stands after the name, what waits on the user in clay. `barred`: the
   *  goal's bar above the input says it, so it stands only where the side
   *  lies over the conversation. */
  private card(card: Card, name: string, state: string, body: string, waiting = "", waits = false, barred = false): string {
    const shut = this.fold.has(card);
    const said = `${escape(state)}${waiting ? `<span class="sh-waits">${escape(waiting)}</span>` : ""}`;
    return `<section class="sh-card${shut ? " shut" : ""}${barred ? " barred" : ""}" data-of="${card}"><button class="sh-head" type="button" data-card="${card}" aria-expanded="${!shut}"><span class="sh-name">${escape(name)}</span><span class="sh-turn">${glyph("chevron", 12)}</span><span class="sh-state${waits ? " waiting" : ""}">${said}</span></button><div class="sh-fold${shut ? "" : " open"}"><div class="sh-fold-inner"><div class="sh-body">${body}</div></div></div></section>`;
  }

  private goalCard(now: number): string {
    const goal = this.goalShown;
    if (!this.goals || !goal) return "";
    const words = this.words.goal;
    const worked = goal.worked + (goal.status === "active" ? Math.max(0, now - goal.since) : 0);
    const state = `${words.states[goal.status]}　${words.worked(worked)}`;
    const body = `<button class="sh-open sh-clamp" type="button" data-open="goal">${escape(goal.objective)}</button>`;
    return this.card("goal", this.words.pages.goal, state, body, "", goal.status === "blocked", this.barred);
  }

  /** The plan read whole in its card: how far it is after its name, in
   *  clay while it asks the user. */
  private planCard(): string {
    if (!this.planned) return "";
    const words = this.words;
    const shape = this.planShown;
    let count = "";
    const here = shape?.stops.findIndex((stop) => stop.where === "here") ?? -1;
    const walked = shape?.stops.filter((stop) => stop.where === "walked").length ?? 0;
    if (shape && (here >= 0 || walked > 0)) count = words.planCount((here >= 0 ? here : walked - 1) + 1, shape.stops.length);
    else if (shape?.checks.length) count = words.planCount(shape.checks.filter((check) => check.done).length, shape.checks.length);
    const asks = shape?.asks.length ? words.collab.states.waiting : "";
    return this.card("plan", words.pages.plan, count, `<div class="sh-plan"></div>`, asks);
  }

  /** The colleagues, or the host's agents: those at work first, a short line
   *  each; the rest, and each colleague's own, in the sheet. */
  private teamCard(card: "colleagues" | "agents", nodes: CollabNode[], now: number): string {
    if (nodes.length === 0) return "";
    const words = this.words;
    const working = nodes.filter((node) => going(node.state)).length;
    const waiting = nodes.some((node) => node.state === "waiting");
    const shown = nodes.slice(0, SHORT);
    for (const node of shown) this.lined.push(node);
    const lines = shown.map((node) => shortLine(node, now, words.collab, words.goal)).join("");
    const more = nodes.length > SHORT || nodes.some((node) => node.children.length > 0);
    const all = more ? `<button class="text-button sh-all" type="button" data-open="team">${escape(words.home.all(nodes.length))}</button>` : "";
    const name = card === "colleagues" ? words.collab.groups.colleagues : words.collab.groups.agents;
    const state = working ? words.collab.atWork(working) : "";
    return this.card(card, name, state, `<div class="sh-lines">${lines}</div>${all}`, waiting ? words.collab.states.waiting : "");
  }

  private historyCard(): string {
    const documents = [...(this.catalog?.documents ?? [])].filter((one) => !one.retired).slice(0, LATEST);
    if (!this.catalog) return "";
    const words = this.words;
    const lines = documents.length
      ? documents.map((one) => `<a class="sh-doc" href="${escape(this.seams.link(one.name))}">${glyph("history", 16)}<span>${escape(one.name)}</span></a>`).join("")
      : "";
    const all = `<button class="text-button sh-all" type="button" data-open="history">${escape(words.home.allHistory)}</button>`;
    return this.card("history", words.pages.history, "", lines + all);
  }

  /** Each second while the page is seen and one works: their times. */
  private tick(): void {
    const live = this.seen && this.lined.some((node) => going(node.state) || anyGoing(node));
    if (live && this.timer === null) this.timer = window.setInterval(() => this.second(), 1000);
    if (!live && this.timer !== null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
  }

  private second(): void {
    const now = Date.now();
    const cells = this.element.querySelectorAll<HTMLElement>(".cp-line .cp-took");
    this.lined.forEach((node, index) => {
      if (!going(node.state)) return;
      const took = workedFor(node, now);
      const cell = cells[index];
      if (cell && took !== null) cell.textContent = this.words.collab.took(took);
    });
  }

  private find(list: CollabNode[], id: string): CollabNode | null {
    for (const node of list) {
      if (node.id === id) return node;
      const found = this.find(node.children, id);
      if (found) return found;
    }
    return null;
  }
}
