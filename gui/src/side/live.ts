// The panel's seams for a conversation in the World, and for one in a
// specimen. Its colleagues are the residents the World lists as asked by
// it; its plan and goal come from the conversation's own records.

import { effect } from "../app/signals";
import { jobsOf, residentsOf } from "../app/store";
import { titleOf } from "../world/names";
import type { LiveFact } from "../plan/live";
import type { Store } from "../app/store";
import { modelName } from "../conversation/view";
import type { PlanShown, PageSeams } from "../plan/panel";
import type { PlanWords } from "../plan/words";
import { unwatch, type WorldNow } from "../world/bridge";
import { planRead, planWrite, watchPlan, type PlanPage } from "../world/plan";
import { goalRead, goalShown, goalUpdate, watchGoal, type GoalChange, type GoalView } from "../world/goal";
import { collaborationTree, type Branch, type NativeAgent, type Tree } from "../world/collab-tree";
import type { Address } from "../world/wire";
import type { GoalStatus } from "./types";
import { goalSamples, teamSample } from "./samples";
import type { SideSeams } from "./seams";
import type { CollabNode, GoalShown, NodeState } from "./types";
import type { SideWords } from "./words";

/** The conversation's collaboration tree, as the World reports it: the
 *  colleagues it asked, each with its goal and the agents it sent, and the
 *  conversation's own agents on the root beside them. */
function treeNodes(tree: Tree, store: Store, address: Address, now: number): CollabNode[] {
  const agentOf = (resident: string, fallback: "claude" | "codex"): "claude" | "codex" => {
    const found = residentsOf(store.world.peek()).find((r) => r.about.id === resident);
    return found ? (/codex/i.test(found.about.host.agent) ? "codex" : "claude") : fallback;
  };
  const root = agentOf(address.resident, "claude");
  return [
    ...tree.colleagues.map((branch) => branchNode(branch, agentOf(branch.address.resident, "codex"), store, now)),
    ...tree.conversation.agents.map((agent) => agentNode(agent, root, now)),
  ];
}

function branchNode(branch: Branch, agent: "claude" | "codex", store: Store, now: number): CollabNode {
  const found = residentsOf(store.world.peek()).find((r) => r.about.id === branch.address.resident);
  const busy = branch.now?.activity === "busy";
  const born = branch.born_at ? Date.parse(branch.born_at) : NaN;
  const since = branch.now?.since ? Date.parse(branch.now.since) : born;
  return {
    id: branch.address.resident,
    kind: "colleague",
    agent,
    name: found?.about.model ? modelName(found.about.model) : (branch.label ?? agent),
    task: branch.label ?? branch.now?.title ?? "",
    state: branch.now?.trouble ? "failed" : busy ? "working" : "done",
    goal: treeGoal(branch.goal),
    took: Number.isFinite(born) ? Math.max(0, (busy ? now : Number.isFinite(since) ? since : now) - born) : 0,
    steps: null,
    machine: branch.address.machine,
    children: branch.agents.map((child) => agentNode(child, agent, now)),
  };
}

function agentNode(agent: NativeAgent, parent: "claude" | "codex", now: number): CollabNode {
  const model = agent.model ?? "";
  const kind: "claude" | "codex" = /codex|gpt/i.test(model) ? "codex" : /claude/i.test(model) ? "claude" : parent;
  const started = agent.started_at ? Date.parse(agent.started_at) : NaN;
  return {
    id: agent.id,
    kind: "subagent",
    agent: kind,
    name: agent.name ?? agent.id,
    task: agent.task ?? "",
    state: agentState(agent),
    goal: null,
    took: agent.duration_ms ?? (Number.isFinite(started) ? Math.max(0, now - started) : 0),
    steps: null,
    children: agent.children.map((child) => agentNode(child, kind, now)),
  };
}

/** A native agent's state, `unknown` when only a historical start is known. */
function agentState(agent: NativeAgent): NodeState {
  if (agent.state == null) return agent.ended_at ? "done" : "unknown";
  const state = agent.state.toLowerCase();
  if (/busy|run|active|work/.test(state)) return "working";
  if (/fail|error/.test(state)) return "failed";
  if (/stop|cancel/.test(state)) return "stopped";
  if (/done|complete|idle|rest|end/.test(state) || agent.ended_at) return "done";
  return "unknown";
}

/** A colleague's goal state, or none where it could not be read (Rust Result). */
function treeGoal(goal: Branch["goal"]): GoalStatus | null {
  return "Ok" in goal && goal.Ok ? goal.Ok.state : null;
}

/** How a conversation or a job a plan names stands now. */
export function planFact(world: WorldNow | null, id: string, words: PlanWords, took: (ms: number) => string, now: number): LiveFact | null {
  const resident = residentsOf(world).find((r) => r.about.id === id);
  if (resident) {
    const origin = resident.about.origin.kind === "colleague" ? resident.about.origin.label : null;
    const busy = resident.now?.activity === "busy";
    const since = Date.parse(resident.now?.since ?? resident.about.born_at);
    return {
      kind: "resident",
      agent: /codex/i.test(resident.about.host.agent) ? "codex" : "claude",
      label: origin ?? titleOf(resident) ?? id,
      state: resident.now?.trouble ? words.live.failed : busy ? words.live.working(took(now - since)) : words.live.resting,
      lit: busy,
    };
  }
  const job = jobsOf(world).find((one) => one.job === id);
  if (job) return { kind: "job", label: job.command, state: words.live.running(took(now - Date.parse(job.started_at))), lit: true };
  return null;
}

