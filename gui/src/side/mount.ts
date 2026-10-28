// What surrounds a conversation, set into its page (`world-gui.md`, 对话的
// 侧面板): the page is two parts, the conversation with its head and, opened,
// its side, at the conversation's right as tall as the window by default, or
// at its left, or along its foot as wide as the page; half the page at first
// and dragged larger or smaller by the line between them. Where it stands is
// kept on this device: a conversation dragged in from the list is set where
// it is let go, and the side's own menu moves it. Where the page has no room
// for it there, it stands along the other edge, or lies over the whole page
// as on a phone, until the room comes back. The side's first tab is the
// conversation's overview, a few small cards (`overview.ts`); after it,
// other conversations in tabs: a colleague it asked (to talk with), one of
// the host's agents (to read), or any conversation dragged in from the
// list. The tabs open beside a conversation are kept on this device, as is
// whether the side was open. The goal's bar stands above the input. Who
// works with the conversation is drawn from two places: the colleagues it
// asked, from the World, and the host's own agents, from the conversation
// itself as it goes.

import { makeGrip } from "../app/grip";
import { TALK_DRAG } from "../app/talk-row";
import { glyph } from "../conversation/glyphs";
import type { AgentRun } from "../conversation/agents";
import { holdColumn } from "../conversation/column-hold";
import { PlanPage, type PlanShown } from "../plan/panel";
import type { Address } from "../world/wire";
import { CollabPage } from "./collab-page";
import { GoalBar } from "./goal-bar";
import { GoalPage } from "./goal-page";
import { Overview } from "./overview";
import { SidePane, type Dock, type PaneContent } from "./pane";
import type { OpenedTalk, SideSeams } from "./seams";
import type { CollabNode, GoalShown, NodeState } from "./types";

/** Narrower than this, the side lies over the page. */
const OVER = matchMedia("(max-width: 760px)");

/** The share of the page the side was dragged to, kept on this device. */
const SHARE_KEPT = "side-share";
/** Half the page at first; the least the side may be, and the least it
 *  leaves the conversation, across and down. */
const SHARE = 0.5;
const LEAST = { across: 320, down: 180 };
const ROOM = { across: 360, down: 220 };

/** Where the side stands, kept on this device. */
const DOCK_KEPT = "side-dock";
const DOCKS: readonly Dock[] = ["right", "bottom", "left"];

function keptDock(): Dock {
  try {
    const dock = localStorage.getItem(DOCK_KEPT);
    return dock === "bottom" || dock === "left" ? dock : "right";
  } catch {
    return "right";
  }
}

function keptShare(): number | null {
  try {
    const share = Number(localStorage.getItem(SHARE_KEPT));
    return Number.isFinite(share) && share > 0 && share < 1 ? share : null;
  } catch {
    return null;
  }
}

interface KeptSide {
  open: boolean;
  /** Each tab's conversation, and the name it was opened under. */
  tabs: (Address & { name?: string })[];
  current: string | null;
}

const talkId = (address: Address) => `talk:${address.resident}@${address.machine}`;

export interface MountContext {
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
  /** What the conversation is pointed at, shown in the side's home; none
   *  where it was not opened in Fastctx. */
  env?: HTMLElement | null;
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

export class SideMount {
  readonly pane: SidePane;
  private readonly opener: HTMLButtonElement;
  private readonly bar: GoalBar;
  private readonly collab: CollabPage;
  /** The overview, the side's first tab; none on a conversation shown in
   *  another's side. */
  private readonly overview: Overview | null = null;
  /** Someone new works with the conversation while its side is shut. */
  private fresh = false;
  private readonly goalPage: GoalPage;
  private readonly planPage: PlanPage;
  private readonly stops: (() => void)[] = [];
  private colleagues: CollabNode[] = [];
  private runs: AgentRun[] = [];
  private drawnTeam = "";
  /** The goal as last shown, whose version a change is made against. */
  private shownGoal: GoalShown | null = null;
  /** The goals of the conversations open in tabs, followed for their marks. */
  private readonly tabGoals = new Map<string, () => void>();
  /** The boxes of the host's agents open in tabs, followed as they go. */
  private readonly boxes = new Map<string, (run: AgentRun) => void>();
  /** The conversation and the side, the two columns of the page. */
  private split: HTMLElement | null = null;
  /** Where the side is kept on this device, what was last kept, and
   *  whether it is still being put back as it was left. */
  private readonly kept: string;
  private written: string | null = null;
  private restoring = true;
  /** Where the side was put, and where it stands for the room the page
   *  has now. */
  private dock: Dock = keptDock();
  private placed: Dock | "over" = this.dock;

