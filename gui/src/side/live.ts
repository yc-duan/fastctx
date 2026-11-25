// The panel's seams for a conversation in the World, and for one in a
// specimen. Its colleagues are the residents the World lists as asked by
// it; its plan and goal come from the conversation's own records.

import { SENTINELS } from "../world/topics";
import { followAll } from "../app/numbers";
import { effect } from "../app/signals";
import { bareName, jobsOf, residentsOf } from "../app/store";
import { agentName, titleOf } from "../world/names";
import type { LiveFact } from "../plan/live";
import type { Store } from "../app/store";
import { modelName } from "../conversation/view";
import type { PlanShown, PageSeams } from "../plan/panel";
import type { PlanWords } from "../plan/words";
import { unwatch, type WorldNow } from "../world/bridge";
import { planRead, planWrite, watchPlan, type PlanAccount, type PlanPage } from "../world/plan";
import { goalPast, goalRead, goalShown, goalUpdate, watchGoal, type GoalChange, type GoalView } from "../world/goal";
import type { Branch, Tree } from "../world/collab-tree";
import { collaborationKey, followCollaboration, type CollaborationState } from "./collaboration-follow";
import type { Address } from "../world/wire";
import type { GoalStatus } from "./types";
import { goalSamples, pastSamples, teamSample } from "./samples";
import type { SideSeams } from "./seams";
import type { CollabNode, GoalShown, NodeState, PastGoal, WhereShown } from "./types";
import { whereOf } from "./where-live";
import type { SideWords } from "./words";
import { sentinelList, type SentinelSummary } from "../world/sentinels";
import { accountFeatures, featureAccount } from "../app/account-features";

/** The colleagues the conversation asked, as the World reports them, each
 *  with its goal. The agents a colleague sent of its own come with the tree
 *  and are left there: they are read in the colleague's conversation
 *  (`types.ts`, `CollabNode`). The conversation's own agents come from its
 *  own events, which the panel already follows. */
function treeNodes(tree: Tree, store: Store, now: number): CollabNode[] {
  const agentOf = (resident: string, fallback: "claude" | "codex"): "claude" | "codex" => {
    const found = residentsOf(store.world.peek()).find((r) => r.about.id === resident);
    return found ? (/codex/i.test(found.about.host.agent) ? "codex" : "claude") : fallback;
  };
  return tree.colleagues.map((branch) => branchNode(branch, agentOf(branch.address.resident, "codex"), store, now));
}

function branchNode(branch: Branch, agent: "claude" | "codex", store: Store, now: number): CollabNode {
  const found = residentsOf(store.world.peek()).find((r) => r.about.id === branch.address.resident);
  const busy = branch.now?.activity === "busy";
  const born = branch.born_at ? Date.parse(branch.born_at) : NaN;
  const since = branch.now?.since ? Date.parse(branch.now.since) : born;
  // A branch its machine could not tell about is not known to be done.
  const why = [...branch.unavailable, ...("Err" in branch.goal ? [branch.goal.Err] : [])].join("; ");
  const state: NodeState = branch.now?.trouble
    ? "failed"
    : !branch.now
      ? "unknown"
      : branch.now.waiting_for_user
        ? "waiting"
        : busy
          ? "working"
          : "done";
  return {
    id: branch.address.resident,
    kind: "colleague",
    agent,
    name: found?.about.model ? modelName(found.about.model) : (branch.label ?? agent),
    task: branch.label ?? branch.now?.title ?? "",
    state,
    why: why || null,
    goal: treeGoal(branch.goal),
    branchOf: "Ok" in branch.goal ? (branch.goal.Ok?.parent ?? null) : null,
    took: Number.isFinite(born) ? Math.max(0, (busy ? now : Number.isFinite(since) ? since : now) - born) : 0,
    began: Number.isFinite(born) ? born : null,
    ended: busy || state === "waiting" || !Number.isFinite(since) ? null : since,
    model: found?.about.model ?? null,
    steps: null,
    machine: branch.address.machine,
  };
}

/** A colleague's goal state, or none where it could not be read (Rust Result). */
function treeGoal(goal: Branch["goal"]): GoalStatus | null {
  return "Ok" in goal && goal.Ok ? goal.Ok.state : null;
}

