// Who works with a conversation (`world-gui.md`, 协作), in groups under their
// names: the colleagues it asked, a card each, holding under a ruled line the
// agents that colleague sent, a short line each; the host's own agents it
// sent, a card each; the conversations branched off it. A card says how it stands at its
// head, before its task in its own words: a breathing dot while it works, a
// clay one while it waits on the user, a tick once it handed back, a cross
// where it failed, a stroke where it was stopped. Under the task, how long it
// has worked, to the second while it works, and what it does this moment;
// once ended, what it spent. Those at work come first. Those that ended
// before the work now going on began, and long before now, fold into one
// line. The rest is said when pointed at. Pressing one opens it: a colleague
// beside the conversation, one of the host's agents in the side.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { ruled, seeded } from "../pen/shapes";
import type { SidePage } from "./pane";
import type { CollabNode, NodeState } from "./types";
import type { CollabWords, GoalWords } from "./words";

/** How long ago one ended and still stands among the rest. */
const RECENT = 30 * 60_000;

export const going = (state: NodeState) => state === "working" || state === "waiting";
export const anyGoing = (node: CollabNode): boolean => going(node.state) || node.children.some(anyGoing);

/** When it or anyone under it was last at work: now while one still is. */
function lastAt(node: CollabNode, now: number): number {
  if (anyGoing(node)) return now;
  return Math.max(node.ended ?? node.began ?? 0, ...node.children.map((child) => lastAt(child, now)));
}

/** Those at work first, the longest at it on top; then those that ended,
 *  the latest first; each one's own in the same order. */
export function order(nodes: CollabNode[], now: number): CollabNode[] {
  return [...nodes]
    .sort((a, b) => {
      const working = Number(anyGoing(b)) - Number(anyGoing(a));
      if (working) return working;
      return anyGoing(a) ? (a.began ?? now) - (b.began ?? now) : lastAt(b, now) - lastAt(a, now);
    })
    .map((node) => ({ ...node, children: order(node.children, now) }));
}

/** The line ruled between a colleague and the agents it sent, stretched to
 *  the card's width. */
function rule(seed: string): string {
  return `<svg class="cp-rule" viewBox="0 0 360 6" preserveAspectRatio="none" aria-hidden="true"><path d="${ruled([2, 3], [358, 3], 0.6, seeded(`collab-rule:${seed}`))}"/></svg>`;
}

export class CollabPage implements SidePage {
  readonly element: HTMLElement;
  private nodes: CollabNode[] = [];
  /** Those drawn, in the order their times stand on the page. */
  private drawn: CollabNode[] = [];
  private unfolded = false;
  private timer: number | null = null;
  private seen = false;

