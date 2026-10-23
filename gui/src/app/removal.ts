// Taking fastctx off this computer, from the settings (`world-gui.md`, 设置),
// in three steps under one line of them, each taking the place of the one
// before rather than growing under it: two ways, each a slip with its own
// small drawing; then what the way chosen takes and what it keeps, grouped,
// with the full list a press away in a frame of its own; a hub this computer
// keeps is said plainly and its World's name typed to go on; then every step
// listed at once and ticked off in place as it is done, and World closes and
// uninstalls itself.

import { Sketch } from "../pen/sketch";
import { stepMark } from "../pen/step-mark";
import { escape } from "../conversation/prose";
import type { RemovalEvent, RemovalItem, RemovalPlan } from "../world/bridge";
import { DrawnOnce } from "./drawn-once";
import { progressSteps } from "./progress-steps";
import { groupOf, type RemovalGroup, type RemovalWords } from "./removal-words";

/** What the page needs of the shell to remove fastctx here. */
export interface Removing {
  plan(everything: boolean): Promise<RemovalPlan>;
  run(everything: boolean, onEvent: (event: RemovalEvent) => void): Promise<void>;
  finish(everything: boolean): Promise<void>;
}

type Result = { result: "done" | "failed" | "skipped"; why: string | null };

type State =
  | { at: "choose" }
  | { at: "planning"; everything: boolean }
  | { at: "unlisted"; everything: boolean; why: string }
  | { at: "plan"; plan: RemovalPlan; armed: boolean; typed: string; open: boolean }
  | { at: "running"; plan: RemovalPlan; results: Map<string, Result> }
  | { at: "finished"; plan: RemovalPlan; results: Map<string, Result>; left: { kind: string; what: string; where: string | null; why: string }[]; error: string | null }
  | { at: "finishing"; plan: RemovalPlan; error: string | null };

const ORDER: RemovalGroup[] = ["start", "agents", "hub", "program", "data", "hubs"];

export class RemovalSection {
  private state: State = { at: "choose" };
  /** Set while the section rewrites itself. Chromium (WebView2) reports
   *  the pressed button it removes as a focusout without a related target,
   *  synchronously inside the rewrite; that is not the reader moving on. */
  private drawing = false;
  /** The dialog is drawn again on every change: its drawings are drawn in
   *  once, and the second step's panel rises in once. */
  private readonly drawn = new DrawnOnce();
  private risen = false;

  constructor(
    private readonly host: HTMLElement,
    private readonly words: RemovalWords,
    private readonly seam: Removing,
    /** Told when steps start running and when they have ended: the dialog
     *  around it does not close in between. */
    private readonly working: (on: boolean) => void = () => {},
  ) {
    host.addEventListener("click", (event) => {
      const target = (event.target as HTMLElement).closest<HTMLElement>("[data-removal]");
      if (target && !(target as HTMLButtonElement).disabled) this.act(target.dataset.removal!, target);
    });
    host.addEventListener("input", (event) => {
      const input = event.target as HTMLInputElement;
      if (input.dataset.removalName === undefined || this.state.at !== "plan") return;
      this.state.typed = input.value;
      const go = host.querySelector<HTMLButtonElement>("[data-removal='go']");
      if (go) go.disabled = !this.nameTyped(this.state);
    });
    // A press that waits for a second one is let go when the reader moves on.
    host.addEventListener("focusout", (event) => {
      const into = (event as FocusEvent).relatedTarget as HTMLElement | null;
      if (this.drawing) return;
      if (this.state.at === "plan" && this.state.armed && !into?.closest("[data-removal='go']")) {
        this.state.armed = false;
        this.draw();
      }
    });
    this.draw();
  }

  /** Goes to one of the two ways, as if it were pressed. */
  choose(everything: boolean): void {
    this.act(everything ? "erase" : "keep", this.host);
  }

  private nameTyped(state: Extract<State, { at: "plan" }>): boolean {
    return !(state.plan.everything && state.plan.hub) || state.typed.trim() === state.plan.hub.name;
  }