  constructor(
    private readonly page: HTMLElement,
    private readonly context: MountContext,
  ) {
    const { seams } = context;
    const words = seams.words;
    const planWords = seams.plan.words;
    this.kept = `side:${seams.self.resident}@${seams.self.machine}`;
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
    this.pane = new SidePane(words, {
      changed: () => {
        this.forgetGoals();
        this.drawSwitch();
      },
      dock: (dock) => this.setDock(dock),
      ...(seams.fork ? { fork: () => this.fork() } : {}),
    });
    this.bar = new GoalBar(words.goal, {
      pause: () => void this.act(seams.goal.pause(version())),
      resume: () => void this.act(seams.goal.resume(version())),
      clear: () => void this.act(seams.goal.clear(version())),
      expand: seams.bare ? undefined : () => this.openGoal(),
    });
    this.collab = new CollabPage(words.collab, words.goal, (node) => this.openNode(node));
    this.opener = document.createElement("button");
    const composer = page.querySelector(".composer-dock > form.composer");
    composer?.before(this.bar.element);
    // A conversation shown in another's side keeps only its goal's bar.
    if (seams.bare) {
      this.stops.push(seams.goal.watch((goal) => this.goal(goal)));
      return;
    }

    // The conversation with its head in one column; the side in the other,
    // its switch over the conversation.
    const main = document.createElement("div");
    main.className = "talk-main";
    main.append(...Array.from(page.childNodes));
    const split = document.createElement("div");
    split.className = "sp-split";
    split.append(main, this.pane.element);
    page.append(split);
    page.classList.add("sp-host");
    this.split = split;

    this.overview = new Overview(words, { goal: seams.noGoal ? null : this.goalPage, plan: this.planPage, collab: this.collab, env: context.env ?? null }, (node) => this.openNode(node));
    this.pane.setHome(this.overview.element);
    const mount = this;
    this.pane.element.append(
      makeGrip("sp-grip", {
        get part() {
          return mount.placed === "over" ? mount.dock : mount.placed;
        },
        label: words.resize,
        width: () => this.width(),
        least: () => this.least(),
        most: () => this.most(this.room()),
        set: (width) => this.resize(width),
        moving: (moving) => page.classList.toggle("sp-resizing", moving),
      }),
    );

    this.opener.type = "button";
    this.opener.className = "icon-button sp-toggle";
    this.opener.setAttribute("aria-label", words.panel);
    this.opener.title = words.panel;
    this.drawOpener();
    this.opener.addEventListener("click", () => this.pane.toggle());
    main.querySelector(":scope > .page-head")?.after(this.opener);
    const drop = document.createElement("div");
    drop.className = "sp-drop";
    drop.setAttribute("aria-hidden", "true");
    drop.innerHTML = `<span class="sp-drop-words"><span class="sp-drop-mark"></span><span></span></span>`;
    drop.querySelector(".sp-drop-words > span")!.textContent = words.dropHere;
    split.append(drop);
    this.dropping(split);

    this.place(true);
    const fitting = new ResizeObserver(() => this.place());
    fitting.observe(split);
    this.stops.push(() => fitting.disconnect());
    OVER.addEventListener("change", this.replace);
    this.restore();
    this.restoring = false;
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
    this.drawSwitch();
    seams.fork?.can().then(
      (can) => this.pane.offerFork(can),
      () => {},
    );
    this.stops.push(() => {
      for (const stop of this.tabGoals.values()) stop();
      this.tabGoals.clear();
    });
  }

