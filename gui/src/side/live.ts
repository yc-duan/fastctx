// The panel's seams for a conversation in the World, and for one in a
// specimen. Its colleagues are the residents the World lists as asked by
// it; its plan and goal come from the conversation's own records.

import { effect } from "../app/signals";
import { residentsOf } from "../app/store";
import type { Store } from "../app/store";
import { modelName } from "../conversation/view";
import type { PlanShown, PageSeams } from "../plan/panel";
import type { PlanWords } from "../plan/words";
import type { WorldNow } from "../world/bridge";
import type { Address } from "../world/wire";
import type { ResidentNow } from "../world/world";
import { goalSamples, teamSample } from "./samples";
import type { SideSeams } from "./seams";
import type { CollabNode, GoalShown } from "./types";
import type { SideWords } from "./words";

/** The colleagues a conversation asked, as the World lists them now. */
export function colleaguesOf(world: WorldNow | null, address: Address, now: number): CollabNode[] {
  return residentsOf(world)
    .filter((r) => r.about.origin.kind === "colleague" && r.about.origin.asked_by.resident === address.resident && r.about.origin.asked_by.machine === address.machine)
    .map((r) => colleagueOf(r, now));
}

function colleagueOf(r: ResidentNow, now: number): CollabNode {
  const origin = r.about.origin.kind === "colleague" ? r.about.origin : null;
  const busy = r.now?.activity === "busy";
  const born = Date.parse(r.about.born_at);
  return {
    id: r.about.id,
    kind: "colleague",
    agent: /codex/i.test(r.about.host.agent) ? "codex" : "claude",
    name: r.about.model ? modelName(r.about.model) : r.about.host.agent,
    task: origin?.label ?? r.now?.title ?? "",
    state: r.now?.trouble ? "failed" : busy ? "working" : "done",
    goal: null,
    took: Number.isFinite(born) ? Math.max(0, (busy ? now : Date.parse(r.now?.since ?? r.about.born_at)) - born) : 0,
    steps: null,
    machine: r.about.machine,
    children: [],
  };
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
}

/** What the World does not tell yet: the plan and the goal arrive with the
 *  nodes that keep them. */
const notYet = () => Promise.reject(new Error("this version of the World keeps no goal"));

export function liveSide(options: LiveSide): SideSeams {
  const plan: PageSeams = { facts: () => null, save: () => {}, answer: () => {} };
  return {
    words: options.words,
    ...(options.bare ? { bare: true } : {}),
    plan: {
      words: options.planWords,
      seams: plan,
      watch: (show: (plan: PlanShown | null) => void) => {
        show(null);
        return () => {};
      },
    },
    goal: {
      watch: (show: (goal: GoalShown | null) => void) => {
        show(null);
        return () => {};
      },
      set: notYet,
      pause: notYet,
      resume: notYet,
      clear: notYet,
    },
    colleagues: (show) => effect(() => show(colleaguesOf(options.store.world.get(), options.address, Date.now()))),
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
      seams: { facts: plan.facts, save: () => {}, answer: () => {} },
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
