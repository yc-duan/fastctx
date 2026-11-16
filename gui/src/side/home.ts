// The side's first page (`world-gui.md`, 对话的侧面板): the conversation in
// focus at a glance, so the most is seen for the least done. A card for each
// of its parts with anything to say, each folding away under its name: the
// goal, what it is and how it stands; the plan, read whole in its card,
// which takes what height the others leave; the colleagues it asked and the
// host's agents it sent, those at work first, a short line each. Pressing
// what a card shows opens it: the goal as a page of the side, a colleague
// beside the conversation, one of the host's agents, or everyone who works
// with it, in a sheet. The history is no card: what is kept to look things up
// in has its own mark in the desk's corner.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { figures, mark } from "../files/changed";
import { tally, type ChangedFile } from "../files/model";
import type { FilesWords } from "../files/words";
import type { PlanShape } from "../plan/shape";
import { anyGoing, going, order, shortLine, workedFor } from "./collab-page";
import type { SidePage } from "./pane";
import { ONLY, onBoards } from "./board";
import { CHOSEN, cardHidden, onCardsChosen, openCardsMenu, type Card } from "./shown";
import type { CollabNode, GoalShown } from "./types";
import type { SideWords } from "./words";

/** The lines a card of those who work with the conversation shows, before
 *  the rest are left to the sheet. */
const SHORT = 4;
/** Where this device keeps the cards folded away. */
const FOLDED = "side-home-folded";

export interface HomeSeams {
  /** The goal's page of the side. */
  page(part: "goal"): void;
  /** One who works with the conversation: a colleague beside it, one of the
   *  host's agents read in a sheet. */
  node(node: CollabNode): void;
  /** Everyone who works with it, in a sheet. */
  team(): void;
  /** The files it changed, as a page of the side; and one of them, opened
   *  to its changes. */
  files?(): void;
  file?(machine: string, path: string): void;
  /** Whether a card stands in a window of its own, and so not here. */
  away?(card: Card): boolean;
  /** Takes a card out into a window of its own, from where it stands on
   *  the screen; none where that cannot be. */
  pull?: ((card: Card, from: { left: number; top: number; width: number; height: number }) => void) | undefined;
}

/** The files the card lists, the last changed first. */
const FILES = 4;

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

/** Carries the plan's card from the height it stands at down to its name
 *  as it shuts. Its plan is taller than the room the card has, so folded by
 *  its rows alone the card stood at that height, emptying, until the rows
 *  had fallen below it, and only then came down: the plan gone from a card
 *  still standing. */
function shutFrom(card: HTMLElement, head: HTMLElement, from: number): void {
  const style = getComputedStyle(card);
  const token = style.getPropertyValue("--expand").trim();
  const duration = parseFloat(token) * (token.endsWith("ms") ? 1 : 1000) || 0;
  const to = head.getBoundingClientRect().height;
  if (duration <= 0 || from <= to + 1) return;
  card.animate([{ height: `${from}px` }, { height: `${to}px` }], { duration, easing: style.getPropertyValue("--expand-easing").trim() || "ease" });
}

export class SideHome implements SidePage {
  readonly element: HTMLElement;
  private goalShown: GoalShown | null = null;
  /** The goal stands on its bar above the input. */
  private barred = false;
  private planShown: PlanShape | null = null;
  private planned = false;
  private nodes: CollabNode[] = [];
  private changed: ChangedFile[] = [];
  private readonly fold = folded();
  /** Those on short lines, in the order their times stand on the page. */
  private lined: CollabNode[] = [];
  private timer: number | null = null;
  private seen = false;
  /** The cards on the page, by what they are of, each with what it said
   *  when it was set; what each says now, as it was last made; and the
   *  line said where no card stands. */
  private readonly standing = new Map<string, { sign: string; element: HTMLElement }>();
  private readonly signs = new Map<string, string>();
  private readonly none = document.createElement("p");