  /** A side conversation branched off this one, opened in a tab beside it. */
  private fork(): void {
    const fork = this.context.seams.fork;
    if (!fork) return;
    this.pane.forking(true);
    fork.make().then(
      (address) => {
        this.pane.forking(false);
        if (!this.openTalk(address, this.context.seams.words.sideTalk)) this.context.seams.go(address);
      },
      (error: unknown) => {
        this.pane.forking(false);
        this.context.say(this.context.seams.words.forkFailed, error instanceof Error ? error.message : String(error));
      },
    );
  }

  /** The goals of tabs closed are no longer followed. */
  private forgetGoals(): void {
    const open = new Set(this.pane.tabs.map((tab) => tab.id));
    for (const [id, stop] of this.tabGoals) {
      if (open.has(id)) continue;
      stop();
      this.tabGoals.delete(id);
    }
  }

  /** The host's own agents, as the conversation now knows them. */
  agents(runs: AgentRun[]): void {
    this.runs = runs;
    this.drawTeam();
    for (const [id, update] of this.boxes) {
      const run = runs.find((at) => at.id === id);
      if (run) update(run);
    }
  }

  /** One of the host's agents, opened beside the conversation from the row
   *  that sent it; false where the conversation does not know it. */
  openAgent(id: string): boolean {
    const run = this.runs.find((at) => at.id === id);
    // With no room for two, the row opens where it stands.
    if (!run || this.placed === "over") return false;
    this.openNode(nodeOf(run, this.context.host, new Map(), Date.now()));
    return true;
  }

  /** Another conversation, opened beside this one, under `name` where the
   *  one who opened it knows it better than the list; false where it cannot
   *  be, as for this conversation itself. */
  openTalk(address: Address, name?: string): boolean {
    const self = this.context.seams.self;
    if (!this.split || (address.resident === self.resident && address.machine === self.machine)) return false;
    // With no room for two, it opens as the page's own.
    if (this.placed === "over") {
      this.context.seams.go(address);
      return true;
    }
    const id = talkId(address);
    if (!this.pane.tabs.some((tab) => tab.id === id) && !this.addTalk(address, name)) return false;
    this.pane.show(id);
    return true;
  }

  /** Whether there is a side for a phone's "⋯" to offer: its overview,
   *  wherever the conversation has a side of its own. */
  get shown(): boolean {
    return this.split !== null;
  }

  /** Whether a goal may be set here, from the input among others. */
  get goals(): boolean {
    return !this.context.seams.noGoal;
  }

  /** The goal set from the input (`/goal`): it opens at once. */
  setGoal(objective: string): void {
    void this.act(this.context.seams.goal.set(objective, this.shownGoal?.version));
  }

  /** The page is about to have `by` pixels more room across: the side takes
   *  its share, a conversation shown in it carried as this one is; what is
   *  left is the conversation's own. */
  roomWillChange(by: number): number {
    if (!this.pane.open || this.placed === "over" || !this.split) return by;
    // Along the foot the side is as wide as the page: both widen alike.
    if (this.placed === "bottom") {
      this.pane.content(this.pane.shownTab)?.roomWillChange?.(by);
      return by;
    }
    const room = this.room();
    const side = this.fitted(room + by) - this.fitted(room);
    this.pane.content(this.pane.shownTab)?.roomWillChange?.(side);
    return by - side;
  }

  leave(): void {
    OVER.removeEventListener("change", this.replace);
    for (const stop of this.stops) stop();
    for (const tab of this.pane.tabs) this.pane.content(tab.id)?.leave?.();
  }

  private readonly replace = () => this.place();

