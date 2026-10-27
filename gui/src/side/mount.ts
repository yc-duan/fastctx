// The panel beside a conversation, set into its page (`world-gui.md`,
// 对话右边的侧面板): the page's head across the whole page, and under it the
// conversation and the panel side by side; the panel's switch keeps one place
// at the top right under the head, over the conversation while the panel is
// shut and at the end of its pages' names while it is open; the goal's bar
// above the input. Who works with the conversation is drawn from two places: the
// colleagues it asked, from the World, and the host's own agents, from the
// conversation itself as it goes.

import { glyph } from "../conversation/glyphs";
import type { AgentRun } from "../conversation/agents";
import { PlanPage, type PlanShown } from "../plan/panel";
import { CollabPage } from "./collab-page";
import { GoalBar } from "./goal-bar";
import { GoalPage } from "./goal-page";
import { SidePanel } from "./panel";
import type { SideSeams } from "./seams";
import type { CollabNode, GoalShown, NodeState } from "./types";

/** Narrower than this, the panel lies over the conversation. */
const OVER = matchMedia("(max-width: 760px)");

export interface MountContext {
  host: "claude" | "codex";
  seams: SideSeams;
  /** Says a line beside the input, as the page does. */
  say(text: string, detail?: string): void;
  /** Sends the user's words into the conversation at once, as an answer. */
  answer(text: string): Promise<void>;
  /** What one of the host's agents did, as a box the panel can show. */
  agentBox(run: AgentRun): { element: HTMLElement; update(run: AgentRun): void; destroy(): void };
  /** Brings the row of the call that sent one of the host's agents into view. */
  jump(id: string): void;
}

function stateOf(run: AgentRun): NodeState {
  if (run.going) return "working";
  if (run.state === "done") return "done";
  if (run.state === "failed") return "failed";
  if (run.state === "stopped") return "stopped";
  return "unknown";
}

/** One of the host's agents as a node, with those it sent under it. */
function nodeOf(run: AgentRun, host: "claude" | "codex", under: Map<string, AgentRun[]>, now: number): CollabNode {
  const began = run.began ?? now;
  const took = run.duration ?? Math.max(0, (run.ended ?? (run.going ? now : began)) - began);
  return {
    id: run.id,
    kind: "subagent",
    agent: host,
    name: run.name ?? run.kind ?? run.id,
    task: run.task ?? run.prompt?.split("\n")[0] ?? "",
    state: stateOf(run),
    took,
    steps: run.tools ?? run.steps.length,
    children: (under.get(run.id) ?? []).map((child) => nodeOf(child, host, under, now)),
  };
}

export class SideMount {
  readonly panel: SidePanel;
  private readonly toggle: HTMLButtonElement;
  private readonly bar: GoalBar;
  private readonly goalPage: GoalPage;
  private readonly planPage: PlanPage;
  private readonly collab: CollabPage;
  private readonly stops: (() => void)[] = [];
  private colleagues: CollabNode[] = [];
  private runs: AgentRun[] = [];
  private drawnTeam = "";
  /** Whether anyone works with the conversation, for the switch. */
  private team = false;
  private planSeen: string | null = null;
  private goalSeen: string | null = null;
  /** The goal as last shown, whose version a change is made against. */
  private shownGoal: GoalShown | null = null;
  /** The box of one of the host's agents open in the panel. */
  private box: { id: string; update(run: AgentRun): void } | null = null;

  constructor(
    private readonly page: HTMLElement,
    private readonly context: MountContext,
  ) {
    const { seams } = context;
    const words = seams.words;
    const planWords = seams.plan.words;
    this.planPage = new PlanPage(planWords, {
      ...seams.plan.seams,
      // A decision on the plan is the user's words to the conversation.
      answer: (question, option) => context.answer(planWords.reply(question, option)),
    });
    const version = () => this.shownGoal?.version;
    this.goalPage = new GoalPage(words.goal, {
      save: (text, seen) => this.act(seams.goal.set(text, seen)),
      pause: () => void this.act(seams.goal.pause(version())),
      resume: () => void this.act(seams.goal.resume(version())),
      clear: () => void this.act(seams.goal.clear(version())),
    });
    this.collab = new CollabPage(words.collab, words.goal, (node) => this.openNode(node));
    const pages = { plan: this.planPage, ...(seams.noGoal ? {} : { goal: this.goalPage }), collab: this.collab };
    this.panel = new SidePanel(words, pages, { changed: () => this.drawToggle() });
    this.bar = new GoalBar(words.goal, {
      pause: () => void this.act(seams.goal.pause(version())),
      resume: () => void this.act(seams.goal.resume(version())),
      clear: () => void this.act(seams.goal.clear(version())),
      expand: seams.bare ? undefined : () => this.panel.show("goal"),
    });
    this.toggle = document.createElement("button");
    const composer = page.querySelector(".composer-dock > form.composer");
    composer?.before(this.bar.element);
    // A colleague's page in another's panel keeps only its goal's bar.
    if (seams.bare) {
      this.stops.push(seams.goal.watch((goal) => this.goal(goal)));
      return;
    }

    // The head across the page; under it the page's own column and the
    // panel side by side, the switch over both.
    const head = page.querySelector(":scope > .page-head");
    const main = document.createElement("div");
    main.className = "talk-main";
    main.append(...Array.from(page.childNodes).filter((node) => node !== head));
    const split = document.createElement("div");
    split.className = "sp-split";
    split.append(main, this.panel.element, this.toggle);
    page.append(split);
    page.classList.add("sp-host");

    this.toggle.type = "button";
    this.toggle.className = "icon-button sp-toggle";
    this.toggle.setAttribute("aria-label", words.panel);
    this.toggle.title = words.panel;
    this.toggle.innerHTML = `${glyph("panel", 18)}<span class="sp-toggle-dot" aria-hidden="true"></span>`;
    this.toggle.addEventListener("click", () => this.panel.toggle());

    this.place();
    OVER.addEventListener("change", this.place);
    this.stops.push(
      seams.plan.watch((plan) => this.plan(plan)),
      seams.goal.watch((goal) => this.goal(goal)),
      seams.colleagues((nodes) => {
        this.colleagues = nodes;
        this.drawTeam();
      }),
    );
    this.drawTeam();
    this.drawToggle();
  }