/** How a conversation, a job or a sentinel a plan names stands now. */
export function planFact(
  world: WorldNow | null,
  id: string,
  words: PlanWords,
  took: (ms: number) => string,
  now: number,
  sentinels: SentinelSummary[] = [],
): LiveFact | null {
  const sentinel = sentinels.find((one) => one.definition.id === id);
  if (sentinel) {
    const label = sentinel.definition.condition || sentinel.plan;
    return { kind: "sense", label, state: words.live.sense(sentinel.phase), lit: sentinel.phase === "satisfied" || sentinel.phase === "unable" };
  }
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
  /** No goal is set here (`SideSeams.noGoal`). */
  noGoal?: boolean;
  /** The history its model works with (`SideSeams.history`). */
  history?: SideSeams["history"];
  /** A span of time, short. */
  took(ms: number): string;
  /** Opens what one of the world's ids names, from the plan. */
  openId(id: string): void;
  /** Where a computer's page is read. */
  machineLink(machine: string): string | null;
  /** Shows a folder of this computer in the system's own window. */
  reveal?: ((path: string) => void) | undefined;
}

/** The colleagues the conversation at `address` asked, as the World pushes
 *  them: who each is and how it stands, worked out as the tree does. Their
 *  goals come with the tree, while a page shows them. */
function worldColleagues(store: Store, address: Address, now: number): CollabNode[] {
  return residentsOf(store.world.peek()).flatMap((r): CollabNode[] => {
    const origin = r.about.origin;
    if (origin.kind !== "colleague" || origin.asked_by.resident !== address.resident || origin.asked_by.machine !== address.machine) return [];
    const agent = /codex/i.test(r.about.host.agent) ? "codex" : "claude";
    const busy = r.now?.activity === "busy";
    const born = Date.parse(r.about.born_at);
    const since = r.now?.since ? Date.parse(r.now.since) : born;
    // As the tree tells it (`branchNode`), so the two order alike.
    const state: NodeState = r.now?.trouble ? "failed" : !r.now ? "unknown" : r.now.waiting_for_user ? "waiting" : busy ? "working" : "done";
    return [
      {
        id: r.about.id,
        kind: "colleague",
        agent,
        name: r.about.model ? modelName(r.about.model) : (origin.label ?? agent),
        task: origin.label ?? r.now?.title ?? "",
        state,
        why: null,
        goal: null,
        branchOf: null,
        took: Number.isFinite(born) ? Math.max(0, (busy ? now : Number.isFinite(since) ? since : now) - born) : 0,
        // As the tree tells them (`branchNode`): the page orders by them,
        // and the two must order alike as one replaces the other.
        began: Number.isFinite(born) ? born : null,
        ended: busy || state === "waiting" || !Number.isFinite(since) ? null : since,
        steps: null,
        machine: r.about.machine,
      },
    ];
  });
}

/** The side conversations branched off the one at `address`, from the
 *  World's list: each named by its model, with its title or what it is. */
function sideNodes(store: Store, address: Address, words: SideWords, now: number): CollabNode[] {
  return residentsOf(store.world.peek())
    .filter((r) => r.about.side && r.about.forked_from?.resident === address.resident && r.about.forked_from.machine === address.machine)
    .map((r) => {
      const busy = r.now?.activity === "busy";
      const born = Date.parse(r.about.born_at);
      const since = Date.parse(r.now?.since ?? r.about.born_at);
      const state: NodeState = r.now?.trouble ? "failed" : busy ? "working" : "idle";
      return {
        id: r.about.id,
        kind: "side",
        agent: /codex/i.test(r.about.host.agent) ? "codex" : "claude",
        name: r.about.model ? modelName(r.about.model) : agentName(r.about.host.agent),
        task: titleOf(r) ?? words.sideTalk,
        state,
        goal: null,
        took: Number.isFinite(born) ? Math.max(0, (busy ? now : Number.isFinite(since) ? since : now) - born) : 0,
        began: Number.isFinite(born) ? born : null,
        ended: busy || !Number.isFinite(since) ? null : since,
        steps: null,
        machine: r.about.machine,
      };
    });
}

/** Applies a change to a goal against the version the user saw, with a
 *  fresh operation id; false when it changed elsewhere first. The watch
 *  brings the result. */
async function changeGoal(owner: string, seen: number | null, change: GoalChange): Promise<boolean> {
  const updated = await goalUpdate(owner, crypto.randomUUID(), seen, change);
  return updated.result === "goal";
}

/** How many times the user's change is made over a newer version of the
 *  goal before it is handed back. */
const GOAL_TRIES = 3;

/** Makes the user's change over whatever version of the goal stands now:
 *  met with a newer one, the goal is read again and the change, made from
 *  what the goal is now, applied over it (the user, 2026-11-18: "「已在别处
 *  修改」的规矩，推到计划、笔记、目标、哨兵设置"). False only where it still
 *  could not be made. */
async function changeGoalOver(owner: string, seen: GoalView | null, version: number | null, change: (view: GoalView | null) => GoalChange): Promise<boolean> {
  let view = seen;
  let at = version;
  for (let tries = 1; ; tries += 1) {
    if (await changeGoal(owner, at, change(view))) return true;
    if (tries >= GOAL_TRIES) return false;
    view = await goalRead(owner);
    at = view.version;
  }
}