  constructor(
    private readonly words: CollabWords,
    private readonly goals: GoalWords,
    private readonly pressed: (node: CollabNode) => void,
  ) {
    this.element = document.createElement("section");
    this.element.className = "cp-page";
    this.element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest(".cp-earlier")) {
        this.unfolded = !this.unfolded;
        this.draw();
        return;
      }
      const id = target.closest<HTMLElement>("[data-id]")?.dataset.id;
      const node = id ? this.find(this.nodes, id) : null;
      if (node) this.pressed(node);
    });
    // The times of those at work move only while the page is seen.
    new IntersectionObserver((entries) => {
      this.seen = entries.some((entry) => entry.isIntersecting);
      this.tick();
    }).observe(this.element);
  }

  show(nodes: CollabNode[]): void {
    this.nodes = nodes;
    this.draw();
  }

  private draw(): void {
    const now = Date.now();
    const ordered = order(this.nodes, now);
    // What still goes began the work at hand: one that ended within it, or
    // lately, stands; while none goes, the work at hand is the latest.
    const latest = Math.max(0, ...ordered.map((node) => lastAt(node, now)));
    const starts = ordered.filter(anyGoing).map((node) => node.began ?? now);
    const since = Math.min(latest - RECENT, ...starts);
    const recent = (node: CollabNode) => anyGoing(node) || lastAt(node, now) >= since;
    this.drawn = [];
    const groups = this.words.groups;
    const kinds: [string, CollabNode["kind"]][] = [
      [groups.colleagues, "colleague"],
      [groups.agents, "subagent"],
      [groups.side, "side"],
    ];
    const drawn = kinds.map(([name, kind]) => {
      const all = ordered.filter((node) => node.kind === kind);
      const kept = this.unfolded ? all : all.filter(recent);
      const earlier = all.length - all.filter(recent).length;
      if (all.length === 0) return "";
      const fold = earlier ? `<button class="cp-earlier" type="button">${escape(this.unfolded ? this.words.fold : this.words.earlier(earlier))}</button>` : "";
      return `<section class="cp-group"><h3 class="cp-group-name">${escape(name)}</h3>${kept.map((node) => this.card(node, now)).join("")}${fold}</section>`;
    });
    const empty = this.nodes.length === 0 ? `<p class="cp-empty">${escape(this.words.empty)}</p>` : "";
    this.element.innerHTML = drawn.join("") + empty;
    this.tick();
  }

  /** One of the host's agents alone, or a colleague with the agents it
   *  sent: under a ruled line, how many, and each on a short line. */
  private card(node: CollabNode, now: number): string {
    const sent: CollabNode[] = [];
    const walk = (list: CollabNode[]) => {
      for (const child of list) {
        sent.push(child);
        walk(child.children);
      }
    };
    walk(node.children);
    const head = this.entry(node, now, sent.length > 0);
    if (sent.length === 0) return `<div class="cp-card">${head}</div>`;
    const lines = sent.map((child) => this.sentLine(child, now)).join("");
    return `<div class="cp-card">${head}${rule(node.id)}<span class="cp-sent">${escape(this.words.sent(sent.length))}</span><div class="cp-sents">${lines}</div></div>`;
  }

  /** One of the agents a colleague sent: how it stands, its task, how long. */
  private sentLine(node: CollabNode, now: number): string {
    this.drawn.push(node);
    return shortLine(node, now, this.words, this.goals);
  }

  /** One line of a card: how it stands, its task, and under the task how
   *  long it worked and what it does, or what it spent. The one that sent
   *  the rest of its card heads it, its task in the heavier weight. */
  private entry(node: CollabNode, now: number, heads: boolean): string {
    const words = this.words;
    this.drawn.push(node);
    const took = this.took(node, now);
    // A state no mark tells plainly is said: waiting on the user, stopped;
    // a colleague's goal says why it stopped, where it did.
    const stated = node.goal === "blocked" || node.goal === "paused";
    const state = node.state === "waiting" || (node.state === "stopped" && !stated) ? `<span class="cp-said ${node.state}">${escape(words.states[node.state])}</span>` : "";
    const meta = [
      node.kind === "colleague" ? `<span>${escape(node.name)}</span>` : "",
      state,
      going(node.state) && node.doing ? `<span class="cp-doing">${escape(node.doing)}</span>` : "",
      !going(node.state) && node.tokens ? `<span>${escape(words.tokens(node.tokens))}</span>` : "",
      // A colleague's goal says itself only where it stopped short of done.
      node.goal === "blocked" || node.goal === "paused" ? `<span class="cp-goal ${node.goal}">${escape(words.goal[node.goal])}</span>` : "",
    ].join("");
    const under = meta ? `<span class="cp-meta">${meta}</span>` : "";
    return `<button class="cp-entry ${node.state}${heads ? " heads" : ""}" type="button" data-id="${escape(node.id)}" title="${escape(pointedAt(node, took, now, words, this.goals))}"><span class="cp-state" role="img" aria-label="${escape(words.states[node.state])}">${stateMark(node.state)}</span><span class="cp-task">${escape(node.task || node.name)}</span><span class="cp-took">${took === null ? "" : escape(words.took(took))}</span>${under}</button>`;
  }

  private took(node: CollabNode, now: number): number | null {
    return workedFor(node, now);
  }

  /** Each second while the page is seen and one works: their times. */
  private tick(): void {
    const live = this.seen && this.drawn.some((node) => going(node.state));
    if (live && this.timer === null) this.timer = window.setInterval(() => this.second(), 1000);
    if (!live && this.timer !== null) {
      window.clearInterval(this.timer);
      this.timer = null;
    }
  }

  private second(): void {
    const now = Date.now();
    const cells = this.element.querySelectorAll<HTMLElement>(".cp-took");
    this.drawn.forEach((node, index) => {
      if (!going(node.state)) return;
      const took = this.took(node, now);
      const cell = cells[index];
      if (cell && took !== null) cell.textContent = this.words.took(took);
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

/** How it stands, drawn before its task: a breathing dot while it works, a
 *  clay one while it waits on the user, a tick, a cross, a stroke. */
export function stateMark(state: NodeState): string {
  if (state === "working") return '<span class="cp-pulse"></span>';
  if (state === "waiting") return '<span class="cp-dot"></span>';
  if (state === "done") return glyph("check", 14);
  if (state === "failed") return glyph("cross", 14);
  if (state === "stopped") return '<span class="cp-stopped"></span>';
  return '<span class="cp-dot faint"></span>';
}

/** How long one worked: to now while it works. */
export function workedFor(node: CollabNode, now: number): number | null {
  if (typeof node.began === "number") {
    if (typeof node.ended === "number") return Math.max(0, node.ended - node.began);
    if (going(node.state)) return Math.max(0, now - node.began);
  }
  return node.took > 0 ? node.took : null;
}

/** What pointing at one says: who it is, how it stands, and the rest. */
export function pointedAt(node: CollabNode, took: number | null, now: number, words: CollabWords, goals: GoalWords): string {
  const facts = [
    words.states[node.state],
    took === null ? "" : words.took(took),
    node.steps ? words.steps(node.steps) : "",
    node.tokens ? words.tokens(node.tokens) : "",
  ];
  // A colleague goes by its model's name already.
  const model = node.kind === "colleague" ? "" : (node.model ?? "");
  const began = typeof node.began === "number" ? words.began(goals.at(node.began, now)) : "";
  return [node.name, model, facts.filter(Boolean).join(words.separator), began, node.machine ?? "", node.why ?? "", node.task].filter(Boolean).join("\n");
}

/** One that works with the conversation on a short line: how it stands,
 *  its task, how long; said in full when pointed at. */
export function shortLine(node: CollabNode, now: number, words: CollabWords, goals: GoalWords): string {
  const took = workedFor(node, now);
  // One waiting on the user says so before its time.
  const waits = node.state === "waiting" ? `<span class="cp-said waiting">${escape(words.states.waiting)}</span>` : "";
  return `<button class="cp-line ${node.state}" type="button" data-id="${escape(node.id)}" title="${escape(pointedAt(node, took, now, words, goals))}"><span class="cp-state" role="img" aria-label="${escape(words.states[node.state])}">${stateMark(node.state)}</span><span class="cp-task">${escape(node.task || node.name)}</span><span class="cp-end">${waits}<span class="cp-took">${took === null ? "" : escape(words.took(took))}</span></span></button>`;
}