  /** The host's own agents, as the conversation now knows them. */
  agents(runs: AgentRun[]): void {
    this.runs = runs;
    this.drawTeam();
    const open = this.box ? runs.find((run) => run.id === this.box!.id) : undefined;
    if (open) this.box!.update(open);
  }

  /** One of the host's agents, opened in the panel from the row that sent
   *  it; false where the conversation does not know it. */
  openAgent(id: string): boolean {
    const run = this.runs.find((at) => at.id === id);
    if (!run) return false;
    this.panel.show("collab");
    this.openNode(nodeOf(run, this.context.host, new Map(), Date.now()));
    return true;
  }

  /** Whether the panel holds anything: its switch shows, and a phone's
   *  "⋯" offers it. */
  get shown(): boolean {
    return !!this.planSeen || !!this.shownGoal || this.team;
  }

  /** Whether a goal may be set here, from the input among others. */
  get goals(): boolean {
    return !this.context.seams.noGoal;
  }

  /** The goal set from the input (`/goal`): it opens at once. */
  setGoal(objective: string): void {
    void this.act(this.context.seams.goal.set(objective, this.shownGoal?.version));
  }

  leave(): void {
    OVER.removeEventListener("change", this.place);
    for (const stop of this.stops) stop();
    this.collab.back();
  }

  private readonly place = () => this.panel.over(OVER.matches);

  private plan(plan: PlanShown | null): void {
    const seen = plan?.source == null ? "" : `${plan.version}`;
    if (this.planSeen !== null && seen !== this.planSeen && seen !== "") this.panel.news("plan");
    this.planSeen = seen;
    this.planPage.show(plan ?? { source: null, version: 0, when: "" });
    this.drawToggle();
  }

  private goal(goal: GoalShown | null): void {
    this.shownGoal = goal;
    const seen = goal ? `${goal.version}|${goal.status}|${goal.sentinel?.at ?? ""}` : "";
    if (this.goalSeen !== null && seen !== this.goalSeen && seen !== "") this.panel.news("goal");
    this.goalSeen = seen;
    this.bar.show(goal);
    this.goalPage.show(goal);
    this.drawToggle();
  }

  /** The tree drawn again when what it shows changed: who is in it, how
   *  each stands, and the minutes they have worked. */
  private drawTeam(): void {
    const now = Date.now();
    const under = new Map<string, AgentRun[]>();
    const known = new Set(this.runs.map((run) => run.id));
    const top: AgentRun[] = [];
    for (const run of this.runs) {
      if (run.parent && known.has(run.parent)) under.set(run.parent, [...(under.get(run.parent) ?? []), run]);
      else top.push(run);
    }
    const nodes = [...this.colleagues, ...top.map((run) => nodeOf(run, this.context.host, under, now))];
    const key = JSON.stringify(nodes, (name, value) => (name === "took" ? Math.floor((value as number) / 60_000) : value));
    if (key === this.drawnTeam) return;
    const fresh = this.drawnTeam !== "";
    this.drawnTeam = key;
    this.team = nodes.length > 0;
    this.collab.show({ agent: this.context.host, name: "" }, nodes);
    if (fresh) this.panel.news("collab");
    this.drawToggle();
  }

  private openNode(node: CollabNode): void {
    if (node.kind === "colleague") {
      const opened = this.context.seams.colleague(node);
      if (opened) this.collab.open(node, opened.element, () => opened.leave(), null);
      return;
    }
    const run = this.runs.find((at) => at.id === node.id);
    if (!run) return;
    const box = this.context.agentBox(run);
    this.box = { id: run.id, update: (next) => box.update(next) };
    this.collab.open(
      node,
      box.element,
      () => {
        box.destroy();
        if (this.box?.id === run.id) this.box = null;
      },
      () => this.context.jump(run.id),
    );
  }

  /** The switch, and the page opening or shutting by the panel. Opening,
   *  the page is laid out shut first, so it opens from nothing. A panel with
   *  nothing in it has no switch. */
  private drawToggle(): void {
    const open = this.panel.open;
    if (open && !this.page.classList.contains("sp-shown")) void this.page.offsetWidth;
    this.page.classList.toggle("sp-shown", open);
    this.toggle.setAttribute("aria-pressed", String(open));
    this.toggle.classList.toggle("news", this.panel.hasNews);
    this.toggle.hidden = !open && !this.panel.leaving && !this.shown;
  }

  /** What the user did to the goal: said beside the input when it did not
   *  go through, the World's own words when pointed at, or when the goal
   *  changed elsewhere first. Resolves whether it went through. */
  private act(done: Promise<boolean>): Promise<boolean> {
    const words = this.context.seams.words.goal;
    return done.then(
      (went) => {
        if (!went) this.context.say(words.changed);
        return went;
      },
      (error: unknown) => {
        this.context.say(words.failed, error instanceof Error ? error.message : String(error));
        return false;
      },
    );
  }
}