  private act(act: string, target: HTMLElement): void {
    const state = this.state;
    switch (act) {
      case "keep":
      case "erase": {
        const everything = act === "erase";
        if (state.at === "running" || state.at === "finished" || state.at === "finishing") return;
        if ((state.at === "plan" && state.plan.everything === everything) || (state.at === "planning" && state.everything === everything)) return;
        this.state = { at: "planning", everything };
        this.draw();
        this.seam.plan(everything).then(
          (plan) => {
            if (this.state.at !== "planning" || this.state.everything !== everything) return;
            this.state = { at: "plan", plan, armed: false, typed: "", open: false };
            this.draw();
          },
          (error: unknown) => {
            if (this.state.at !== "planning" || this.state.everything !== everything) return;
            this.state = { at: "unlisted", everything, why: String(error) };
            this.draw();
          },
        );
        return;
      }
      case "list":
        if (state.at === "plan") {
          state.open = !state.open;
          this.draw();
        }
        return;
      case "cancel":
        this.state = { at: "choose" };
        this.draw();
        return;
      case "go":
        if (state.at !== "plan" || !this.nameTyped(state)) return;
        if (state.plan.everything && !state.armed) {
          state.armed = true;
          this.draw();
          this.host.querySelector<HTMLButtonElement>("[data-removal='go']")?.focus();
          return;
        }
        this.run(state.plan);
        return;
      case "finish":
        if (state.at !== "finished") return;
        this.state = { at: "finishing", plan: state.plan, error: null };
        this.draw();
        this.seam.finish(state.plan.everything).catch((error: unknown) => {
          if (this.state.at === "finishing") this.state.error = String(error);
          this.draw();
        });
        return;
      default:
        void target;
    }
  }

  private run(plan: RemovalPlan): void {
    const results = new Map<string, Result>();
    this.state = { at: "running", plan, results };
    this.working(true);
    this.draw();
    let left: { kind: string; what: string; where: string | null; why: string }[] = [];
    this.seam
      .run(plan.everything, (event) => {
        if ("left" in event) {
          left = event.left;
          return;
        }
        results.set(event.step, { result: event.result, why: event.why });
        this.tick(event.step);
      })
      .then(
        () => ({ error: null }),
        (error: unknown) => ({ error: String(error) }),
      )
      .then(({ error }) => {
        this.state = { at: "finished", plan, results, left, error };
        this.working(false);
        this.draw();
      });
  }

  /** Marks one step in place, so the rest of the list does not move; the
   *  count follows, and the frame keeps the step after it in view. */
  private tick(step: string): void {
    if (this.state.at !== "running") return;
    const row = this.host.querySelector<HTMLElement>(`[data-step="${CSS.escape(step)}"]`);
    const result = this.state.results.get(step);
    if (!row || !result) return;
    row.dataset.result = result.result;
    row.querySelector(".removal-mark")!.innerHTML = stepMark(result.result);
    if (result.why) row.title = result.why;
    const count = this.host.querySelector<HTMLElement>(".removal-count");
    if (count) count.textContent = this.words.count(this.state.results.size, this.state.plan.steps.length);
    const frame = this.host.querySelector<HTMLElement>(".removal-frame");
    const next = (row.nextElementSibling as HTMLElement | null) ?? row;
    if (frame && next.offsetTop + next.offsetHeight > frame.scrollTop + frame.clientHeight) frame.scrollTop = next.offsetTop + next.offsetHeight - frame.clientHeight;
  }

  private draw(): void {
    const { words } = this;
    const state = this.state;
    const step = state.at === "choose" ? 0 : state.at === "planning" || state.at === "unlisted" || state.at === "plan" ? 1 : state.at === "running" ? 2 : 3;
    this.drawing = true;
    try {
      this.host.innerHTML = `${progressSteps(words.stages, step, words.title)}<div class="removal-stage${step === 0 ? " choosing" : ""}">${this.body()}</div>`;
    } finally {
      this.drawing = false;
    }
    this.drawn.seen(this.host);
    this.risen = state.at === "plan";
    const name = this.host.querySelector<HTMLInputElement>("[data-removal-name]");
    if (name && state.at === "plan") name.value = state.typed;
  }

