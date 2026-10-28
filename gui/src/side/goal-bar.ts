// A conversation's goal, while it has one, as a line above its input
// (`world-goal.md`, 界面上): the goal's pennant, how it stands, the user's
// words cut to what fits, how long it has been worked at, or why it waits
// and until when; then clearing it, pausing or resuming it, and opening it
// in the panel. Cleared, it says so with an undo for a few seconds before
// the World is told.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { GoalShown } from "./types";
import type { GoalWords } from "./words";

export interface GoalActs {
  pause(): void;
  resume(): void;
  clear(): void;
  /** The goal opened in the panel beside the conversation; absent where
   *  the page is itself in a panel. */
  expand?: (() => void) | undefined;
}

/** How long a cleared goal can be brought back before the World is told. */
const UNDO_MS = 6000;
/** How often the time worked is said again while it goes on. */
const TICK_MS = 30_000;

/** The page's word for how a goal stands; a standing one under way says it stands. */
export function stateWord(goal: GoalShown, words: GoalWords): string {
  return goal.standing && goal.status === "active" ? words.standing : words.states[goal.status];
}

/** How long a goal has been worked at now, counting the stretch under way. */
export function workedNow(goal: GoalShown, now: number): number {
  return goal.worked + (goal.status === "active" ? Math.max(0, now - goal.since) : 0);
}

export class GoalBar {
  readonly element: HTMLElement;
  private goal: GoalShown | null = null;
  private clearing = 0;
  private tick = 0;

  constructor(
    private readonly words: GoalWords,
    private readonly acts: GoalActs,
  ) {
    this.element = document.createElement("div");
    this.element.className = "gb-bar";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]")?.dataset.act;
      if (act === "pause") this.acts.pause();
      else if (act === "resume") this.acts.resume();
      else if (act === "expand") this.acts.expand?.();
      else if (act === "clear") this.startClearing();
      else if (act === "undo") this.undo();
    });
  }

  show(goal: GoalShown | null): void {
    this.goal = goal;
    if (this.clearing) return;
    this.draw();
  }

  private draw(): void {
    clearTimeout(this.tick);
    const goal = this.goal;
    this.element.hidden = goal === null;
    if (!goal) return;
    const words = this.words;
    const now = Date.now();
    this.element.dataset.status = goal.status;
    const when =
      goal.status === "paused" && goal.paused
        ? words.paused(goal.paused.by, goal.paused.until === null ? null : words.at(goal.paused.until, now))
        : words.worked(workedNow(goal, now));
    const turn = goal.status === "active" ? this.button("pause", "pause", words.pause) : this.button("resume", "resume", words.resume);
    const reason = goal.paused?.by === "unwoken" ? words.unwokenWhy : goal.paused?.reason;
    const why = goal.status === "paused" && reason ? ` title="${escape(reason)}"` : "";
    const acts = [this.button("clear", "close", words.clear), turn, this.acts.expand ? this.button("expand", "expand", words.expand) : ""].join("");
    const state = goal.standing && goal.status === "active" ? words.barStanding : words.bar[goal.status];
    this.element.innerHTML = `<span class="gb-mark" aria-hidden="true">${glyph("goal", 16)}</span><span class="gb-state">${escape(state)}</span><span class="gb-text" title="${escape(goal.objective)}">${escape(goal.objective)}</span><span class="gb-when"${why}>${escape(when)}</span><span class="gb-acts">${acts}</span>`;
    if (goal.status === "active") this.tick = window.setTimeout(() => this.draw(), TICK_MS);
  }

  private startClearing(): void {
    clearTimeout(this.tick);
    this.element.dataset.status = "cleared";
    this.element.innerHTML = `<span class="gb-mark" aria-hidden="true">${glyph("goal", 16)}</span><span class="gb-text">${escape(this.words.cleared)}</span><span class="gb-acts"><button class="gb-undo" type="button" data-act="undo">${escape(this.words.undo)}</button></span>`;
    this.clearing = window.setTimeout(() => {
      this.clearing = 0;
      this.goal = null;
      this.element.hidden = true;
      this.acts.clear();
    }, UNDO_MS);
  }

  private undo(): void {
    clearTimeout(this.clearing);
    this.clearing = 0;
    this.draw();
  }

  private button(act: string, mark: "close" | "pause" | "resume" | "expand", label: string): string {
    return `<button class="icon-button gb-act" type="button" data-act="${act}" aria-label="${escape(label)}" title="${escape(label)}">${glyph(mark, 16)}</button>`;
  }
}
