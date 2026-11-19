// A conversation's own parts, for the side of the desk while it is in focus
// (`world-gui.md`, 对话的侧面板): its overview first, a card for each part,
// then its goal, its files and its history, each a page of the side opened
// on a tab, as each file read there is; a card, or the whole side, can be
// taken out into a window of its own (`board.ts`) and put back; the
// goal's bar above the input while the goal is worked toward. Who works with
// it is drawn from two places: the colleagues it asked, from the World, and
// the host's own agents, from the conversation itself as it goes. A colleague
// pressed opens beside the conversation on the desk; everyone who works with
// it, and one of the host's agents, are read in a sheet. Where it works is
// said in the desk's corner, whether the side is open or folded away.

import type { AgentRun } from "../conversation/agents";
import { PlanPage, type PlanShown } from "../plan/panel";
import { planShape } from "../plan/shape";
import type { Address } from "../world/wire";
import { CollabPage } from "./collab-page";
import { GoalBar } from "./goal-bar";
import { GoalPage } from "./goal-page";
import { HistoryCard } from "./history-card";
import { SideHome } from "./home";
import { SideFiles } from "./files-page";
import type { ChangedFile } from "../files/model";
import type { ViewTab } from "../files/view";
import { SideWhere } from "./where";
import { ALONE, ONLY, PULLS, askBack, bringFront, pullOut, sendFile, serve, standsOut, type FileAsked, type Served } from "./board";
import { cardNames } from "./shown";
import type { Reading } from "./file-tabs";
import type { FileLines } from "../files/view";
import { idsIn, type LiveFact } from "../plan/live";
import type { PanePart, PartSet, Tab } from "./pane";
import type { Following, SideSeams } from "./seams";
import type { CollabNode, GoalShown, NodeState, WhereShown } from "./types";

/** The side's own parts, as their marks are named. */
export const PART = { home: "part:home", goal: "part:goal", files: "part:files", history: "part:history" } as const;

export interface PartsContext {
  host: "claude" | "codex";
  seams: SideSeams;
  /** Says a line beside the input, as the page does. */
  say(text: string, detail?: string): void;
  /** Sends the user's words into the conversation at once, as an answer. */
  answer(text: string): Promise<void>;
  /** Brings the row of the call that sent one of the host's agents into
   *  view and opens it: what the agent did is read there, in the
   *  conversation, not in a window over it. */
  jump(id: string): void;
  /** What one of the host's agents does this moment, in a few words. */
  doing?(run: AgentRun): string | null;
  /** Opens a file of the conversation's over the side's parts: to what it
   *  holds, or to its changes. */
  openFile?(machine: string, path: string, tab: ViewTab | null): void;
  detail?(error: unknown): string;
  /** How a file of the conversation's is read, for the window its files'
   *  card stands in. */
  reading?: Reading;
  /** In a window of its own (`board.ts`), where one who works with the
   *  conversation is opened: in the desk's window, not here. */
  away?: { node(node: CollabNode): void };
}

/** A file's changes as they cross to a board: whether each has lines to
 *  read stands in for the way to read them. */
export type ChangedTold = (Omit<ChangedFile, "changes"> & { changes: (Omit<ChangedFile["changes"][number], "rows"> & { rowed: boolean })[] })[];

/** What a board follows of a conversation's side, by name. */
type Topic = "plan" | "goal" | "team" | "where" | "changed";

/** The side showing these parts, while their conversation is in focus. */
export interface PartsFrame {
  /** Opens the side at one of these parts. */
  show(part: string): void;
  /** Opens a page on a tab of the side, the side opened for it. */
  add(tab: Tab): void;
  /** Shuts the tabs that read files. */
  shutFiles(): void;
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

/** One of the host's agents as a node. */
function nodeOf(run: AgentRun, host: "claude" | "codex", now: number, doing?: (run: AgentRun) => string | null): CollabNode {
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
  };
}

/** A node's time worked to the minute: the team is drawn again as a minute
 *  passes, not as each second does. */
const minutes = (name: string, value: unknown) => (name === "took" ? Math.floor((value as number) / 60_000) : value);