  /** The side laid out where it stands for the room the page has now:
   *  drawn anew where that changed, and sized for the room either way. */
  private place(first = false): void {
    const placed = this.placement(this.dock);
    if (first || placed !== this.placed) {
      this.placed = placed;
      this.pane.over(placed === "over");
      const shown = placed === "over" ? this.dock : placed;
      if (this.split) this.split.dataset.dock = shown;
      this.pane.setDock(shown);
      this.drawOpener();
    }
    this.pane.fitting(DOCKS.filter((dock) => this.placement(dock) === dock));
    this.fit();
  }

  /** Whether the page has room for the side and the conversation both,
   *  with the side at `dock`. */
  private fits(dock: Dock): boolean {
    if (!this.split) return false;
    return dock === "bottom" ? this.split.clientHeight >= LEAST.down + ROOM.down : this.split.clientWidth >= LEAST.across + ROOM.across;
  }

  /** Where the side stands when put at `dock`: there where it fits, else
   *  along the other edge where that fits, else over the whole page. */
  private placement(dock: Dock): Dock | "over" {
    if (OVER.matches) return "over";
    if (this.fits(dock)) return dock;
    const other: Dock = dock === "bottom" ? "right" : "bottom";
    return this.fits(other) ? other : "over";
  }

  /** Whether the side stands along the foot, sized by its height. */
  private get down(): boolean {
    return this.placed === "bottom";
  }

  /** The room the two parts share, across or down. */
  private room(): number {
    return (this.down ? this.split?.clientHeight : this.split?.clientWidth) ?? 0;
  }

  /** The side's size now, across or down. */
  private width(): number {
    return parseFloat(getComputedStyle(this.page).getPropertyValue(this.down ? "--side-height" : "--side-width")) || 0;
  }

  private least(): number {
    return this.down ? LEAST.down : LEAST.across;
  }

  /** The most the side may be in `room`, the conversation keeping its own. */
  private most(room: number): number {
    return Math.max(this.least(), room - (this.down ? ROOM.down : ROOM.across));
  }

  /** The side's size in `room`, at the share it was dragged to. */
  private fitted(room: number): number {
    return Math.round(Math.min(this.most(room), Math.max(this.least(), (keptShare() ?? SHARE) * room)));
  }

  /** The side is put where it was chosen to: kept on this device, and the
   *  page laid out anew around it. */
  private setDock(dock: Dock): void {
    if (dock === this.dock) return;
    this.dock = dock;
    try {
      localStorage.setItem(DOCK_KEPT, dock);
    } catch {
      // Not kept: the side stands here while the window is open.
    }
    this.place();
  }

  /** The switch that opens the side, drawn as the place it opens at. */
  private drawOpener(): void {
    const at = this.placed === "over" ? this.dock : this.placed;
    this.opener.innerHTML = `${glyph(`dock-${at}`, 18)}<span class="sp-toggle-dot" aria-hidden="true"></span>`;
  }

  /** Takes a width the side was dragged to, as its share of the page, or
   *  none to go back to half. */
  private resize(width: number | null): void {
    try {
      if (width === null) localStorage.removeItem(SHARE_KEPT);
      else localStorage.setItem(SHARE_KEPT, String(width / Math.max(1, this.room())));
    } catch {
      // Not kept: the share holds while the page is open.
    }
    this.fit();
  }

  /** The side's size for the room the page has now. */
  private fit(): void {
    const room = this.room();
    if (room <= 0) return;
    const name = this.down ? "--side-height" : "--side-width";
    const value = `${this.fitted(room)}px`;
    if (this.page.style.getPropertyValue(name) !== value) this.page.style.setProperty(name, value);
  }

