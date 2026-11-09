// A conversation's own parts, for the side of the desk while it is in focus
// (`world-gui.md`, 对话的侧面板): its overview first, a card for each part,
// then its goal, its plan and its history, each a page of the side; the
// goal's bar above the input while the goal is worked toward. Who works with
// it is drawn from two places: the colleagues it asked, from the World, and
// the host's own agents, from the conversation itself as it goes. A colleague
// pressed opens beside the conversation on the desk; everyone who works with
// it, and one of the host's agents, are read in a sheet. Where it works is
// said in the desk's corner, whether the side is open or folded away.

import { Sheet } from "../app/confirm";
import type { AgentRun } from "../conversation/agents";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { PlanPage, type PlanShown } from "../plan/panel";
import { planShape } from "../plan/shape";
import type { Address } from "../world/wire";
import { CollabPage } from "./collab-page";
import { GoalBar } from "./goal-bar";
import { GoalPage } from "./goal-page";
import { HistoryCard } from "./history-card";
import { SideHome } from "./home";
import { SideWhere } from "./where";
import type { PanePart, PartSet, Peek } from "./pane";
import type { Following, SideSeams } from "./seams";
import type { CollabNode, GoalShown, NodeState } from "./types";

/** The side's own parts, as their marks are named. */
export const PART = { home: "part:home", goal: "part:goal", history: "part:history" } as const;

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
  /** What one of the host's agents does this moment, in a few words. */
  doing?(run: AgentRun): string | null;
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
function nodeOf(
  run: AgentRun,
  host: "claude" | "codex",
  under: Map<string, AgentRun[]>,
  now: number,
  doing?: (run: AgentRun) => string | null,
): CollabNode {
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
    began: run.began,
    ended: run.going ? null : (run.ended ?? (run.began !== null && run.duration !== null ? run.began + run.duration : null)),
    doing: run.going ? (doing?.(run) ?? null) : null,
    tokens: run.tokens,
    model: run.model,
    steps: run.tools ?? run.steps.length,
    children: (under.get(run.id) ?? []).map((child) => nodeOf(child, host, under, now, doing)),
  };
}

/** A node's time worked to the minute: the team is drawn again as a minute
 *  passes, not as each second does. */
const minutes = (name: string, value: unknown) => (name === "took" ? Math.floor((value as number) / 60_000) : value);

export class SideParts implements PartSet {
  readonly parts: PanePart[];
  readonly where: SideWhere;
  part: string | null = null;
  /** Someone new works with the conversation while its side was not seen. */
  fresh = false;
  /** The side showing these parts, while their conversation is in focus. */
  frame: PartsFrame | null = null;
  /** Its history's part, and what the mark that opens it is named; none
   *  without a history. */
  history: { part: string; label: string } | null = null;
  private readonly bar: GoalBar;
  private readonly home: SideHome;
  private readonly collab: CollabPage;
  /** Everyone who works with the conversation, while their sheet is open. */
  private teamSheet: Sheet | null = null;
  private readonly goalPage: GoalPage;
  private readonly planPage: PlanPage;
  private readonly stops: (() => void)[] = [];
  private colleagues: CollabNode[] = [];
  private teamReading = false;
  /** The colleagues followed, read in full while a page shows them. */
  private following: Following | null = null;
  private detailed = false;
  private shownPart: string | null = null;
  private runs: AgentRun[] = [];
  private drawnTeam = "";
  /** The team as last drawn, in full. */
  private shownTeam = "";
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
    this.where = new SideWhere(words.where, words.close, seams.reveal);
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
    this.home = new SideHome(
      words,
      {
        page: () => this.frame?.show(PART.goal),
        node: (node) => this.openNode(node),
        team: () => this.openTeam(),
      },
      !seams.noGoal,
      this.planPage.element,
    );
    this.parts = [
      { id: PART.home, name: words.home.name, page: this.home },
      ...(seams.noGoal ? [] : [{ id: PART.goal, name: words.pages.goal, page: this.goalPage }]),
    ];
    // The history its model works with, a page of its own that the mark in
    // the desk's corner opens: its project's, else its own.
    if (seams.history) {
      const history = seams.history;
      const earlier = history.earlier;
      const card = new HistoryCard({
        words: history.words,
        link: history.link,
        write: history.write,
        ...(history.project ? { project: history.project } : {}),
        ...(earlier ? { earlier: (name: string) => earlier.link(name) } : {}),
      });
      this.parts.push({ id: PART.history, name: words.pages.history, page: card });
      this.history = {
        part: PART.history,
        label: history.project ? history.words.of(history.project.name) : words.pages.history,
      };
      this.stops.push(history.watch((catalog) => card.show(catalog)));
      if (earlier) this.stops.push(earlier.watch((catalog) => card.showEarlier(catalog)));
    }
    this.following = seams.colleagues((nodes, reading = false) => {
      this.colleagues = nodes;
      this.teamReading = reading;
      this.goalPage.showTeam(nodes);
      this.drawTeam();
    });
    const following = this.following;
    this.stops.push(
      seams.where((where) => this.where.show(where)),
      seams.plan.watch((plan) => this.plan(plan)),
      ...(seams.noGoal ? [] : [seams.goal.watch((goal) => this.goal(goal))]),
      () => following.stop(),
    );
    this.drawTeam();
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

  /** One of the host's agents, opened in a sheet from the row that sent
   *  it; false where the conversation does not know it. */
  openAgent(id: string): boolean {
    const run = this.runs.find((at) => at.id === id);
    if (!run) return false;
    this.openNode(nodeOf(run, this.context.host, new Map(), Date.now()));
    return true;
  }