/** The account a conversation works for, where it is one, by its hub's name. */
function accountOf(store: Store, address: Address): PlanAccount | null {
  const account = featureAccount(store.whole.peek(), address);
  return account ? { hub: account.hubName, account: account.account } : null;
}

/** The plan page's shape, read from the World's stored plan. */
function planShownOf(page: PlanPage, words: SideWords, now: number): PlanShown | null {
  const at = page.metadata;
  if (!at || at.deleted || !page.body) return null;
  return { source: page.body.markdown, version: at.version, when: words.goal.at(Date.parse(at.at), now) };
}

export function liveSide(options: LiveSide): SideSeams {
  const owner = options.address.resident;
  const at = () => accountOf(options.store, options.address);
  // The sentinels it left, which their homes are asked about, not followed.
  let sentinels: SentinelSummary[] = [];
  const plan: Omit<PageSeams, "answer"> = {
    facts: (id) => planFact(options.store.world.peek(), id, options.planWords, options.took, Date.now(), sentinels),
    open: (id) => options.openId(id),
    save: async (source, expected) => {
      const result = await planWrite(owner, expected, source, at(), bareName(options.address.machine));
      return result.result === "page" ? { ok: true } : { ok: false, current: result.current?.version ?? null };
    },
  };
  // The goal as last seen, for what a change is to: its version even when
  // there is no goal, since a cleared one keeps it.
  let current: GoalView | null = null;
  const seen = (version: number | undefined) => version ?? current?.version ?? null;
  return {
    words: options.words,
    self: options.address,
    ...(options.noGoal ? { noGoal: true } : {}),
    ...(options.history ? { history: options.history } : {}),
    where: (show) => effect(() => show(whereOf(options.store.world.get(), options.address, options.machineLink))),
    ...(options.reveal ? { reveal: options.reveal } : {}),
    plan: {
      words: options.planWords,
      seams: plan,
      watch: (show: (plan: PlanShown | null) => void) => {
        let alive = true;
        let watching: number | null = null;
        let shown: PlanShown | null = null;
        const naming = () => accountFeatures.talk(options.store.whole.peek(), options.address).sentinels
          && /\bs-[0-9a-z]{6,16}\b/.test(shown?.source ?? "");
        const refresh = () => {
          planRead(owner, at(), bareName(options.address.machine))
            .then((page) => {
              if (!alive) return;
              show((shown = planShownOf(page, options.words, Date.now())));
              stopAsking.refresh();
              if (naming()) void ask().catch(() => {});
            })
            .catch((error) => console.error("the plan could not be read", error));
        };
        // One question at a time: one asked for while a reading is on its
        // way, which may have read before what moved it, is asked once that
        // one answers, and the later answers both.
        let reading: Promise<void> | null = null;
        let again = false;
        const ask = (): Promise<void> => {
          if (reading) {
            again = true;
            return reading;
          }
          // Begun after it is kept, so even one that fails at once lets go.
          reading = Promise.resolve().then(async () => {
            try {
              do {
                again = false;
                const listed = await sentinelList(options.address, at() ?? undefined);
                const changed = JSON.stringify(listed.map((one) => [one.definition.id, one.phase])) !== JSON.stringify(sentinels.map((one) => [one.definition.id, one.phase]));
                sentinels = listed;
                if (alive && changed && shown && /\bs-[0-9a-z]{6,16}\b/.test(shown.source ?? "")) show(shown);
              } while (again && alive);
            } finally {
              // Let go as the last look at `again` is taken, so a question
              // after it begins a reading of its own.
              reading = null;
            }
          });
          return reading;
        };
        // Their homes are asked as the plan names them, and again as they
        // change (`app/numbers.ts`); a home that tells nothing, every half minute.
        const stopAsking = followAll(options.store.world, "sentinels", SENTINELS, () => naming() ? ask() : undefined, () => naming() ? 30_000 : false);
        refresh();
        watchPlan(
          owner,
          (piece) => {
            if (piece.kind === "changed") refresh();
          },
          at(),
        )
          .then((id) => {
            if (alive) watching = id;
            else void unwatch(id);
          })
          .catch((error) => console.error("the plan could not be watched", error));
        return () => {
          alive = false;
          stopAsking();
          if (watching != null) void unwatch(watching);
        };
      },
    },
    goal: {
      watch: (show: (goal: GoalShown | null) => void) => {
        let alive = true;
        let watching: number | null = null;
        watchGoal(owner, (piece) => {
          if (piece.kind !== "snapshot" || !alive) return;
          current = piece.view;
          show(goalShown(piece.view));
        })
          .then(({ id, view }) => {
            if (!alive) {
              void unwatch(id);
              return;
            }
            watching = id;
            current = view;
            show(goalShown(view));
          })
          .catch((error) => console.error("the goal could not be watched", error));
        return () => {
          alive = false;
          if (watching != null) void unwatch(watching);
        };
      },
      // Words for a goal done set a new one; for any other, they replace its words.
      set: (objective, version, standing) =>
        changeGoalOver(owner, current, seen(version), (view) => {
          const goal = view?.goal;
          return goal && goal.status !== "complete" ? { change: "edit", objective } : { change: "open", objective, ...(standing ? { standing: true } : {}) };
        }),
      past: () => goalPast(owner),
      pause: (version) => changeGoalOver(owner, current, seen(version), () => ({ change: "pause" })),
      resume: (version) => changeGoalOver(owner, current, seen(version), () => ({ change: "resume" })),
      clear: (version) => changeGoalOver(owner, current, seen(version), () => ({ change: "clear" })),
    },
    colleagues: (show) => {
      let detail = false;
      let key: string | null = null;
      let release: (() => void) | null = null;
      let state: CollaborationState = { tree: null, broken: null };
      const draw = () => {
        const now = Date.now();
        const told = state.tree ? treeNodes(state.tree, options.store, now) : null;
        const colleagues = told
          ? state.broken === null ? told : told.map((node) => ({ ...node, state: "unknown" as const, why: state.broken }))
          : worldColleagues(options.store, options.address, now);
        show([...colleagues, ...sideNodes(options.store, options.address, options.words, now)]);
      };
      const follow = (next: string | null) => {
        release?.();
        release = null;
        key = next;
        state = { tree: null, broken: null };
        if (detail) release = followCollaboration(options.store, options.address, (told) => {
          state = told;
          draw();
        });
      };
      const stop = effect(() => {
        const world = options.store.whole.get();
        options.store.world.get();
        if (detail) {
          const next = collaborationKey(world, options.address);
          if (next !== key) follow(next);
        }
        draw();
      });
      return {
        stop: () => {
          stop();
          release?.();
          release = null;
        },
        detail: (shown) => {
          if (shown === detail) return;
          detail = shown;
          follow(collaborationKey(options.store.whole.peek(), options.address));
          draw();
        },
      };
    },
  };
}