  /** A conversation dragged in from the list opens beside this one, the
   *  side standing where it is let go: low on the page along the foot, to
   *  the left on the left, else on the right, as far as the page has room
   *  for it there. While one is dragged over the page, where it would open
   *  is shown. */
  private dropping(split: HTMLElement): void {
    const carries = (event: DragEvent) => !!event.dataTransfer?.types.includes(TALK_DRAG);
    const mark = split.querySelector<HTMLElement>(".sp-drop-mark")!;
    const done = () => {
      this.page.classList.remove("sp-dropping");
      delete this.page.dataset.dropping;
    };
    const zone = (event: DragEvent): Dock => {
      const box = split.getBoundingClientRect();
      const x = (event.clientX - box.left) / Math.max(1, box.width);
      const y = (event.clientY - box.top) / Math.max(1, box.height);
      return y > 0.62 ? "bottom" : x < 0.38 ? "left" : "right";
    };
    split.addEventListener("dragover", (event) => {
      if (!carries(event) || OVER.matches) return;
      event.preventDefault();
      event.dataTransfer!.dropEffect = "copy";
      this.page.classList.add("sp-dropping");
      const at = this.placement(zone(event));
      if (this.page.dataset.dropping === at) return;
      this.page.dataset.dropping = at;
      mark.innerHTML = glyph(`dock-${at === "over" ? this.dock : at}`, 18);
    });
    split.addEventListener("dragleave", (event) => {
      if (!split.contains(event.relatedTarget as Node | null)) done();
    });
    split.addEventListener("drop", (event) => {
      done();
      const carried = event.dataTransfer?.getData(TALK_DRAG);
      if (!carried) return;
      event.preventDefault();
      try {
        const address = JSON.parse(carried) as Address;
        if (!address?.resident || !address.machine) return;
        const self = this.context.seams.self;
        if (address.resident === self.resident && address.machine === self.machine) return;
        // Kept where it is seen to stand, not where it had no room.
        const at = this.placement(zone(event));
        if (at !== "over") this.setDock(at);
        this.openTalk(address);
      } catch {
        // Not a conversation: nothing to open.
      }
    });
    document.addEventListener("dragend", done);
    this.stops.push(() => document.removeEventListener("dragend", done));
  }

  /** A conversation's tab, its page made when the tab is first shown; false
   *  where the World cannot open it here. */
  private addTalk(address: Address, name?: string): boolean {
    const opened = this.context.seams.open(address);
    if (!opened) return false;
    let made: OpenedTalk | null = opened;
    this.pane.add(
      {
        id: talkId(address),
        name: name ?? opened.name,
        agent: opened.agent,
        main: () => {
          this.pane.remove(talkId(address));
          opened.main();
        },
      },
      () => {
        const talk = made ?? this.context.seams.open(address);
        made = null;
        if (!talk) return { element: document.createElement("div") };
        return { element: talk.element, leave: () => talk.leave(), roomWillChange: (by) => talk.roomWillChange?.(by) } satisfies PaneContent;
      },
    );
    const id = talkId(address);
    if (!this.tabGoals.has(id) && this.context.seams.goalOf) {
      this.tabGoals.set(id, this.context.seams.goalOf(address, (status) => this.pane.markGoal(id, status)));
    }
    return true;
  }

  /** One that works with the conversation, pressed in its whole or in the
   *  conversation: a colleague opens beside it to talk with, one of the
   *  host's agents to read. */
  private openNode(node: CollabNode): void {
    if (node.kind === "colleague" || node.kind === "side") {
      this.openTalk({ resident: node.id, machine: node.machine ?? this.context.seams.self.machine }, node.kind === "side" ? node.task : node.name);
      return;
    }
    // An agent a colleague sent is read in the colleague's conversation.
    if (!this.runs.some((run) => run.id === node.id)) {
      const colleague = this.colleagues.find((one) => contains(one, node.id));
      if (colleague) this.openNode(colleague);
      return;
    }
    // With no room for two, the side gives way to the row that sent it.
    if (this.placed === "over") {
      this.pane.close();
      this.context.jump(node.id);
      return;
    }
    const id = `agent:${node.id}`;
    if (!this.pane.tabs.some((tab) => tab.id === id)) {
      const run = this.runs.find((at) => at.id === node.id);
      if (!run) return;
      this.pane.add({ id, name: node.name, agent: node.agent }, () => {
        const box = this.context.agentBox(this.runs.find((at) => at.id === node.id) ?? run);
        this.boxes.set(node.id, (next) => box.update(next));
        const jump = document.createElement("button");
        jump.type = "button";
        jump.className = "text-button sp-jump";
        jump.textContent = this.context.seams.words.collab.jump;
        jump.addEventListener("click", () => this.context.jump(node.id));
        box.element.prepend(jump);
        return {
          element: box.element,
          leave: () => {
            this.boxes.delete(node.id);
            box.destroy();
          },
        };
      });
    }
    this.pane.show(id);
  }

