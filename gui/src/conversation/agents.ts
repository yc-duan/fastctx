// The host's own agents in a conversation (`world-gui.md`, 对话): Claude
// Code's subagents, the threads Codex spawns. Each is known by the host's
// id: which call started it and what it was asked, how it stands, the steps
// it took, and what it did, as far as the conversation has seen it.
//
// What it did is a conversation of its own: its events as they came, kept
// for the box that shows them (`agent-box.ts`). One seen from its start as
// it happened, with nothing missed since, is whole, and its box shows what
// came; any other is read from its own record when its box opens. What came
// live is let go once it no longer works and no box shows it, as its record
// holds it all. Only its steps and the step it takes now are kept apart, for
// its row and the tree of who works with the conversation
// (`side/collab-page.ts`).

import type { HostAgent, HostAgentEnd, HostAgentStart, SessionEvent, Tool, TurnHow } from "../world/events";
import type { Words } from "../words";

/** How an agent stands. Known only from what was kept, with no end told,
 *  it is unknown: it may have ended long ago, or run still. */
export type AgentState = "running" | "waiting" | "done" | "failed" | "stopped" | "unknown";

/** One step an agent took: a tool it called, when, and how it ended. */
export interface AgentStep {
  item: string;
  tool: Tool;
  at: number;
  /** Null while it runs. */
  ended: { at: number; failed: boolean } | null;
}

export class AgentRun {
  call: string | null = null;
  parent: string | null = null;
  kind: string | null = null;
  name: string | null = null;
  task: string | null = null;
  prompt: string | null = null;
  model: string | null = null;
  background = false;
  /** Seen from its start as it happened, with nothing missed since. */
  whole = false;
  /** Known when what was seen live broke off: whatever it did then is not
   *  all here. */
  broken = false;
  state: AgentState = "unknown";
  /** When it began and when its latest run ended, in milliseconds. */
  began: number | null = null;
  ended: number | null = null;
  /** Its steps in the order they began. */
  readonly steps: AgentStep[] = [];
  /** Its steps by their item. */
  readonly stepOf = new Map<string, AgentStep>();
  tokens: number | null = null;
  duration: number | null = null;
  tools: number | null = null;
  answer: string | null = null;
  reason: string | null = null;
  /** Its own events as they came, live. */
  readonly events: SessionEvent[] = [];
  /** How many boxes show what it did now. */
  held = 0;
  /** Moves whenever anything of it does. */
  stamp = 0;

  constructor(readonly id: string) {}

  /** Whether it still works: running, or waiting on something it owns. */
  get going(): boolean {
    return this.state === "running" || this.state === "waiting";
  }

  /** What it is called in a line: its task's words, or its name, or its
   *  kind. */
  label(): string {
    return this.task ?? this.name ?? this.kind ?? this.id;
  }

  /** Lets go of what came of it live, once it no longer works and no box
   *  shows it: a box opened again reads its record. */
  release(): void {
    if (this.going || this.held || !this.events.length) return;
    this.events.length = 0;
    this.whole = false;
  }
}

export class Agents {
  private readonly runs = new Map<string, AgentRun>();
  /** Tools the agents called, by the call: facts a run's job tells of one
   *  go to the agent that called it. */
  private readonly byTool = new Map<string, AgentRun>();
  /** Moves whenever any agent does. */
  stamp = 0;

  /** An agent started, or more is known of it. What a telling leaves out
   *  is not taken back from what another told. */
  started(agent: HostAgentStart, at: number): AgentRun {
    const run = this.run(agent.id);
    run.call = agent.call ?? run.call;
    run.parent = agent.parent ?? run.parent;
    run.kind = agent.kind ?? run.kind;
    run.name = agent.name ?? run.name;
    run.task = agent.task ?? run.task;
    run.prompt = agent.prompt ?? run.prompt;
    run.model = agent.model ?? run.model;
    run.background ||= agent.background;
    // Told as it started: what it does comes as it happens from here, and
    // what came of it a moment before was seen as it happened too.
    if (agent.live) {
      if (!run.broken) run.whole = true;
      if (run.state === "unknown") run.state = "running";
    }
    run.began ??= at;
    return this.touch(run);
  }

