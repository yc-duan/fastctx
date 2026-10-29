// The update view in the specimen: every machine of the specimen's World
// with a release it runs and a stage of its update, a few of each kind, and
// scheduling and the network's policy kept for the visit.

import type { WorldNow } from "../world/bridge";
import type { Due, MachineUpdate, Manifest, Policy, UpdateActions, UpdateStage, UpdatesView, UpdatesViewCall, UpdatesViewReply } from "../world/updates";
import { machinesOf } from "../app/reach";

const v = (n: number) => `1.0.1-beta.${n}`;

function manifest(version: string): Manifest {
  return { version, notes: "", pub_date: "2026-10-20T08:00:00Z", platforms: { "windows-x86_64-nsis": { url: "", signature: "", size: 63_000_000 } } };
}

const allowed = { allowed: true, reason: null };
const actions = (online: boolean): UpdateActions => ({
  update: allowed,
  choose: allowed,
  settings: online ? allowed : { allowed: false, reason: "offline" },
  schedule: allowed,
  cancel: allowed,
  repair: allowed,
});

export function specimenUpdateView(world: () => WorldNow | null): (hub: string, account: string, call: UpdatesViewCall) => Promise<UpdatesViewReply> {
  const due = new Map<string, Due>([["lab-a-01", { version: v(8), by: "me", at: "2026-10-20T09:00:00Z" }]]);
  const policy: Policy = { minimum: v(6), managed_auto: true };
  const auto = new Map<string, boolean>();
  const stageOf = (name: string, here: boolean): { running: string; stage: UpdateStage } => {
    if (here) return { running: v(8), stage: { kind: "idle" } };
    switch (name) {
      case "server":
        return { running: v(7), stage: { kind: "idle" } };
      case "lab":
        return { running: v(7), stage: { kind: "fetching", version: v(8), via: "github", received: 27_000_000, total: 63_000_000 } };
      case "laptop":
        return { running: v(7), stage: { kind: "waiting_idle", version: v(8) } };
      case "lab-a-02":
        return { running: v(5), stage: { kind: "failed", version: v(8), problem: { reason: { kind: "sources", sources: [] }, why: "github: connection reset; network: the hub holds no package yet" } } };
      case "lab-a-01":
        return { running: v(6), stage: { kind: "idle" } };
      case "studio":
        return { running: v(6), stage: { kind: "idle" } };
      default:
        return { running: v(5), stage: { kind: "idle" } };
    }
  };
  const view = (): UpdatesView => {
    const now = world();
    const machines: MachineUpdate[] = [];
    for (const machine of now ? machinesOf(now) : []) {
      const { running, stage } = stageOf(machine.name, machine.here);
      const online = machine.online !== false;
      machines.push({
        machine: machine.name,
        owner: machine.on[0]?.owner ?? "me",
        group: null,
        kind: machine.name.startsWith("lab-a") ? "managed" : machine.here ? "app" : "headless",
        online,
        reported: "2026-10-20T09:00:00Z",
        kernel: running,
        app: machine.here ? { version: running, target: "windows-x86_64-nsis" } : null,
        target: "fastctx-x86_64-pc-windows-msvc",
        channel: "beta",
        auto: auto.get(machine.name) ?? !machine.here,
        auto_chosen: auto.has(machine.name),
        relays: true,
        github: true,
        github_at: "2026-10-20T08:30:00Z",
        check_failure: null,
        checking: false,
        stage,
        due: due.get(machine.name) ?? null,
        offer: running === v(8) ? null : manifest(v(8)),
        actions: actions(online),
      });
    }
    for (const hub of now?.accounts.hubs ?? []) {
      for (const member of hub.members.filter((one) => one.kind === "phone")) {
        machines.push({
          machine: member.name,
          owner: member.logins[0] ?? "me",
          group: null,
          kind: "phone",
          online: false,
          reported: null,
          kernel: v(7),
          app: { version: v(7), target: "android-apk" },
          target: null,
          channel: "beta",
          auto: null,
          auto_chosen: false,
          relays: null,
          github: null,
          github_at: null,
          check_failure: null,
          checking: false,
          stage: { kind: "idle" },
          due: null,
          offer: manifest(v(8)),
          actions: { update: { allowed: false, reason: "phone" }, choose: { allowed: false, reason: "phone" }, settings: { allowed: false, reason: "phone" }, schedule: { allowed: false, reason: "phone" }, cancel: { allowed: false, reason: "phone" } },
        });
      }
    }
    const releases = [8, 7, 6, 5].map((n) => manifest(v(n)));
    const versions = releases.map((one) => ({ version: one.version, published: one.pub_date ?? null, summary: one.notes ?? null, manifest: one, problem: null }));
    return { hub: v(7), stable: null, beta: manifest(v(8)), releases, versions, policy: { ...policy }, machines, available: machines.filter((one) => one.offer && one.actions.schedule.allowed).length, repair_supported: true };
  };
  return async (hub, _account, call) => {
    // A node knows its hubs by name, not by id: asked by anything else, the
    // specimen refuses as the node does.
    if (!(world()?.accounts.hubs ?? []).some((one) => one.name === hub)) throw new Error(`this computer is not logged in to a hub named ${hub}`);
    if (call.do === "view") return { kind: "view", view: view() };
    if (call.do === "release") {
      const one = manifest(call.version);
      return { kind: "release", choice: { version: one.version, published: one.pub_date ?? null, summary: one.notes ?? null, manifest: one, problem: null } };
    }
    if (call.do === "policy") {
      if (call.minimum) policy.minimum = call.minimum;
      else delete policy.minimum;
      policy.managed_auto = call.managed_auto;
      return { kind: "done" };
    }
    if (call.do === "repair" || call.do === "cancel_repair") return { kind: "scheduled", results: call.machines.map((machine) => ({ machine, done: true, reason: null, why: null })) };
    for (const name of call.machines) {
      if (call.version) due.set(name, { version: call.version, by: "me", at: new Date().toISOString() });
      else due.delete(name);
    }
    return { kind: "scheduled", results: call.machines.map((machine) => ({ machine, done: true, reason: null, why: null })) };
  };
}
