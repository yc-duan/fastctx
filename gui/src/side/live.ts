// The panel's seams for a conversation in the World, and for one in a
// specimen. Its colleagues are the residents the World lists as asked by
// it; its plan and goal come from the conversation's own records.

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
import { goalPast, goalShown, goalUpdate, watchGoal, type GoalChange, type GoalView } from "../world/goal";
import { collaborationTree, type Branch, type NativeAgent, type Tree } from "../world/collab-tree";
import type { Address } from "../world/wire";
import type { GoalStatus } from "./types";
import { goalSamples, pastSamples, teamSample } from "./samples";
import type { OpenedTalk, SideSeams } from "./seams";
import type { CollabNode, GoalShown, NodeState, PastGoal } from "./types";
import type { SideWords } from "./words";
import { sentinelList, type SentinelSummary } from "../world/sentinels";

/** The colleagues the conversation asked, as the World reports them, each
 *  with its goal and the agents it sent. The conversation's own agents come
 *  from its own events, which the panel already follows. */
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
  const state: NodeState = branch.now?.trouble ? "failed" : !branch.now ? "unknown" : busy ? "working" : "done";
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
  /** A conversation shown in another's side: only its goal's bar is drawn. */
  bare?: boolean;
  /** No goal is set here (`SideSeams.noGoal`). */
  noGoal?: boolean;
  /** Another conversation, to talk with it beside this one, or as the
   *  page's own. */
  open(address: Address): OpenedTalk | null;
  go(address: Address): void;
  /** A span of time, short. */
  took(ms: number): string;
  /** Opens what one of the world's ids names, from the plan. */
  openId(id: string): void;
  /** Branches off a side conversation; absent in another's side. */
  fork?: SideSeams["fork"];
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
        steps: null,
        machine: r.about.machine,
        children: [],
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

/** The account a conversation works for, where it is one, by its hub's name. */
function accountOf(store: Store, address: Address): PlanAccount | null {
  const world = store.world.peek();
  const acting = residentsOf(world).find((r) => r.about.id === address.resident)?.about.acting;
  if (acting?.kind !== "accounts" || acting.accounts.length !== 1) return null;
  const [one] = acting.accounts;
  const hub = world?.accounts?.hubs.find((seen) => seen.hub === one!.hub)?.name;
  return hub ? { hub, account: one!.account } : null;
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
    ...(options.bare ? { bare: true } : {}),
    ...(options.noGoal ? { noGoal: true } : {}),
    plan: {
      words: options.planWords,
      seams: plan,
      watch: (show: (plan: PlanShown | null) => void) => {
        let alive = true;
        let watching: number | null = null;
        let shown: PlanShown | null = null;
        const refresh = () => {
          planRead(owner, at(), bareName(options.address.machine))
            .then((page) => {
              if (alive) show((shown = planShownOf(page, options.words, Date.now())));
            })
            .catch((error) => console.error("the plan could not be read", error));
        };
        // A sentinel the plan names says how it stands as its home last
        // answered, asked one question at a time.
        let reading = false;
        const ask = () => {
          if (reading) return;
          reading = true;
          void sentinelList(options.address)
            .then(
              (listed) => {
                const changed = JSON.stringify(listed.map((one) => [one.definition.id, one.phase])) !== JSON.stringify(sentinels.map((one) => [one.definition.id, one.phase]));
                sentinels = listed;
                if (alive && changed && shown && /\bs-[0-9a-z]{6,16}\b/.test(shown.source ?? "")) show(shown);
              },
              () => {},
            )
            .finally(() => {
              reading = false;
            });
        };
        void ask();
        const asking = window.setInterval(() => void ask(), 30_000);
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
          clearInterval(asking);
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
      set: (objective, version, standing) => {
        const goal = current?.goal;
        const change: GoalChange =
          goal && goal.status !== "complete" ? { change: "edit", objective } : { change: "open", objective, ...(standing ? { standing: true } : {}) };
        return changeGoal(owner, seen(version), change);
      },
      past: () => goalPast(owner),
      pause: (version) => changeGoal(owner, seen(version), { change: "pause" }),
      resume: (version) => changeGoal(owner, seen(version), { change: "resume" }),
      clear: (version) => changeGoal(owner, seen(version), { change: "clear" }),
    },
    colleagues: (show) => {
      let alive = true;
      let inflight = false;
      let again = false;
      let last: CollabNode[] = [];
      const refresh = () => {
        if (inflight) {
          again = true;
          return;
        }
        inflight = true;
        again = false;
        collaborationTree(owner, options.address.machine)
          .then((tree) => {
            const now = Date.now();
            if (alive) show((last = [...treeNodes(tree, options.store, now), ...sideNodes(options.store, options.address, options.words, now)]));
          })
          .catch((error: unknown) => {
            // What was known stays, no longer said to stand as it did.
            const why = error instanceof Error ? error.message : String(error);
            if (alive) show(last.map((node) => ({ ...node, state: "unknown", why })));
          })
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
    open: (address) => options.open(address),
    go: (address) => options.go(address),
    ...(options.fork ? { fork: options.fork } : {}),
    goalOf: (address, show) => {
      let alive = true;
      let watching: number | null = null;
      watchGoal(address.resident, (piece) => {
        if (alive && piece.kind === "snapshot") show(goalShown(piece.view)?.status ?? null);
      })
        .then(({ id, view }) => {
          if (!alive) return void unwatch(id);
          watching = id;
          show(goalShown(view)?.status ?? null);
        })
        .catch(() => {});
      return () => {
        alive = false;
        if (watching != null) void unwatch(watching);
      };
    },
  };
}

/** A specimen conversation's side: a plan, a goal in the state `goal`
 *  names (under way at first) and a team; other specimen conversations open
 *  beside it through `open`. */
export function specimenSide(
  words: SideWords,
  planWords: PlanWords,
  plan: PlanShown & { facts: PageSeams["facts"] },
  english: boolean,
  side: { self: Address; open(address: Address): OpenedTalk | null; go(address: Address): void; goal?: string | null; bare?: boolean; fork?: SideSeams["fork"] },
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
    ...(side.bare ? { bare: true } : {}),
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
      show(teamSample(english).filter((node) => node.kind === "colleague"));
      return () => {};
    },
    open: (address) => side.open(address),
    go: (address) => side.go(address),
    ...(side.fork ? { fork: side.fork } : {}),
    // A specimen colleague works at its goal.
    goalOf: (_address, show) => {
      show("active");
      return () => {};
    },
  };
}