/** A specimen conversation's side: a plan, a goal in the state `goal`
 *  names (under way at first), a team, and its own history where it is in
 *  no project. */
export function specimenSide(
  words: SideWords,
  planWords: PlanWords,
  plan: PlanShown & { facts: PageSeams["facts"] },
  english: boolean,
  side: { self: Address; goal?: string | null; history?: SideSeams["history"]; where: () => WhereShown | null },
): SideSeams {
  let goal: GoalShown | null = side.goal === "none" ? null : (goalSamples(english, Date.now())[side.goal ?? "active"] ?? null);
  const past: PastGoal[] = pastSamples(english, Date.now());
  // A goal done or cleared joins the earlier ones, as the World keeps it.
  const retire = () => {
    if (goal) past.push({ id: goal.id ?? `goal-${past.length}`, objective: goal.objective, done: goal.status === "complete", standing: goal.standing === true, created: goal.since - goal.worked, closed: Date.now(), statement: goal.statement ?? null });
  };
  const shows = new Set<(goal: GoalShown | null) => void>();
  const tell = (next: GoalShown | null) => {
    goal = next;
    for (const show of shows) show(goal);
  };
  const wait = () => new Promise<void>((resolve) => setTimeout(resolve, 300));
  return {
    words,
    self: side.self,
    ...(side.history ? { history: side.history } : {}),
    where: (show) => {
      show(side.where());
      return () => {};
    },
    plan: {
      words: planWords,
      seams: { facts: plan.facts, save: async () => ({ ok: true }) },
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
      set: async (objective, _seen, standing) => {
        await wait();
        if (goal && goal.status !== "complete") tell({ ...goal, objective, version: goal.version + 1 });
        else {
          retire();
          tell({ objective, version: (goal?.version ?? 0) + 1, status: "active", worked: 0, since: Date.now(), id: `goal-${Date.now()}`, ...(standing ? { standing: true } : {}) });
        }
        return true;
      },
      past: async () => past.map((one) => ({ ...one })),
      pause: async () => {
        await wait();
        if (goal) tell({ ...goal, status: "paused", worked: goal.worked + Date.now() - goal.since, since: Date.now(), paused: { by: "user", until: null } });
        return true;
      },
      resume: async () => {
        await wait();
        if (goal) tell({ ...goal, status: "active", since: Date.now(), paused: null });
        return true;
      },
      clear: async () => {
        await wait();
        retire();
        tell(null);
        return true;
      },
    },
    colleagues: (show) => {
      show(teamSample(english));
      return { stop: () => {}, detail: () => {} };
    },
  };
}