  /** A file read in the side over the parts, the side opened for it, over
   *  the whole desk where there is no room beside; false where the side is
   *  showing another conversation's parts. */
  openFile(page: Peek): boolean {
    if (!this.frame) return false;
    this.frame.peek(page);
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

  /** Where the side opens with nothing chosen: at the overview. */
  firstPart(): string {
    return PART.home;
  }

  /** The part in view, none while the side is shut or shows another
   *  conversation's: the colleagues' goals and agents are read only while
   *  the overview, the goal or everyone's sheet shows them. */
  seen(part: string | null): void {
    this.shownPart = part;
    this.follow();
  }

  private follow(): void {
    const detailed = this.shownPart === PART.home || this.shownPart === PART.goal || this.teamSheet !== null;
    if (detailed === this.detailed) return;
    this.detailed = detailed;
    this.following?.detail(detailed);
  }

  /** Everyone who works with the conversation, in a sheet: a colleague
   *  pressed opens beside the conversation, one of the host's agents in the
   *  sheet, the way back to everyone at its head. */
  private openTeam(): void {
    const sheet = this.teamSheet ?? new Sheet("cs-sheet");
    if (!this.teamSheet) {
      this.teamSheet = sheet;
      sheet.dialog.addEventListener("close", () => {
        if (this.teamSheet === sheet) this.teamSheet = null;
        this.follow();
      });
    }
    const words = this.context.seams.words;
    sheet.body.innerHTML = sheet.head(words.pages.collab, words.close);
    sheet.body.append(this.collab.element);
    sheet.anew();
    sheet.show();
    this.follow();
  }

  /** One of the host's agents, read in a sheet: in everyone's sheet where it
   *  was pressed there, with the way back to them; its row in the
   *  conversation a press away. */
  private readAgent(run: AgentRun, node: CollabNode): void {
    const words = this.context.seams.words;
    const back = this.teamSheet;
    const sheet = back ?? new Sheet("cs-sheet");
    const box = this.context.agentBox(run);
    let gone = false;
    const done = () => {
      if (gone) return;
      gone = true;
      if (this.box === held) this.box = null;
      box.destroy();
    };
    const way = back ? `<button class="text-button page-back cs-back" type="button">${glyph("back", 18)}<span>${escape(words.pages.collab)}</span></button>` : "";
    sheet.body.innerHTML = `<header class="sheet-head cs-head">${way}<h2 class="sheet-title">${escape(node.task || node.name)}</h2><button class="text-button cs-jump" type="button">${escape(words.collab.jump)}</button><button class="icon-button sheet-close" type="button" aria-label="${escape(words.close)}">${glyph("close", 18)}</button></header>`;
    sheet.body.append(box.element);
    sheet.body.querySelector(".cs-back")?.addEventListener("click", () => {
      done();
      this.openTeam();
    });
    sheet.body.querySelector(".cs-jump")!.addEventListener("click", () => {
      sheet.close();
      this.context.jump(node.id);
    });
    sheet.dialog.addEventListener("close", done, { once: true });
    this.box = { id: node.id, update: (next) => box.update(next) };
    const held = this.box;
    sheet.anew();
    sheet.show();
  }

  leave(): void {
    for (const stop of this.stops) stop();
  }

  /** One that works with the conversation, pressed in its whole or in the
   *  conversation: a colleague opens beside it on the desk, one of the
   *  host's agents is read in the side. */
  private openNode(node: CollabNode): void {
    if (node.kind === "colleague" || node.kind === "side") {
      this.teamSheet?.close();
      this.frame?.beside({ resident: node.id, machine: node.machine ?? this.context.seams.self.machine });
      return;
    }
    // An agent a colleague sent is read in the colleague's conversation.
    if (!this.runs.some((run) => run.id === node.id)) {
      const colleague = this.colleagues.find((one) => contains(one, node.id));
      if (colleague) this.openNode(colleague);
      return;
    }
    this.readAgent(this.runs.find((at) => at.id === node.id)!, node);
  }

  /** The plan: its page, and its card on the overview, how far it is and
   *  what it asks. */
  private plan(plan: PlanShown | null): void {
    this.planPage.show(plan ?? { source: null, version: 0, when: "" });
    this.home.plan(plan?.source ?? null, plan?.source ? planShape(plan.source) : null);
  }

  /** The goal: on its bar while it is worked toward, one done leaving it
   *  for its page; its card on the overview while the bar does not say it. */
  private goal(goal: GoalShown | null): void {
    this.shownGoal = goal;
    this.goalPage.show(goal);
    const barred = !this.context.seams.noGoal && goal && goal.status !== "complete" ? goal : null;
    this.bar.show(barred);
    this.home.goal(goal, barred !== null);
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
    const doing = this.context.doing?.bind(this.context);
    const nodes = [...this.colleagues, ...top.map((run) => nodeOf(run, this.context.host, under, now, doing))];
    const shown = JSON.stringify([nodes, this.teamReading], minutes);
    if (shown === this.shownTeam) return;
    this.shownTeam = shown;
    // What is new is told by what the World pushes, the same whether a
    // page reads the colleagues in full or not: a colleague's goal and its
    // agents come and go with the page and tell nothing.
    const colleagues = nodes
      .filter((node) => node.kind === "colleague")
      .map((node) => ({ ...node, goal: null, branchOf: null, why: null, children: [] }))
      .sort((a, b) => `${a.machine}/${a.id}`.localeCompare(`${b.machine}/${b.id}`));
    const key = JSON.stringify([...colleagues, ...nodes.filter((node) => node.kind !== "colleague")], minutes);
    const fresh = this.drawnTeam !== "" && key !== this.drawnTeam;
    this.drawnTeam = key;
    this.collab.show(nodes, this.teamReading);
    this.home.team(nodes);
    if (fresh) {
      this.fresh = true;
      this.frame?.restate();
    }
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