export class SideParts implements PartSet {
  readonly parts: PanePart[];
  readonly tabs: Tab[] = [];
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
  /** The colleagues it asked, and the host's agents it sent, each a page
   *  set into its own card of the overview. */
  private readonly colleaguesPage: CollabPage;
  private readonly agentsPage: CollabPage;
  private readonly goalPage: GoalPage;
  private readonly filesPage: SideFiles | null = null;
  private readonly planPage: PlanPage;
  private readonly stops: (() => void)[] = [];
  private colleagues: CollabNode[] = [];
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
  /** What was last shown of each thing a board follows, and the boards
   *  told of it as it changes. */
  private shownPlan: PlanShown | null = null;
  private shownWhere: WhereShown | null = null;
  private teamNodes: CollabNode[] = [];
  private changedFiles: ChangedFile[] = [];
  private readonly telling: Record<Topic, Set<(...args: unknown[]) => void>> = { plan: new Set(), goal: new Set(), team: new Set(), where: new Set(), changed: new Set() };

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
    // The files it changed and the folders it works in: what their card of
    // the overview turns to; in a window of their own, the window's first
    // page.
    if (seams.files)
      this.filesPage = new SideFiles({
        words: seams.files.words,
        machine: seams.self.machine,
        list: seams.files.list,
        home: seams.files.home,
        browse: seams.files.browse,
        open: (machine, path, tab) => context.openFile?.(machine, path, tab),
        detail: (error) => context.detail?.(error) ?? String(error),
      });
    this.colleaguesPage = new CollabPage(words.collab, words.goal, (node) => this.openNode(node), ["colleague", "side"]);
    this.agentsPage = new CollabPage(words.collab, words.goal, (node) => this.openNode(node), ["subagent"]);
    page.querySelector(".composer-dock > form.composer")?.before(this.bar.element);
    this.home = new SideHome(
      words,
      {
        page: () => this.frame?.show(PART.goal),
        file: (machine, path) => context.openFile?.(machine, path, "changes"),
        // A card taken out stands in a window of its own, its place kept
        // here; the files' goes with the files open, which are shut here.
        away: (card) => card !== "goal" && standsOut(seams.self, card),
        front: (card) => card !== "goal" && bringFront(seams.self, card),
        back: (card) => card !== "goal" && askBack(seams.self, card),
        pull: PULLS
          ? (card, from) => {
              if (card === "goal" || !pullOut(seams.self, card, card === "files" ? this.filesOpen() : [], from, `${cardNames(words)[card]} - Fastctx`)) return;
              if (card === "files") this.frame?.shutFiles();
            }
          : undefined,
      },
      !seams.noGoal,
      this.planPage.element,
      seams.files?.words ?? null,
      // The pages set into their cards. The files in a window of their own
      // are that window's first page, not a card's.
      { colleagues: this.colleaguesPage, agents: this.agentsPage, ...(this.filesPage && ONLY !== "files" ? { files: this.filesPage } : {}) },
    );
    this.parts =
      ONLY === "files" && this.filesPage && seams.files
        ? [{ id: PART.files, name: seams.files.words.side.name, page: this.filesPage }]
        : [{ id: PART.home, name: words.home.name, page: this.home }, ...(seams.noGoal ? [] : [{ id: PART.goal, name: words.pages.goal, page: this.goalPage }])];
    // The history its model works with, a page of its own that the mark in
    // the desk's corner opens: its project's, else its own.
    if (seams.history && !ONLY) {
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
    this.following = seams.colleagues((nodes) => {
      this.colleagues = nodes;
      this.goalPage.showTeam(nodes);
      this.drawTeam();
    });
    const following = this.following;
    this.stops.push(
      seams.where((where) => {
        this.shownWhere = where;
        this.where.show(where);
        this.filesPage?.place(where);
        this.tell("where");
      }),
      seams.plan.watch((plan) => this.plan(plan)),
      ...(seams.noGoal ? [] : [seams.goal.watch((goal) => this.goal(goal))]),
      () => following.stop(),
    );
    this.drawTeam();
    // The cards of this side that stand in windows of their own are shown
    // and worked from here.
    if (!ALONE) this.stops.push(serve(seams.self, this.served()));
  }

  /** The host's own agents, as the conversation now knows them. */
  agents(runs: AgentRun[]): void {
    this.runs = runs;
    this.drawTeam();
  }

  /** The files the conversation changed, as they now stand. */
  changed(files: ChangedFile[]): void {
    this.changedFiles = files;
    this.home.files(files);
    this.filesPage?.changed(files);
    this.tell("changed");
  }

  /** The conversation these are the parts of. */
  get self(): Address {
    return this.context.seams.self;
  }

  /** A page opened on a tab of the side, the side opened for it, over the
   *  whole desk where there is no room beside; false where the side is
   *  showing another conversation's parts. */
  openTab(tab: Tab): boolean {
    if (!this.frame) return false;
    this.frame.add(tab);
    return true;
  }

  /** Turns the side to a tab already open; false where there is none, or
   *  the side is showing another conversation's parts. */
  showTab(id: string): boolean {
    if (!this.frame || !this.tabs.some((tab) => tab.id === id)) return false;
    this.frame.show(id);
    return true;
  }