  /** The tabs and whether the side was open, as this device last left them. */
  private restore(): void {
    let kept: KeptSide | null = null;
    try {
      kept = JSON.parse(localStorage.getItem(this.kept) ?? "null") as KeptSide | null;
    } catch {
      kept = null;
    }
    if (!kept) return;
    for (const tab of kept.tabs ?? []) this.addTalk({ resident: tab.resident, machine: tab.machine }, tab.name);
    if (kept.open && this.placed !== "over") {
      // Shown as it was left: no glide, the page is not yet on the screen.
      this.pane.show(kept.current);
      this.page.classList.add("sp-shown");
    } else if (kept.current) this.pane.select(kept.current);
  }

  /** Keeps the tabs and whether the side is open for the next time. */
  private keep(): void {
    if (this.restoring || !this.split) return;
    const tabs = this.pane.tabs
      .filter((tab) => tab.id.startsWith("talk:"))
      .map((tab) => {
        const [resident, machine] = tab.id.slice("talk:".length).split("@") as [string, string];
        return { resident, machine, name: tab.name };
      });
    const current = this.pane.shownTab?.startsWith("talk:") ? this.pane.shownTab : null;
    const kept: KeptSide = { open: this.pane.open, tabs, current };
    const written = !kept.open && tabs.length === 0 ? "" : JSON.stringify(kept);
    if (written === this.written) return;
    this.written = written;
    try {
      if (!written) localStorage.removeItem(this.kept);
      else localStorage.setItem(this.kept, written);
    } catch {
      // Not kept: the side opens shut next time.
    }
  }

  private plan(plan: PlanShown | null): void {
    this.planPage.show(plan ?? { source: null, version: 0, when: "" });
    this.overview?.showPlan(plan);
  }

  /** The goal: on its bar while it is worked toward, one done leaving it
   *  for the overview. */
  private goal(goal: GoalShown | null): void {
    this.shownGoal = goal;
    this.goalPage.show(goal);
    this.bar.show(goal && goal.status !== "complete" ? goal : null);
    this.overview?.showGoal(goal);
  }

  /** The goal opened in the overview, as its bar's last button asks. */
  private openGoal(): void {
    this.pane.show();
    this.pane.goHome();
    this.overview?.open("goal");
  }

  /** Who works with the conversation, drawn again when it changed: who is
   *  in it, how each stands, and the minutes they have worked. */
  private drawTeam(): void {
    // A conversation shown in another's side has no home to draw it in.
    if (!this.split) return;
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
    this.overview?.showTeam(nodes);
    if (fresh && !this.pane.open) {
      this.fresh = true;
      this.drawSwitch();
    }
  }

  /** The switch, and the page opening or shutting by the side: opening,
   *  the page is laid out shut first, so it opens from nothing; either way
   *  the conversation's column is carried to where it will stand. */
  private drawSwitch(): void {
    if (!this.split) return;
    const open = this.pane.open;
    const was = this.page.classList.contains("sp-shown");
    // Across, the conversation gives way beside the side; along the foot,
    // only its height changes.
    if (open !== was && (this.placed === "right" || this.placed === "left")) holdColumn(this.page, open ? -this.width() : this.width());
    if (open && !was) void this.page.offsetWidth;
    this.page.classList.toggle("sp-shown", open);
    if (open) this.fresh = false;
    this.opener.setAttribute("aria-pressed", String(open));
    this.opener.classList.toggle("news", this.pane.hasNews || this.fresh);
    this.opener.hidden = open && !this.pane.leaving;
    this.overview?.showTabs(this.pane.tabs.length > 0);
    this.keep();
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
