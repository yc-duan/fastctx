// The goal's page in the panel beside a conversation (`world-goal.md`,
// 界面上), set in one voice like the plan: how it stands, drawn as a mark
// and said in a word, with how long it has been worked at or why it waits;
// the user's words in a field that is edited where it stands, saved as the
// goal's next version; the agent's account of it done or blocked; and what
// the goal's sentinel last made of a stop; and the goals it had before.
// With no goal, or one done, the same field sets a new one, standing or
// not.

import { escape, render } from "../conversation/prose";
import { checkBox } from "../plan/render";
import { stateWord, workedNow } from "./goal-bar";
import { drawPast, pastHtml } from "./goal-past";
import { goalMark } from "./marks";
import type { SidePage } from "./pane";
import type { CollabNode, GoalShown, PastGoal } from "./types";
import type { GoalWords } from "./words";

export interface GoalPageActs {
  /** A goal set, standing or not, or its words changed, from the version
   *  the edit began at; resolves whether it went through. */
  save(objective: string, seen: number | undefined, standing: boolean): Promise<boolean>;
  pause(): void;
  resume(): void;
  clear(): void;
  /** The conversation's earlier goals. */
  past(): Promise<PastGoal[]>;
  /** A colleague, opened beside the conversation. */
  open(node: CollabNode): void;
}

export class GoalPage implements SidePage {
  readonly element: HTMLElement;
  private goal: GoalShown | null = null;
  /** The words as last saved or shown, to tell an edit from them. */
  private kept = "";
  /** The version an edit under way began at. */
  private editFrom: number | undefined;
  /** Whether a goal about to be set stands. */
  private standing = false;
  private past: PastGoal[] = [];
  private team: CollabNode[] = [];
  /** The goal the earlier ones were last asked after, to ask again once
   *  it is replaced or cleared. */
  private pastAfter: string | null | undefined;

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
      else if (act === "standing") this.toggleStanding();
      const target = event.target as HTMLElement;
      const opened = target.closest<HTMLButtonElement>("button.gh-goal");
      if (opened) {
        const said = opened.nextElementSibling as HTMLElement | null;
        const open = opened.getAttribute("aria-expanded") !== "true";
        opened.setAttribute("aria-expanded", String(open));
        if (said) said.hidden = !open;
        this.drawInk();
      }
      const branch = target.closest<HTMLElement>(".gh-branch")?.dataset.node;
      const node = branch ? this.team.find((one) => one.id === branch) : undefined;
      if (node) this.acts.open(node);
    });
    new ResizeObserver(() => this.drawInk()).observe(this.element);
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
    if ((goal?.id ?? null) !== this.pastAfter) this.askPast();
    if (editing) return this.drawState();
    this.draw();
  }

  shown(): void {
    const field = this.element.querySelector<HTMLTextAreaElement>(".gp-field");
    if (field) this.fit(field);
    this.drawMark();
    this.askPast();
  }

  /** The colleagues the conversation asked, some under its earlier goals. */
  showTeam(nodes: CollabNode[]): void {
    const team = nodes.filter((node) => node.kind === "colleague" && node.branchOf);
    const key = (list: CollabNode[]) => JSON.stringify(list.map((node) => [node.id, node.branchOf, node.goal, node.task]));
    if (key(team) === key(this.team)) return;
    this.team = team;
    this.drawPast();
  }

  private askPast(): void {
    const after = this.goal?.id ?? null;
    this.pastAfter = after;
    this.acts.past().then(
      (past) => {
        if (this.pastAfter !== after) return;
        this.past = past.filter((one) => one.id !== after);
        this.drawPast();
      },
      (error: unknown) => console.error("the earlier goals could not be read", error),
    );
  }

  /** The earlier goals, in their place at the foot of the page. */
  private drawPast(): void {
    const slot = this.element.querySelector<HTMLElement>(".gp-past-slot");
    if (!slot) return;
    slot.innerHTML = pastHtml(this.past, this.team, this.words, Date.now());
    requestAnimationFrame(() => this.drawInk());
  }

  private drawInk(): void {
    const slot = this.element.querySelector<HTMLElement>(".gp-past-slot");
    if (slot?.firstElementChild) drawPast(slot);
  }

  private toggleStanding(): void {
    this.standing = !this.standing;
    const toggle = this.element.querySelector<HTMLElement>(".gp-standing");
    if (!toggle) return;
    toggle.setAttribute("aria-pressed", String(this.standing));
    toggle.querySelector(".gp-standing-box")!.innerHTML = checkBox(this.standing, "goal-standing");
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
    // Words for no goal, or for one done, set a new one: it may stand.
    const fresh = !goal || goal.status === "complete";
    this.standing = false;
    const standing = fresh
      ? `<button class="gp-standing" type="button" data-act="standing" aria-pressed="false" title="${escape(words.standingTip)}"><span class="gp-standing-box" aria-hidden="true">${checkBox(false, "goal-standing")}</span><span>${escape(words.standing)}</span></button>`
      : "";
    this.element.innerHTML = `${goal ? '<div class="gp-state"></div>' : ""}<div class="gp-edit"><textarea class="gp-field" rows="2" spellcheck="false" placeholder="${escape(words.ask)}" aria-label="${escape(words.ask)}">${escape(this.kept)}</textarea><div class="gp-save" hidden>${standing}<button class="gp-main" type="button" data-act="save">${escape(fresh ? words.start : words.save)}</button></div></div>${statement}${sentinel}<div class="gp-past-slot"></div>`;
    this.drawPast();
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
    const reason = goal.paused?.by === "unwoken" ? words.unwokenWhy : goal.paused?.reason;
    const why = goal.status === "paused" && reason ? ` title="${escape(reason)}"` : "";
    line.dataset.status = goal.status;
    line.innerHTML = `<span class="gp-mark" aria-hidden="true"></span><span class="gp-word">${escape(stateWord(goal, words))}</span><span class="gp-when"${why}>${escape(when)}</span><span class="gp-acts">${turn}${this.act("clear", words.clear)}</span>`;
    this.drawMark();
  }

  /** How it stands, as a hand would mark it: a drop of ink that breathes
   *  while it is worked at, two strokes while it waits, a bar where it is
   *  stopped, a tick in clay once it is done. */
  private drawMark(): void {
    const mark = this.element.querySelector<HTMLElement>(".gp-mark");
    const goal = this.goal;
    if (!mark || !goal) return;
    mark.innerHTML = goalMark(goal.status, `goal-${goal.version}-${goal.status}`);
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
    const fresh = !this.goal || this.goal.status === "complete";
    this.kept = text;
    this.element.querySelector<HTMLElement>(".gp-save")!.hidden = true;
    field?.blur();
    void this.acts.save(text, seen, fresh && this.standing).then((went) => {
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