  /** Files opened here on tabs: those a board had open as it came back,
   *  or the ones the desk sent a board. */
  reopen(files: FileAsked[]): void {
    for (const file of files) this.context.openFile?.(file.machine, file.path, file.tab);
  }

  /** The files read on tabs here. */
  filesOpen(): FileAsked[] {
    return this.tabs.flatMap((tab) => (tab.file ? [{ ...tab.file, tab: null }] : []));
  }

  /** A file pressed in the conversation goes to where its files are read
   *  in a window of their own: true where it was sent there. `lost` where
   *  the window turns out to be gone. */
  sendOut(file: FileAsked, lost: () => void): boolean {
    return sendFile(this.self, file, lost);
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
    return this.parts[0]!.id;
  }

  /** The part in view, none while the side is shut or shows another
   *  conversation's: the colleagues' goals and agents are read only while
   *  the overview, the goal or everyone's sheet shows them. */
  seen(part: string | null): void {
    this.shownPart = part;
    this.follow();
  }

  private follow(): void {
    const detailed = this.shownPart === PART.home || this.shownPart === PART.goal || this.telling.team.size > 0;
    if (detailed === this.detailed) return;
    this.detailed = detailed;
    this.following?.detail(detailed);
  }

  leave(): void {
    for (const stop of this.stops) stop();
    for (const tab of this.tabs.splice(0)) tab.leave?.();
    this.where.dispose();
    this.home.dispose();
    this.bar.dispose();
  }

  /** One that works with the conversation, pressed on its card: a
   *  colleague opens beside it on the desk; one of the host's agents is
   *  read in the conversation, under the row of the call that sent it,
   *  which is brought into view and opened. */
  private openNode(node: CollabNode): void {
    if (this.context.away) return this.context.away.node(node);
    if (node.kind === "colleague" || node.kind === "side") {
      this.frame?.beside({ resident: node.id, machine: node.machine ?? this.context.seams.self.machine });
      return;
    }
    this.context.jump(node.id);
  }

  /** The plan: its page, and its card on the overview, how far it is and
   *  what it asks. */
  private plan(plan: PlanShown | null): void {
    this.shownPlan = plan;
    this.tell("plan");
    this.planPage.show(plan ?? { source: null, version: 0, when: "" });
    this.home.plan(plan?.source ?? null, plan?.source ? planShape(plan.source) : null);
  }

  /** The goal: on its bar while it is worked toward, one done leaving it
   *  for its page; its card on the overview while the bar does not say it. */
  private goal(goal: GoalShown | null): void {
    this.shownGoal = goal;
    this.tell("goal");
    this.goalPage.show(goal);
    const barred = !this.context.seams.noGoal && goal && goal.status !== "complete" ? goal : null;
    this.bar.show(barred);
    this.home.goal(goal, barred !== null);
  }

  /** Who works with the conversation, drawn again when it changed: who is
   *  in it, how each stands, and the minutes they have worked. */
  private drawTeam(): void {
    const now = Date.now();
    const doing = this.context.doing?.bind(this.context);
    // The host's agents are one level with the colleagues: one an agent
    // sent of its own stands among them, not under it.
    const nodes = [...this.colleagues, ...this.runs.map((run) => nodeOf(run, this.context.host, now, doing))];
    const shown = JSON.stringify(nodes, minutes);
    if (shown === this.shownTeam) return;
    this.shownTeam = shown;
    // What is new is told by what the World pushes, the same whether a
    // page reads the colleagues in full or not: a colleague's goal comes
    // and goes with the page and tells nothing.
    const colleagues = nodes
      .filter((node) => node.kind === "colleague")
      .map((node) => ({ ...node, goal: null, branchOf: null, why: null }))
      .sort((a, b) => `${a.machine}/${a.id}`.localeCompare(`${b.machine}/${b.id}`));
    const key = JSON.stringify([...colleagues, ...nodes.filter((node) => node.kind !== "colleague")], minutes);
    const fresh = this.drawnTeam !== "" && key !== this.drawnTeam;
    this.drawnTeam = key;
    this.teamNodes = nodes;
    this.tell("team");
    this.colleaguesPage.show(nodes);
    this.agentsPage.show(nodes);
    this.home.team(nodes);
    if (fresh) {
      this.fresh = true;
      this.frame?.restate();
    }
  }

  /** What a board is told of `topic` now: the plan with what is known of
   *  each id it names, which a board cannot ask as it draws. */
  private said(topic: Topic): unknown[] {
    if (topic === "plan") {
      const facts: Record<string, LiveFact | null> = {};
      for (const id of idsIn(this.shownPlan?.source ?? "")) facts[id] = this.context.seams.plan.seams.facts(id);
      const plan = this.shownPlan;
      return [plan && { source: plan.source, version: plan.version, when: plan.when }, facts];
    }
    if (topic === "goal") return [this.shownGoal];
    if (topic === "team") return [this.teamNodes];
    if (topic === "where") return [this.shownWhere];
    const told: ChangedTold = this.changedFiles.map((file) => ({ ...file, changes: file.changes.map(({ rows, ...change }) => ({ ...change, rowed: !!rows })) }));
    return [told];
  }

