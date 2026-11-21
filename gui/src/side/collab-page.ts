// Who works with a conversation (`world-gui.md`, 协作), on a card of the
// side's overview: the colleagues it asked and the conversations branched
// off it on one, the host's own agents it sent on another. Every one of
// them is there, a row each, the page scrolling in its card past a height:
// none is left out to be asked for (the user, 2026-11-17: "全部搞成能滚动
// 的，就没有展开逻辑了"). They are one level: what a colleague sent of its own
// is read in that colleague's conversation ("他只有一个层级"). A row says how
// it stands at its head, before its task in its own words: a breathing dot
// while it works, a clay one while it waits on the user, a tick once it
// handed back, a cross where it failed, a stroke where it was stopped. Under
// the task, who it is, what it does this moment while it works, what it
// spent once ended; at the row's end, how long it has worked, to the second
// while it works. Those at work come first. The rest is said when pointed
// at. Pressing one opens it: a colleague beside the conversation, one of the
// host's agents at the row of the conversation that sent it.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { SidePage } from "./pane";
import type { CollabNode, NodeState } from "./types";
import type { CollabWords, GoalWords } from "./words";

export const going = (state: NodeState) => state === "working" || state === "waiting";

/** When it was last at work: now while it still is. */
function lastAt(node: CollabNode, now: number): number {
  return going(node.state) ? now : (node.ended ?? node.began ?? 0);
}

/** Those at work first, the longest at it on top; then those that ended,
 *  the latest first; any two alike by when they began, the later first, and
 *  then by who they are — the same rows in the same order however often,
 *  and from whichever source, they come again. */
function order(nodes: CollabNode[], now: number): CollabNode[] {
  return [...nodes].sort((a, b) => {
    const working = Number(going(b.state)) - Number(going(a.state));
    if (working) return working;
    const by = going(a.state) ? (a.began ?? now) - (b.began ?? now) : lastAt(b, now) - lastAt(a, now);
    if (by) return by;
    const began = (b.began ?? 0) - (a.began ?? 0);
    if (began) return began;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}

export class CollabPage implements SidePage {
  readonly element: HTMLElement;
  private nodes: CollabNode[] = [];
  /** Those drawn, in the order their times stand on the page. */
  private drawn: CollabNode[] = [];
  private written = "";
  private timer: number | null = null;
  private seen = false;

  /** `kinds`: which of those that work with the conversation it shows. */
  constructor(
    private readonly words: CollabWords,
    private readonly goals: GoalWords,
    private readonly pressed: (node: CollabNode) => void,
    private readonly kinds: readonly CollabNode["kind"][],
  ) {
    this.element = document.createElement("section");
    this.element.className = "cp-page";
    this.element.addEventListener("click", (event) => {
      const id = (event.target as HTMLElement).closest<HTMLElement>("[data-id]")?.dataset.id;
      const node = id ? this.nodes.find((one) => one.id === id) : null;
      if (node) this.pressed(node);
    });
    // The times of those at work move only while the page is seen.
    new IntersectionObserver((entries) => {
      this.seen = entries.some((entry) => entry.isIntersecting);
      this.tick();
    }).observe(this.element);
  }

  show(nodes: CollabNode[]): void {
    this.nodes = nodes.filter((node) => this.kinds.includes(node.kind));
    this.draw();
  }

  private draw(): void {
    const now = Date.now();
    this.drawn = order(this.nodes, now);
    const html = this.drawn.length ? this.drawn.map((node) => this.entry(node, now)).join("") : `<p class="cp-empty">${escape(this.words.empty)}</p>`;
    // Written only where it changed, so a row pointed at or scrolled to
    // stays under the hand.
    if (html !== this.written) {
      this.written = html;
      this.element.innerHTML = html;
    }
    this.tick();
  }

  /** One row: how it stands, its task, how long it worked; and under the
   *  task who it is and what it does, or what it spent. */
  private entry(node: CollabNode, now: number): string {
    const words = this.words;
    const took = workedFor(node, now);
    // A state no mark tells plainly is said: waiting on the user, stopped;
    // a colleague's goal says why it stopped, where it did.
    const stated = node.goal === "blocked" || node.goal === "paused";
    const state = node.state === "waiting" || (node.state === "stopped" && !stated) ? `<span class="cp-said ${node.state}">${escape(words.states[node.state])}</span>` : "";
    const meta = [
      // A colleague and a side conversation go by their model's name; one
      // of the host's agents by its task alone.
      node.kind === "subagent" ? "" : `<span>${escape(node.name)}</span>`,
      state,
      going(node.state) && node.doing ? `<span class="cp-doing">${escape(node.doing)}</span>` : "",
      !going(node.state) && node.tokens ? `<span>${escape(words.tokens(node.tokens))}</span>` : "",
      // A colleague's goal says itself only where it stopped short of done.
      node.goal === "blocked" || node.goal === "paused" ? `<span class="cp-goal ${node.goal}">${escape(words.goal[node.goal])}</span>` : "",
    ].join("");
    const under = meta ? `<span class="cp-meta">${meta}</span>` : "";
    return `<button class="cp-entry ${node.state}" type="button" data-id="${escape(node.id)}" title="${escape(pointedAt(node, took, now, words, this.goals))}"><span class="cp-state" role="img" aria-label="${escape(words.states[node.state])}">${stateMark(node.state)}</span><span class="cp-task">${escape(node.task || node.name)}</span><span class="cp-took">${took === null ? "" : escape(words.took(took))}</span>${under}</button>`;
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
      const took = workedFor(node, now);
      const cell = cells[index];
      if (cell && took !== null) cell.textContent = this.words.took(took);
    });
  }
}

/** How it stands, drawn before its task: a breathing dot while it works, a
 *  clay one while it waits on the user, a tick, a cross, a stroke. */
function stateMark(state: NodeState): string {
  if (state === "working") return '<span class="cp-pulse"></span>';
  if (state === "waiting") return '<span class="cp-dot"></span>';
  if (state === "done") return glyph("check", 14);
  if (state === "failed") return glyph("cross", 14);
  if (state === "stopped") return '<span class="cp-stopped"></span>';
  return '<span class="cp-dot faint"></span>';
}

/** How long one worked: to now while it works. */
function workedFor(node: CollabNode, now: number): number | null {
  if (typeof node.began === "number") {
    if (typeof node.ended === "number") return Math.max(0, node.ended - node.began);
    if (going(node.state)) return Math.max(0, now - node.began);
  }
  return node.took > 0 ? node.took : null;
}

/** What pointing at one says: who it is, how it stands, and the rest. */
function pointedAt(node: CollabNode, took: number | null, now: number, words: CollabWords, goals: GoalWords): string {
  const facts = [words.states[node.state], took === null ? "" : words.took(took), node.steps ? words.steps(node.steps) : "", node.tokens ? words.tokens(node.tokens) : ""];
  // A colleague goes by its model's name already.
  const model = node.kind === "colleague" ? "" : (node.model ?? "");
  const began = typeof node.began === "number" ? words.began(goals.at(node.began, now)) : "";
  return [node.name, model, facts.filter(Boolean).join(words.separator), began, node.machine ?? "", node.why ?? "", node.task].filter(Boolean).join("\n");
}