  private body(): string {
    const { words } = this;
    const state = this.state;
    const back = `<button class="text-button" type="button" data-removal="cancel">${escape(words.back)}</button>`;
    switch (state.at) {
      case "choose":
        return `<div class="welcome-ways removal-ways">${this.way(false)}${this.way(true)}</div>`;
      case "planning":
        return `${this.chosen(state.everything)}<p class="removal-said">${escape(words.planning)}</p>`;
      case "unlisted":
        return `${this.chosen(state.everything)}<p class="removal-said removal-wrong" title="${escape(state.why)}">${escape(words.failed(state.why))}</p><div class="removal-go">${back}</div>`;
      case "plan": {
        const { plan } = state;
        const admin = plan.steps.some((step) => step.admin);
        const hub = plan.everything && plan.hub;
        // The full list takes the overview's place, so the step keeps its
        // height and its foot.
        const overview = state.open
          ? `<div class="removal-frame"><h3 class="removal-head">${escape(words.goes)}</h3>${this.list(plan.steps, null)}${plan.keeps.length ? `<h3 class="removal-head">${escape(words.stays)}</h3>${this.list(plan.keeps, null)}` : ""}</div>`
          : `<div class="removal-columns">
              ${this.column(words.goes, plan.steps, "goes")}
              ${plan.keeps.length ? this.column(words.stays, plan.keeps, "stays") : ""}
            </div>
            ${hub ? `<p class="removal-warn">${escape(words.hubWarn(plan.hub!.name))}</p>` : ""}
            ${plan.other ? `<p class="removal-note">${escape(words.other(plan.other.path, plan.other.version))}</p>` : ""}`;
        return `${this.chosen(plan.everything)}<div class="removal-plan${this.risen ? " risen" : ""}">
          ${overview}
          <button class="text-button removal-toggle" type="button" data-removal="list" aria-expanded="${state.open}">${escape(state.open ? words.hideList : words.list)}</button>
        </div>
        <div class="removal-go">
          ${hub ? `<input class="removal-name" data-removal-name type="text" spellcheck="false" autocomplete="off" placeholder="${escape(words.typeName(plan.hub!.name))}" aria-label="${escape(words.typeName(plan.hub!.name))}">` : ""}
          ${admin ? `<span class="removal-admin">${escape(words.admin)}</span>` : ""}
          ${back}
          <button class="removal-button${state.armed ? " armed" : ""}" type="button" data-removal="go"${this.nameTyped(state) ? "" : " disabled"}>${escape(state.armed ? words.again : plan.everything ? words.goErase : words.go)}</button>
        </div>`;
      }
      case "running":
        return `${this.tally(words.running, state.results.size, state.plan.steps.length)}<div class="removal-frame">${this.list(state.plan.steps, state.results)}</div>`;
      case "finished": {
        const failed = state.left.length > 0 || state.error !== null || [...state.results.values()].some((r) => r.result === "failed");
        return `${this.tally(failed ? words.left : words.allDone, state.results.size, state.plan.steps.length, failed)}
          ${state.error ? `<p class="removal-said removal-wrong" title="${escape(state.error)}">${escape(words.failedRun(state.error))}</p>` : ""}
          <div class="removal-frame">
            ${
              state.left.length
                ? `<ul class="removal-left">${state.left
                    .map((item) => `<li><span>${escape(this.words.kinds[item.kind] ?? item.what)}</span>${item.where ? `<code>${escape(item.where)}</code>` : ""}<span class="removal-why" title="${escape(item.why)}">${escape(this.words.leftWhy[item.kind] ?? this.words.unfinished)}</span></li>`)
                    .join("")}</ul>`
                : ""
            }
            ${this.list(state.plan.steps, state.results)}
          </div>
          <div class="removal-go">
            <span class="removal-admin">${escape(words.finishWhat)}</span>
            <button class="removal-button" type="button" data-removal="finish">${escape(words.finish)}</button>
          </div>`;
      }
      case "finishing":
        return `<p class="removal-said${state.error ? " removal-wrong" : ""}">${escape(state.error ?? words.finishing)}</p>`;
    }
  }