  /** Tells the boards following `topic` how it stands now. */
  private tell(topic: Topic): void {
    if (!this.telling[topic].size) return;
    const said = this.said(topic);
    for (const tell of this.telling[topic]) tell(...said);
  }

  /** What the cards of this side standing in windows of their own ask of
   *  the desk's window, and follow there (`board.ts`). Nothing more is read
   *  for them: they are told what this side already knows. */
  private served(): Served {
    const { context } = this;
    const { seams } = context;
    const reading = context.reading;
    const none = () => Promise.reject(new Error("files are not read here"));
    /** The files a board reads, by the number it was given for each. */
    const lines = new Map<number, { board: string; source: FileLines }>();
    let given = 0;
    const source = (at: number) => {
      const held = lines.get(at);
      if (!held) throw new Error("the file is no longer held");
      return held.source;
    };
    const follow = (topic: Topic) => (_board: string, tell: (...args: unknown[]) => void) => {
      this.telling[topic].add(tell);
      tell(...this.said(topic));
      if (topic === "team") this.follow();
      return () => {
        this.telling[topic].delete(tell);
        if (topic === "team") this.follow();
      };
    };
    return {
      calls: {
        hello: () => ({ host: context.host, list: !!seams.files?.list, browse: !!seams.files?.browse, reveal: !!seams.reveal, picture: !!reading?.picture, outside: !!reading?.outside }),
        "plan.save": (_board, text: string, expected: number) => seams.plan.seams.save(text, expected),
        "plan.open": (_board, id: string) => {
          seams.plan.seams.open?.(id);
          window.focus();
        },
        answer: (_board, text: string) => context.answer(text),
        say: (_board, text: string, detail: string | null) => context.say(text, detail ?? undefined),
        "files.list": (_board, machine: string, path: string) => seams.files?.list?.(machine, path) ?? none(),
        "files.home": (_board, machine: string) => seams.files?.home(machine) ?? null,
        "files.browse": (_board, machine: string, folder: string | null) => {
          seams.files?.browse?.(machine, folder);
          window.focus();
        },
        reveal: (_board, path: string) => seams.reveal?.(path),
        // One who works with the conversation is opened here, as from the
        // side: a colleague beside it, one of the host's agents at the row
        // of the conversation that sent it.
        node: (_board, id: string) => {
          const node = this.teamNodes.find((one) => one.id === id);
          if (!node) return;
          this.openNode(node);
          window.focus();
        },
        rows: (_board, id: string) =>
          this.changedFiles
            .flatMap((file) => file.changes)
            .find((change) => change.id === id)
            ?.rows?.() ?? null,
        "lines.open": async (board, machine: string, path: string) => {
          const opened = await (reading?.lines(machine, path) ?? none());
          lines.set((given += 1), { board, source: opened });
          return { handle: given, extent: opened.extent };
        },
        "lines.read": (_board, at: number, start: number, count: number) => source(at).read(start, count, new AbortController().signal),
        "lines.find": (_board, at: number, query: string, from: { line: number; offset?: number }, direction: 1 | -1) => source(at).find(query, from, direction, new AbortController().signal),
        "lines.close": async (_board, at: number) => {
          const held = lines.get(at);
          lines.delete(at);
          await held?.source.close();
        },
        picture: (_board, machine: string, path: string, max: number) => reading?.picture?.(machine, path, max) ?? none(),
        outside: (_board, machine: string, path: string) => reading?.outside?.(machine, path),
      },
      topics: {
        plan: follow("plan"),
        goal: follow("goal"),
        team: follow("team"),
        where: follow("where"),
        changed: follow("changed"),
        // A file's lines as they change, and the way to them breaking.
        lines: (_board, tell, at: number) => {
          const held = lines.get(at)?.source;
          if (!held) return () => {};
          const stops = [held.subscribe((change) => tell("changed", change, held.extent)), held.interruptions?.((broken) => tell("broken", broken)) ?? (() => {})];
          return () => stops.forEach((stop) => stop());
        },
      },
      left: (board) => {
        for (const [at, held] of lines) {
          if (held.board !== board) continue;
          lines.delete(at);
          void held.source.close().catch(() => {});
        }
      },
      gone: (error) => reading?.gone(error) ?? false,
      detail: (error) => context.detail?.(error) ?? (error instanceof Error ? error.message : String(error)),
    };
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