export interface LiveSide {
  words: SideWords;
  planWords: PlanWords;
  store: Store;
  address: Address;
  /** A colleague's page, for its own panel: only its goal's bar is drawn. */
  bare?: boolean;
  /** A colleague's conversation, to talk with it in the panel. */
  open(address: Address): { element: HTMLElement; leave(): void };
  /** A span of time, short. */
  took(ms: number): string;
  /** Opens what one of the world's ids names, from the plan. */
  openId(id: string): void;
}

/** What the World does not tell yet: the plan and the goal arrive with the
 *  nodes that keep them. */
/** Applies a change to a goal: reads the current version for the comparison,
 *  then writes it with a fresh operation id. The watch brings the result. */
async function changeGoal(owner: string, of: (view: GoalView) => GoalChange): Promise<void> {
  const view = await goalRead(owner);
  await goalUpdate(owner, crypto.randomUUID(), view.version, of(view));
}

/** The plan page's shape, read from the World's stored plan. */
function planShownOf(page: PlanPage, words: SideWords, now: number): PlanShown | null {
  const at = page.metadata;
  if (!at || at.deleted || !page.body) return null;
  return { source: page.body.markdown, version: at.version, when: words.goal.at(Date.parse(at.at), now) };
}

export function liveSide(options: LiveSide): SideSeams {
  const owner = options.address.resident;
  const plan: PageSeams = {
    facts: (id) => planFact(options.store.world.peek(), id, options.planWords, options.took, Date.now()),
    open: (id) => options.openId(id),
    save: async (source, expected) => {
      const result = await planWrite(owner, expected, source);
      return result.result === "page" ? { ok: true } : { ok: false, current: result.current?.version ?? null };
    },
    answer: () => {},
  };
  return {
    words: options.words,
    ...(options.bare ? { bare: true } : {}),
    plan: {
      words: options.planWords,
      seams: plan,
      watch: (show: (plan: PlanShown | null) => void) => {
        let alive = true;
        let watching: number | null = null;
        const refresh = () => {
          planRead(owner)
            .then((page) => {
              if (alive) show(planShownOf(page, options.words, Date.now()));
            })
            .catch((error) => console.error("the plan could not be read", error));
        };
        refresh();
        watchPlan(owner, (piece) => {
          if (piece.kind === "changed") refresh();
        })
          .then((id) => {
            if (alive) watching = id;
            else void unwatch(id);
          })
          .catch((error) => console.error("the plan could not be watched", error));
        return () => {
          alive = false;
          if (watching != null) void unwatch(watching);
        };
      },
    },
    goal: {
      watch: (show: (goal: GoalShown | null) => void) => {
        let alive = true;
        let watching: number | null = null;
        watchGoal(owner, (piece) => {
          if (piece.kind === "snapshot" && alive) show(goalShown(piece.view, Date.now()));
        })
          .then(({ id, view }) => {
            if (!alive) {
              void unwatch(id);
              return;
            }
            watching = id;
            show(goalShown(view, Date.now()));
          })
          .catch((error) => console.error("the goal could not be watched", error));
        return () => {
          alive = false;
          if (watching != null) void unwatch(watching);
        };
      },
      set: (objective) => changeGoal(owner, (view) => ({ change: view.goal ? "edit" : "open", objective })),
      pause: () => changeGoal(owner, () => ({ change: "pause" })),
      resume: () => changeGoal(owner, () => ({ change: "resume" })),
      clear: () => changeGoal(owner, () => ({ change: "clear" })),
    },
    colleagues: (show) => {
      let alive = true;
      let inflight = false;
      let again = false;
      const refresh = () => {
        if (inflight) {
          again = true;
          return;
        }
        inflight = true;
        again = false;
        collaborationTree(owner, options.address.machine)
          .then((tree) => {
            if (alive) show(treeNodes(tree, options.store, options.address, Date.now()));
          })
          .catch((error) => console.error("the collaboration tree could not be read", error))
          .finally(() => {
            inflight = false;
            if (alive && again) refresh();
          });
      };
      const stop = effect(() => {
        options.store.world.get();
        refresh();
      });
      return () => {
        alive = false;
        stop();
      };
    },
    colleague: (node) => options.open({ resident: node.id, machine: node.machine ?? options.address.machine }),
  };
}

/** A specimen conversation's panel: a plan, a goal under way and a team. */
export function specimenSide(words: SideWords, planWords: PlanWords, plan: PlanShown & { facts: PageSeams["facts"] }, english: boolean): SideSeams {
  let goal: GoalShown | null = goalSamples(english, Date.now()).active ?? null;
  const shows = new Set<(goal: GoalShown | null) => void>();
  const tell = (next: GoalShown | null) => {
    goal = next;
    for (const show of shows) show(goal);
  };
  const wait = () => new Promise<void>((resolve) => setTimeout(resolve, 300));
  return {
    words,
    plan: {
      words: planWords,
      seams: { facts: plan.facts, save: async () => ({ ok: true }), answer: () => {} },
      watch: (show) => {
        show(plan);
        return () => {};
      },
    },
    goal: {
      watch: (show) => {
        shows.add(show);
        show(goal);
        return () => shows.delete(show);
      },
      set: async (objective) => {
        await wait();
        tell(goal ? { ...goal, objective, version: goal.version + 1 } : { objective, version: 1, status: "active", worked: 0, since: Date.now() });
      },
      pause: async () => {
        await wait();
        if (goal) tell({ ...goal, status: "paused", worked: goal.worked + Date.now() - goal.since, since: Date.now(), paused: { by: "user", until: null } });
      },
      resume: async () => {
        await wait();
        if (goal) tell({ ...goal, status: "active", since: Date.now(), paused: null });
      },
      clear: async () => {
        await wait();
        tell(null);
      },
    },
    colleagues: (show) => {
      show(teamSample(english).filter((node) => node.kind === "colleague"));
      return () => {};
    },
    colleague: () => null,
  };
}