  /** `plan` is the plan's own page, drawn in its card. */
  constructor(
    private readonly words: SideWords,
    private readonly seams: HomeSeams,
    private readonly goals: boolean,
    private readonly planPage: HTMLElement,
    /** The words of the files it changed; none where they are not shown. */
    private readonly fileWords: FilesWords | null = null,
  ) {
    this.element = document.createElement("section");
    this.element.className = "sh-page";
    this.none.className = "sh-none";
    this.element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const head = target.closest<HTMLElement>(".sh-head");
      if (head) {
        // Alone in its window a card does not fold: there is nothing else
        // for it to make room for.
        if (ONLY) return;
        const card = head.dataset.card!;
        const shut = !this.fold.has(card);
        if (shut) this.fold.add(card);
        else this.fold.delete(card);
        keepFolded(this.fold);
        // Folded where it stands, so it eases shut or open as a
        // conversation's steps do; the next drawing finds it as it is.
        const section = head.closest<HTMLElement>(".sh-card");
        // How tall the plan's card stands as it is shut, before it changes.
        const from = card === "plan" && shut && section ? section.getBoundingClientRect().height : 0;
        section?.classList.toggle("shut", shut);
        section?.querySelector(":scope > .sh-fold")?.classList.toggle("open", !shut);
        head.setAttribute("aria-expanded", String(!shut));
        if (from && section) shutFrom(section, head, from);
        return;
      }
      const open = target.closest<HTMLElement>("[data-open]")?.dataset.open;
      if (open === "team") return this.seams.team();
      if (open === "goal") return this.seams.page(open);
      if (open === "files") return this.seams.files?.();
      const file = target.closest<HTMLElement>("[data-file]");
      if (file) return this.seams.file?.(file.dataset.machine!, file.dataset.file!);
      const id = target.closest<HTMLElement>("[data-id]")?.dataset.id;
      const node = id ? this.find(this.nodes, id) : null;
      if (node) this.seams.node(node);
    });
    // Pressed with the other button, the overview offers its cards to be
    // shown or put away (`shown.ts`); words chosen in it keep the system's
    // own menu, to be copied.
    this.element.addEventListener("contextmenu", (event) => {
      if (getSelection()?.toString() || (event.target as HTMLElement).closest("a, input, textarea")) return;
      event.preventDefault();
      if (ONLY) return;
      // On a card, the menu first offers to take that card out.
      const section = (event.target as HTMLElement).closest<HTMLElement>(".sh-card");
      const card = section?.dataset.of as Card | undefined;
      // Not where the side lies over the whole desk, as on a phone.
      const pull = this.element.closest(".sp-over") ? undefined : this.seams.pull;
      openCardsMenu(event.clientX, event.clientY, this.words, pull && section && card && card !== "goal" ? () => pull(card, section.getBoundingClientRect()) : undefined);
    });
    // One overview hears what is chosen for all of them, and is let go with
    // its conversation.
    const self = new WeakRef(this);
    const stop = onCardsChosen(() => {
      const home = self.deref();
      if (home) home.draw();
      else stop();
    });
    // A card taken out leaves the overview, and is back as its window goes.
    const still = onBoards(() => {
      const home = self.deref();
      if (home) home.draw();
      else still();
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

  /** The files the conversation changed, as they now stand. */
  files(files: ChangedFile[]): void {
    const sign = (all: ChangedFile[]) => all.map((file) => `${file.key}|${file.how}|${file.added}|${file.removed}|${file.changes[0]?.at ?? 0}`).join("\n");
    if (sign(files) === sign(this.changed)) return;
    this.changed = files;
    this.draw();
  }

  private draw(): void {
    const now = Date.now();
    this.lined = [];
    const ordered = order(this.nodes, now);
    // Each card the user has not put away, where it has anything to say.
    const made: Record<Exclude<Card, "goal">, () => string> = {
      plan: () => this.planCard(),
      files: () => this.filesCard(),
      colleagues: () => this.teamCard("colleagues", ordered.filter((node) => node.kind === "colleague" || node.kind === "side"), now),
      agents: () => this.teamCard("agents", ordered.filter((node) => node.kind === "subagent"), now),
    };
    // In a window of its own a card stands alone, put away or not.
    const chosen = CHOSEN.filter((card): card is Exclude<Card, "goal"> => card !== "goal" && (ONLY ? card === ONLY : !cardHidden(card) && !this.seams.away?.(card)));
    const cards = [ONLY ? "" : this.goalCard(now), ...chosen.map((card) => made[card]())].filter(Boolean);
    // Said where no card stands, the goal's left to its bar among them; and
    // where they were all put away, how they are brought back.
    const away = !ONLY && CHOSEN.some((card) => !cardHidden(card) && this.seams.away?.(card));
    const none = away ? this.words.home.away : chosen.length ? this.words.home.none : this.words.home.hidden;
    this.put(cards);
    if (this.none.textContent !== none) this.none.textContent = none;
    this.element.classList.toggle("sh-has-cards", cards.length > 0);
    this.element.classList.toggle("sh-has-unbarred", this.element.querySelector(".sh-card:not(.barred)") !== null);
    this.tick();
  }

  /** Puts the cards on the page, changing only what changed: a card whose
   *  words are what they were is left standing as it is, folded or open,
   *  mid-fold or scrolled; one whose words changed is set anew in its
   *  place. The whole page used to be written again at every change (a
   *  helper's minute passing among them), which took the plan's page off
   *  the page and put it back, and a page put back is scrolled to its top:
   *  the plan came back to its beginning under the reader. Where the plan's
   *  card itself must be set anew, its page is moved into the new card and
   *  left scrolled where it was. */
  private put(cards: string[]): void {
    const top = this.planPage.scrollTop;
    const wanted = new Map<string, string>();
    for (const html of cards) wanted.set(/ data-of="([a-z]+)"/.exec(html)?.[1] ?? html, html);
    for (const [key, card] of this.standing) {
      if (wanted.has(key)) continue;
      card.element.remove();
      this.standing.delete(key);
    }
    let previous: Element | null = null;
    for (const [key, html] of wanted) {
      const sign = this.signs.get(key) ?? html;
      let card = this.standing.get(key);
      if (!card || card.sign !== sign) {
        const made = document.createElement("template");
        made.innerHTML = html;
        const element = made.content.firstElementChild as HTMLElement;
        // The plan's page is one element, kept as its card is set anew.
        if (key === "plan") element.querySelector(".sh-plan")?.append(this.planPage);
        if (card) card.element.replaceWith(element);
        card = { sign, element };
        this.standing.set(key, card);
      }
      const place: Element | null = previous ? previous.nextElementSibling : this.element.firstElementChild;
      if (card.element !== place) this.element.insertBefore(card.element, place);
      previous = card.element;
    }
    if (this.element.lastElementChild !== this.none) this.element.append(this.none);
    if (this.planPage.scrollTop !== top) this.planPage.scrollTop = top;
  }

  /** A card under its name, folding away when the name is pressed; how it
   *  stands after the name, what waits on the user in clay. `barred`: the
   *  goal's bar above the input says it, so it stands only where the side
   *  lies over the conversation. */
  private card(card: Card, name: string, state: string, body: string, waiting = "", waits = false, barred = false): string {
    // What it says, apart from whether it is folded: folding is done where
    // the card stands, and is no reason to set it anew.
    this.signs.set(card, [name, state, body, waiting, waits, barred].join("\u0001"));
    const shut = !ONLY && this.fold.has(card);
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

  /** The files the conversation changed: how many and by how many lines
   *  after the name, the last changed a line each, the rest in their page. */
  private filesCard(): string {
    const words = this.fileWords;
    if (!words || !this.changed.length) return "";
    const sum = tally(this.changed);
    const latest = [...this.changed].sort((a, b) => (b.changes[0]?.at ?? 0) - (a.changes[0]?.at ?? 0)).slice(0, FILES);
    const lines = latest
      .map((file) => `<button class="sh-file" type="button" data-file="${escape(file.path)}" data-machine="${escape(file.machine)}" title="${escape(file.path)}">${mark(file.how, words.how[file.how])}<span class="sh-file-name">${escape(file.name)}</span>${figures(file.added, file.removed)}</button>`)
      .join("");
    const all = `<button class="text-button sh-all" type="button" data-open="files">${escape(words.side.all(sum.files))}</button>`;
    return this.card("files", words.side.name, words.files(sum.files), `<div class="sh-files">${lines}</div><div class="sh-files-foot">${all}${figures(sum.added, sum.removed)}</div>`);
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