  /** One event of an agent's own conversation, as it happened. */
  event(id: string, event: SessionEvent): AgentRun {
    const run = this.run(id);
    run.events.push(event);
    const at = Date.parse(event.at);
    run.began ??= at;
    switch (event.event) {
      case "turn_started":
        run.state = "running";
        break;
      case "began":
      case "updated":
      case "finished": {
        const item = event.item;
        if (item.kind !== "tool") break;
        let step = run.stepOf.get(item.id);
        if (!step) {
          step = { item: item.id, tool: toolOf(item), at: item.started_at ? Date.parse(item.started_at) : at, ended: null };
          run.stepOf.set(item.id, step);
          run.steps.push(step);
          this.byTool.set(item.id, run);
        } else step.tool = toolOf(item);
        if (event.event === "finished" && item.status !== "running") {
          step.ended = { at, failed: item.status === "failed" || item.status === "declined" };
        }
        if (run.state !== "running" && event.event !== "finished") run.state = "running";
        break;
      }
      case "turn_ended": {
        // A run of its loop ended: its steps still open ended with it.
        for (const step of run.steps) if (step.ended === null) step.ended = { at, failed: false };
        this.end(run, event.end.how, at);
        run.answer = event.end.last_message ?? run.answer;
        if (event.end.duration_ms !== null) run.duration = event.end.duration_ms;
        if (event.end.how !== "completed") run.reason = event.end.reason ?? run.reason;
        run.release();
        break;
      }
      default:
        break;
    }
    return this.touch(run);
  }

  /** An agent's latest run ended, as its host told it. */
  ended(id: string, end: HostAgentEnd, at: number): AgentRun {
    const run = this.run(id);
    for (const step of run.steps) if (step.ended === null) step.ended = { at, failed: false };
    this.end(run, end.how, at);
    run.task ??= end.task ?? null;
    if (end.duration_ms !== undefined) run.duration = end.duration_ms;
    if (end.tokens !== undefined) run.tokens = end.tokens;
    if (end.tools !== undefined) run.tools = end.tools;
    if (end.answer !== undefined) run.answer = end.answer;
    if (end.reason !== undefined) run.reason = end.reason;
    run.release();
    return this.touch(run);
  }

  /** How the host lists its agents now (Claude Code's own list). An agent
   *  it says still works is shown so; one it says ended, ended. */
  listed(agents: readonly HostAgent[], at: number): void {
    for (const agent of agents) {
      const run = this.run(agent.id);
      run.kind ??= agent.type || null;
      run.task ??= agent.description || null;
      run.name ??= agent.name ?? null;
      run.parent ??= agent.parentId ?? null;
      const state = listedState(agent.status);
      if (state === null || state === run.state) continue;
      if (state === "running" || state === "waiting") {
        // A list read just before an end it already told says nothing new.
        if (run.ended === null || run.ended < at) run.state = state;
      } else this.end(run, state === "done" ? "completed" : state === "failed" ? "failed" : "interrupted", at);
      this.touch(run);
    }
  }

  /** What was seen live broke off for a while: none of the agents seen so
   *  far is whole from here. */
  broke(): void {
    for (const run of this.runs.values()) {
      run.broken = true;
      if (run.whole) this.touch(run).whole = false;
    }
  }

  get(id: string): AgentRun | undefined {
    return this.runs.get(id);
  }

  /** The agents a call started. */
  ofCall(call: string): AgentRun[] {
    return [...this.runs.values()].filter((run) => run.call === call);
  }

  /** An agent by its id or its name, as a call addresses it. */
  named(name: string): AgentRun | undefined {
    return this.runs.get(name) ?? [...this.runs.values()].find((run) => run.name === name);
  }

  /** The agent that called a tool. */
  ofTool(call: string): AgentRun | undefined {
    return this.byTool.get(call);
  }

  all(): AgentRun[] {
    return [...this.runs.values()];
  }

  get size(): number {
    return this.runs.size;
  }

  clear(): void {
    if (!this.runs.size) return;
    this.runs.clear();
    this.byTool.clear();
    this.stamp += 1;
  }

  private run(id: string): AgentRun {
    let run = this.runs.get(id);
    if (!run) {
      run = new AgentRun(id);
      this.runs.set(id, run);
    }
    return run;
  }

  private end(run: AgentRun, how: TurnHow, at: number): void {
    run.state = how === "completed" ? "done" : how === "failed" ? "failed" : "stopped";
    run.ended = at;
  }

  private touch(run: AgentRun): AgentRun {
    run.stamp += 1;
    this.stamp += 1;
    return run;
  }
}

/** How an agent that went on in the background ended, as the
 *  conversation is told it. */
export function toldSaid(end: HostAgentEnd, task: string | null, words: Words): string {
  if (end.how === "completed") return words.agents.done(task);
  return end.how === "failed" ? words.agents.failedTold(task) : words.agents.stoppedTold(task);
}

/** How a host's own list says an agent stands, as a run's state; null for
 *  a word it does not know. */
function listedState(status: string): AgentState | null {
  switch (status) {
    case "pending":
    case "running":
      return "running";
    case "waiting":
    case "idle":
      return "waiting";
    case "completed":
      return "done";
    case "failed":
      return "failed";
    case "killed":
    case "stopped":
      return "stopped";
    default:
      return null;
  }
}

function toolOf(item: Extract<SessionEvent, { event: "began" }>["item"]): Tool {
  const { kind: _kind, id: _id, started_at: _started, detail: _detail, ...tool } = item as Extract<typeof item, { kind: "tool" }>;
  return tool as Tool;
}
