// A conversation's own parts, for the side of the desk while it is in focus
// (`world-gui.md`, 对话的侧面板): its goal, its plan and who works with it,
// each a page whole beneath its mark, and how each stands on the mark; the
// goal's bar above the input while the goal is worked toward. Who works with
// it is drawn from two places: the colleagues it asked, from the World, and
// the host's own agents, from the conversation itself as it goes. A colleague
// pressed opens beside the conversation on the desk; one of the host's agents
// is read in the side over the parts.

import type { AgentRun } from "../conversation/agents";
import { PlanPage, type PlanShown } from "../plan/panel";
import { planShape } from "../plan/shape";
import type { Address } from "../world/wire";
import { CollabPage } from "./collab-page";
import { GoalBar } from "./goal-bar";
import { GoalPage } from "./goal-page";
import type { PanePart, PartSet, PartState, Peek } from "./pane";
import type { SideSeams } from "./seams";
import type { CollabNode, GoalShown, NodeState } from "./types";

/** The side's own parts, as their marks are named. */
export const PART = { goal: "part:goal", plan: "part:plan", team: "part:team" } as const;

export interface PartsContext {
  host: "claude" | "codex";
  seams: SideSeams;
  /** Says a line beside the input, as the page does. */
  say(text: string, detail?: string): void;
  /** Sends the user's words into the conversation at once, as an answer. */
  answer(text: string): Promise<void>;
  /** What one of the host's agents did, as a box the side can show. */
  agentBox(run: AgentRun): { element: HTMLElement; update(run: AgentRun): void; destroy(): void };
  /** Brings the row of the call that sent one of the host's agents into view. */
  jump(id: string): void;
  /** What the conversation is pointed at, at the side's foot; none where it
   *  was not opened in Fastctx. */
  env?: HTMLElement | null;
}

/** The side showing these parts, while their conversation is in focus. */
export interface PartsFrame {
  /** Opens the side at one of these parts. */
  show(part: string): void;
  /** Reads something over the parts, the side opened for it. */
  peek(peek: Peek): void;
  /** How these parts stand changed. */
  restate(): void;
  /** Whether the side lies over the whole desk, with no room beside. */
  readonly lying: boolean;
  /** Another conversation, opened beside this one on the desk. */
  beside(address: Address): void;
}

function stateOf(run: AgentRun): NodeState {
  if (run.going) return "working";
  if (run.state === "done") return "done";
  if (run.state === "failed") return "failed";
  if (run.state === "stopped") return "stopped";
  return "unknown";
}

