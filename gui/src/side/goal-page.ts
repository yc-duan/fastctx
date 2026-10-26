// The goal's page in the panel beside a conversation (`world-goal.md`,
// 界面上), set in one voice like the plan: how it stands, drawn as a mark
// and said in a word, with how long it has been worked at or why it waits;
// the user's words in a field that is edited where it stands, saved as the
// goal's next version; the agent's account of it done or blocked; and what
// the goal's sentinel last made of a stop. With no goal, the same field
// sets one.

import { escape, render } from "../conversation/prose";
import { Sketch } from "../pen/sketch";
import { workedNow } from "./goal-bar";
import type { SidePage } from "./panel";
import type { GoalShown } from "./types";
import type { GoalWords } from "./words";

export interface GoalPageActs {
  /** A goal set, or its words changed, from the version the edit began at;
   *  resolves whether it went through. */
  save(objective: string, seen: number | undefined): Promise<boolean>;
  pause(): void;
  resume(): void;
  clear(): void;
}

export class GoalPage implements SidePage {
  readonly element: HTMLElement;
  private goal: GoalShown | null = null;
  /** The words as last saved or shown, to tell an edit from them. */
  private kept = "";
  /** The version an edit under way began at. */
  private editFrom: number | undefined;

  constructor(
    private readonly words: GoalWords,
    private readonly acts: GoalPageActs,
  ) {
    this.element = document.createElement("section");
    this.element.className = "gp-page scroller";
    this.element.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]")?.dataset.act;
      if (act === "pause") this.acts.pause();
      else if (act === "resume") this.acts.resume();
      else if (act === "clear") this.acts.clear();
      else if (act === "save") this.save();
    });
    this.element.addEventListener("input", (event) => {
      const field = event.target as HTMLTextAreaElement;
      if (!field.classList.contains("gp-field")) return;
      this.editFrom ??= this.goal?.version;
      this.fit(field);
      const save = this.element.querySelector<HTMLElement>(".gp-save");
      if (save) save.hidden = field.value.trim() === "" || field.value.trim() === this.kept.trim();
    });
    this.element.addEventListener("keydown", (event) => {
      const field = event.target as HTMLElement;
      if (field.classList.contains("gp-field") && event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
        event.preventDefault();
        this.save();
      }
    });
  }

  show(goal: GoalShown | null): void {
    const field = this.element.querySelector<HTMLTextAreaElement>(".gp-field");
    // An edit under way is not swept away by news of the goal.
    const editing = field !== null && document.activeElement === field && field.value.trim() !== this.kept.trim();
    this.goal = goal;
    if (editing) return this.drawState();
    this.draw();
  }

  shown(): void {
    const field = this.element.querySelector<HTMLTextAreaElement>(".gp-field");
    if (field) this.fit(field);
    this.drawMark();
  }

  private draw(): void {
    const goal = this.goal;
    const words = this.words;
    this.kept = goal?.objective ?? "";
    this.editFrom = undefined;
    const statement =
      goal?.statement && (goal.status === "complete" || goal.status === "blocked")
        ? `<section class="gp-part"><h2 class="gp-label">${escape(goal.status === "complete" ? words.done : words.blocked)}</h2><div class="prose gp-statement">${render(goal.statement)}</div></section>`
        : "";
    const sentinel = goal?.sentinel ? this.sentinel(goal.sentinel) : "";
    this.element.innerHTML = `${goal ? '<div class="gp-state"></div>' : ""}<div class="gp-edit"><textarea class="gp-field" rows="2" spellcheck="false" placeholder="${escape(words.ask)}" aria-label="${escape(words.ask)}">${escape(this.kept)}</textarea><div class="gp-save" hidden><button class="gp-main" type="button" data-act="save">${escape(goal ? words.save : words.start)}</button></div></div>${statement}${sentinel}`;
    this.drawState();
    const field = this.element.querySelector<HTMLTextAreaElement>(".gp-field")!;
    requestAnimationFrame(() => this.fit(field));
  }

  /** The line of how it stands, with its acts. */
  private drawState(): void {
    const line = this.element.querySelector<HTMLElement>(".gp-state");
    const goal = this.goal;
    if (!line || !goal) return;
    const words = this.words;
    const now = Date.now();
    const when =
      goal.status === "paused" && goal.paused
        ? words.paused(goal.paused.by, goal.paused.until === null ? null : words.at(goal.paused.until, now))
        : words.worked(workedNow(goal, now));
    // A goal done goes on where the user finds it not done.
    const turn = goal.status === "active" ? this.act("pause", words.pause) : this.act("resume", words.resume);
    const why = goal.status === "paused" && goal.paused?.reason ? ` title="${escape(goal.paused.reason)}"` : "";
    line.dataset.status = goal.status;
    line.innerHTML = `<span class="gp-mark" aria-hidden="true"></span><span class="gp-word">${escape(words.states[goal.status])}</span><span class="gp-when"${why}>${escape(when)}</span><span class="gp-acts">${turn}${this.act("clear", words.clear)}</span>`;
    this.drawMark();
  }

  /** How it stands, as a hand would mark it: a drop of ink that breathes
   *  while it is worked at, two strokes while it waits, a bar where it is
   *  stopped, a tick in clay once it is done. */
  private drawMark(): void {
    const mark = this.element.querySelector<HTMLElement>(".gp-mark");
    const goal = this.goal;
    if (!mark || !goal) return;
    const sketch = new Sketch(`goal-${goal.version}-${goal.status}`);
    if (goal.status === "active") sketch.blot([8, 9], 3.4, "current");
    else if (goal.status === "paused") sketch.rule([5.5, 4.5], [5.5, 13.5], 1.6, "graphite").rule([10.5, 4.5], [10.5, 13.5], 1.6, "graphite");
    else if (goal.status === "blocked") sketch.rule([3, 9.2], [13, 8.6], 2.4, "clay");
    else sketch.stroke([[2.5, 9], [6.2, 12.8], [13.5, 4]], 1.8, "clay");
    mark.innerHTML = sketch.svg(16, 18, "gp-mark-svg");
  }

  private sentinel(look: NonNullable<GoalShown["sentinel"]>): string {
    const words = this.words;
    const at = words.sentinelAt(words.at(look.at, Date.now()));
    const said = look.sound
      ? look.reason
        ? `<p>${escape(look.reason)}</p>`
        : ""
      : `<dl class="gp-said">${[
          [words.looked, look.looked],
          [words.saw, look.saw],
          [words.thinks, look.thinks],
        ]
          .filter(([, text]) => text)
          .map(([label, text]) => `<dt>${escape(label!)}</dt><dd>${escape(text!)}</dd>`)
          .join("")}</dl>`;
    return `<section class="gp-part"><h2 class="gp-label">${escape(words.sentinel)}<span class="gp-label-when">${escape(at)}</span></h2>${said}</section>`;
  }

  private save(): void {
    const field = this.element.querySelector<HTMLTextAreaElement>(".gp-field");
    const text = field?.value.trim() ?? "";
    if (!text || text === this.kept.trim()) return;
    const seen = this.editFrom ?? this.goal?.version;
    this.kept = text;
    this.element.querySelector<HTMLElement>(".gp-save")!.hidden = true;
    field?.blur();
    void this.acts.save(text, seen).then((went) => {
      if (went) return;
      // Not taken: the user's words stay, to save again over what is there now.
      const now = this.element.querySelector<HTMLTextAreaElement>(".gp-field");
      if (!now) return;
      this.kept = this.goal?.objective ?? "";
      now.value = text;
      this.fit(now);
      this.editFrom = this.goal?.version;
      this.element.querySelector<HTMLElement>(".gp-save")!.hidden = false;
    });
  }

  /** The field as tall as its words, so the page, not it, scrolls. */
  private fit(field: HTMLTextAreaElement): void {
    field.style.height = "auto";
    field.style.height = `${field.scrollHeight}px`;
  }

  private act(name: string, label: string): string {
    return `<button class="gp-act" type="button" data-act="${name}">${escape(label)}</button>`;
  }
}