  /** One of the two ways, as a slip with its drawing. */
  private way(everything: boolean): string {
    const { words } = this;
    const [act, title, what, more] = everything ? ["erase", words.erase, words.eraseWhat, words.eraseMore] : ["keep", words.keep, words.keepWhat, words.keepMore];
    const drawing = this.drawn.mark(act, everything ? eraseMark() : keepMark());
    return `<button class="welcome-way removal-way${everything ? " erase" : ""}" type="button" data-removal="${act}" title="${escape(more)}">${drawing}<span class="welcome-way-title">${escape(title)}</span><span class="welcome-way-what">${escape(what)}</span></button>`;
  }

  /** The way chosen, small, over what it does: its drawing and its words. */
  private chosen(everything: boolean): string {
    const { words } = this;
    const [title, what] = everything ? [words.erase, words.eraseWhat] : [words.keep, words.keepWhat];
    const drawing = this.drawn.mark(everything ? "chosen-erase" : "chosen-keep", everything ? eraseMark() : keepMark());
    return `<div class="removal-chosen${everything ? " erase" : ""}">${drawing}<span class="removal-chosen-words"><span class="removal-chosen-title">${escape(title)}</span><span class="removal-chosen-what">${escape(what)}</span></span></div>`;
  }

  /** Over the list being ticked: what is happening, and how far it is. */
  private tally(said: string, done: number, all: number, wrong = false): string {
    return `<div class="removal-tally"><span class="removal-tally-said${wrong ? " removal-wrong" : ""}">${escape(said)}</span><span class="removal-count">${escape(this.words.count(done, all))}</span></div>`;
  }

  /** What goes, or stays, as a slip per group, each listing its items when
   *  the pointer rests on it. */
  private column(title: string, items: RemovalItem[], kind: "goes" | "stays"): string {
    const groups = new Map<RemovalGroup, RemovalItem[]>();
    for (const item of items) {
      const group = groupOf(item.kind);
      groups.set(group, [...(groups.get(group) ?? []), item]);
    }
    const chips = ORDER.filter((group) => groups.has(group))
      .map((group) => {
        const inside = groups
          .get(group)!
          .map((item) => [this.label(item), item.where].filter(Boolean).join(" "))
          .join("\n");
        return `<li class="removal-chip" title="${escape(inside)}">${escape(this.words.groups[group])}</li>`;
      })
      .join("");
    return `<div class="removal-column ${kind}"><h3 class="removal-head">${escape(title)}</h3><ul class="removal-chips">${chips}</ul></div>`;
  }

  private label(item: RemovalItem): string {
    return this.words.kinds[item.kind] ?? item.what;
  }

  private list(steps: RemovalItem[], results: Map<string, Result> | null): string {
    return `<ol class="removal-list">${steps
      .map((step) => {
        const result = results?.get(step.id);
        return `<li data-step="${escape(step.id)}"${result ? ` data-result="${result.result}"` : ""}${result?.why ? ` title="${escape(result.why)}"` : ""}>${results ? `<span class="removal-mark" aria-hidden="true">${stepMark(result?.result ?? null)}</span>` : ""}<span class="removal-what">${escape(this.label(step))}</span>${step.where ? `<code class="removal-where" title="${escape(step.where)}">${escape(step.where)}</code>` : ""}</li>`;
      })
      .join("")}</ol>`;
  }
}

/** Keeping the data: the machine's ring drawn in dashes, going, and the
 *  knot of its records at the middle, staying. */
function keepMark(): string {
  const sheet = new Sketch("removal-keep");
  for (let i = 0; i < 8; i += 1) {
    const from = ((i * 45 - 80) * Math.PI) / 180;
    sheet.arc([44, 32], 21, from, from + (22 * Math.PI) / 180, 1.8, "graphite");
  }
  return sheet.blot([44, 32], 6, "clay").svg(96, 64, "welcome-way-mark");
}

/** Removing everything: the ring struck through, and the crumbs of an
 *  eraser beside it. */
function eraseMark(): string {
  return new Sketch("removal-erase")
    .ring([40, 32], 19, 1.8, "graphite")
    .stroke(
      [
        [14, 54],
        [30, 41],
        [48, 26],
        [68, 9],
      ],
      2.4,
      "clay",
      0.1,
    )
    .blot([78, 43], 1.6, "clay")
    .blot([73, 51.5], 1.5, "clay")
    .blot([83.5, 53.5], 1.4, "clay")
    .svg(96, 64, "welcome-way-mark");
}