/** Whether `id` is `node` or anyone under it. */
function contains(node: CollabNode, id: string): boolean {
  return node.id === id || node.children.some((child) => contains(child, id));
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

export class SideParts implements PartSet {
  readonly parts: PanePart[];
  readonly env: HTMLElement | null;
  part: string | null = null;
  /** Someone new works with the conversation while its side was not seen. */
  fresh = false;
  /** The side showing these parts, while their conversation is in focus. */
  frame: PartsFrame | null = null;
  private readonly states = new Map<string, PartState>();
  private readonly bar: GoalBar;
  private readonly collab: CollabPage;
  private readonly goalPage: GoalPage;
  private readonly planPage: PlanPage;
  /** Whether the conversation has a plan, for where the side first opens. */
  private planned = false;
  private readonly stops: (() => void)[] = [];
  private colleagues: CollabNode[] = [];
  private runs: AgentRun[] = [];
  private drawnTeam = "";
  /** The goal as last shown, whose version a change is made against. */
  private shownGoal: GoalShown | null = null;
  /** The box of the host's agent read over the parts, followed as it goes. */
  private box: { id: string; update(run: AgentRun): void } | null = null;

  constructor(
    page: HTMLElement,
    private readonly context: PartsContext,
  ) {
    const { seams } = context;
    const words = seams.words;
    const planWords = seams.plan.words;
    this.env = context.env ?? null;
    // What it is pointed at is said at the side's foot while it is in focus,
    // not in its head.
    this.env?.remove();
    this.planPage = new PlanPage(planWords, {
      ...seams.plan.seams,
      // A decision on the plan is the user's words to the conversation.
      answer: (question, option) => context.answer(planWords.reply(question, option)),
    });
    const version = () => this.shownGoal?.version;
    this.goalPage = new GoalPage(words.goal, {
      save: (text, seen, standing) => this.act(seams.goal.set(text, seen, standing)),
      pause: () => void this.act(seams.goal.pause(version())),
      resume: () => void this.act(seams.goal.resume(version())),
      clear: () => void this.act(seams.goal.clear(version())),
      past: () => seams.goal.past(),
      open: (node) => this.openNode(node),
    });
    this.bar = new GoalBar(words.goal, {
      pause: () => void this.act(seams.goal.pause(version())),
      resume: () => void this.act(seams.goal.resume(version())),
      clear: () => void this.act(seams.goal.clear(version())),
      expand: () => this.frame?.show(PART.goal),
    });
    this.collab = new CollabPage(words.collab, words.goal, (node) => this.openNode(node));
    page.querySelector(".composer-dock > form.composer")?.before(this.bar.element);
    const team = document.createElement("section");
    team.className = "sp-team";
    team.append(this.collab.element);
    this.parts = [
      ...(seams.noGoal ? [] : [{ id: PART.goal, name: words.pages.goal, glyph: "goal" as const, page: this.goalPage }]),
      { id: PART.plan, name: words.pages.plan, glyph: "rows", page: this.planPage },
      { id: PART.team, name: words.pages.collab, glyph: "tree", page: { element: team, shown: () => this.collab.shown() } },
    ];
    this.stops.push(
      seams.plan.watch((plan) => this.plan(plan)),
      seams.goal.watch((goal) => this.goal(goal)),
      seams.colleagues((nodes) => {
        this.colleagues = nodes;
        this.goalPage.showTeam(nodes);
        this.drawTeam();
      }),
    );
    this.drawTeam();
  }

  state(id: string): PartState | undefined {
    return this.states.get(id);
  }

  /** The host's own agents, as the conversation now knows them. */
  agents(runs: AgentRun[]): void {
    this.runs = runs;
    this.drawTeam();
    if (this.box) {
      const run = runs.find((at) => at.id === this.box!.id);
      if (run) this.box.update(run);
    }
  }

  /** One of the host's agents, opened in the side from the row that sent
   *  it; false where the conversation does not know it, or where the side
   *  lies over the desk and the row opens where it stands. */
  openAgent(id: string): boolean {
    const run = this.runs.find((at) => at.id === id);
    if (!run || !this.frame || this.frame.lying) return false;
    this.openNode(nodeOf(run, this.context.host, new Map(), Date.now()));
    return true;
  }

  /** Whether a goal may be set here, from the input among others. */
  get goals(): boolean {
    return !this.context.seams.noGoal;
  }

  /** The goal set from the input (`/goal`): it opens at once. */
  setGoal(objective: string): void {
    void this.act(this.context.seams.goal.set(objective, this.shownGoal?.version));
  }

  /** Where the side opens with nothing chosen: with whoever is at work,
   *  else at the plan, else at the goal, else with who works with it. */
  firstPart(): string {
    if (this.collabAtWork()) return PART.team;
    if (this.planned) return PART.plan;
    if (this.shownGoal && !this.context.seams.noGoal) return PART.goal;
    return PART.team;
  }

  leave(): void {
    for (const stop of this.stops) stop();
  }

  /** One that works with the conversation, pressed in its whole or in the
   *  conversation: a colleague opens beside it on the desk, one of the
   *  host's agents is read in the side. */
  private openNode(node: CollabNode): void {
    if (node.kind === "colleague" || node.kind === "side") {
      this.frame?.beside({ resident: node.id, machine: node.machine ?? this.context.seams.self.machine });
      return;
    }
    // An agent a colleague sent is read in the colleague's conversation.
    if (!this.runs.some((run) => run.id === node.id)) {
      const colleague = this.colleagues.find((one) => contains(one, node.id));
      if (colleague) this.openNode(colleague);
      return;
    }
    // With no room beside, the side gives way to the row that sent it.
    if (!this.frame || this.frame.lying) {
      this.context.jump(node.id);
      return;
    }
    const run = this.runs.find((at) => at.id === node.id)!;
    const box = this.context.agentBox(run);
    const jump = document.createElement("button");
    jump.type = "button";
    jump.className = "text-button sp-jump";
    jump.textContent = this.context.seams.words.collab.jump;
    jump.addEventListener("click", () => this.context.jump(node.id));
    box.element.prepend(jump);
    this.box = { id: node.id, update: (next) => box.update(next) };
    const held = this.box;
    this.frame.peek({
      element: box.element,
      leave: () => {
        if (this.box === held) this.box = null;
        box.destroy();
      },
    });
  }

  private setState(id: string, state: PartState): void {
    const before = this.states.get(id);
    if (before && JSON.stringify(before) === JSON.stringify(state)) return;
    this.states.set(id, state);
    this.frame?.restate();
  }

  /** The plan: its page, and on its mark how far it is, in clay while a
   *  question in it waits on the user. */
  private plan(plan: PlanShown | null): void {
    this.planPage.show(plan ?? { source: null, version: 0, when: "" });
    const shape = plan?.source ? planShape(plan.source) : null;
    this.planned = !!shape && (shape.checks.length > 0 || shape.stops.length > 0 || shape.asks.length > 0 || !!shape.title);
    const words = this.context.seams.words;
    let count = "";
    if (shape?.checks.length) count = words.planCount(shape.checks.filter((check) => check.done).length, shape.checks.length);
    else if (shape?.stops.length) {
      const here = shape.stops.findIndex((stop) => stop.where === "here");
      if (here >= 0) count = words.planCount(here + 1, shape.stops.length);
    }
    const ask = shape?.asks[0];
    this.setState(PART.plan, {
      ...(count ? { count } : {}),
      ...(ask ? { waiting: true, said: ask } : {}),
    });
  }

  /** The goal: on its bar while it is worked toward, one done leaving it
   *  for its part of the side; on its mark, how it stands. */
  private goal(goal: GoalShown | null): void {
    this.shownGoal = goal;
    this.goalPage.show(goal);
    this.bar.show(!this.context.seams.noGoal && goal && goal.status !== "complete" ? goal : null);
    const words = this.context.seams.words.goal;
    // Blocked, the goal waits on the user.
    this.setState(PART.goal, {
      ...(goal?.status === "blocked" ? { waiting: true } : {}),
      ...(goal ? { said: `${words.states[goal.status]}\n${goal.objective}` } : {}),
    });
  }

  private collabAtWork(): boolean {
    const working = (nodes: CollabNode[]): boolean => nodes.some((node) => node.state === "working" || working(node.children));
    return working(this.colleagues) || this.runs.some((run) => run.going);
  }

  /** Who works with the conversation, drawn again when it changed: who is
   *  in it, how each stands, and the minutes they have worked. */
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
    this.collab.show({ agent: this.context.host, name: "" }, nodes);
    let all = 0;
    let working = 0;
    const count = (list: CollabNode[]) => {
      for (const node of list) {
        all += 1;
        if (node.state === "working") working += 1;
        count(node.children);
      }
    };
    count(nodes);
    const collab = this.context.seams.words.collab;
    if (fresh) this.fresh = true;
    this.setState(PART.team, {
      ...(all ? { count: String(all) } : {}),
      ...(working ? { said: collab.atWork(working) } : {}),
    });
    if (fresh) this.frame?.restate();
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
