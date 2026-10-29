// New versions of the application (`world-upgrade.md`, 新版本自己往后怎样升级),
// as this computer's node or this phone's device keeps them, or another
// machine's node: the device's choices, what GitHub and the network said,
// and the package on its way or here. Installing a package that is here
// starts the new version.

import { invoke } from "@tauri-apps/api/core";
import type { RepairReport } from "./repair";

export type Channel = "stable" | "beta";

export interface Package {
  url?: string;
  signature: string;
  size?: number;
}

export interface Manifest {
  version: string;
  notes?: string;
  pub_date?: string;
  platforms: Record<string, Package>;
  fastctx?: Record<string, Package>;
}

export interface UpdateSettings {
  /** Taking new releases by itself: what the owner chose, when `chosen`. */
  auto: boolean;
  /** Whether the owner chose `auto`; until then the device's kind decides. */
  chosen?: boolean;
  /** What 1.0.1 beta 1 and 2 kept of test releases; read no more. */
  beta?: boolean;
  relays: boolean;
}

export interface Running {
  kernel: string;
  app?: string;
  github?: boolean;
  /** What it ran when it was last connected, while it is away. */
  away?: boolean;
  /** It runs the network's hub. */
  hub_here?: boolean;
}

/** An update a machine away is to make once it is back. */
export interface Due {
  manifest?: Manifest;
  problem?: UpdateProblem;
  version: string;
  by: string;
  at: string;
}

export interface Policy {
  /** What devices of 1.0.1 beta 1 and 2 follow; told them as the beta line. */
  channel?: Channel;
  minimum?: string;
  /** Whether managed machines take new releases by themselves. */
  managed_auto?: boolean;
  /** How fast the hub takes a new release in, in bytes a second; none, its default. */
  intake?: number | null;
}

export interface UpdateNetwork {
  updates_view?: boolean;
  stable?: Manifest;
  beta?: Manifest;
  policy: Policy;
  machines?: Record<string, Running>;
  /** The version the hub runs; none from a hub before 1.0.1 beta 3. */
  hub?: string;
  due?: Record<string, Due>;
  /** Whether the hub arranges a repair for a machine that asks for one. */
  repair_supported?: boolean;
}

/** A release as GitHub lists it. */
export interface Release {
  version: string;
  published?: string;
  prerelease?: boolean;
  manifest?: string;
}

export interface Looked {
  checking?: boolean;
  failure?: CheckFailure;
  github_at?: string;
  next?: string;
  failures?: number;
  at?: string;
  github?: boolean;
  why?: string;
  stable?: Manifest;
  beta?: Manifest;
  /** Every release with a manifest, the newest first. */
  releases?: Release[];
  /** The manifests of the releases newer than what runs there. */
  newer?: Manifest[];
}

/** A change of a machine's World program, as it goes. */
export interface Renewing {
  version: string;
  phase: import("./renewal").RenewalPhase;
  error?: string;
}

export interface Fetching {
  version: string;
  target: string;
  via: "github" | "network";
  received: number;
  total?: number;
}

export interface UpdateState {
  progress?: boolean;
  preferences?: boolean;
  stage?: UpdateStage;
  /** The World program the node that answers runs; none from a node
   *  before 1.0.1 beta 3. */
  running?: string;
  settings: UpdateSettings;
  /** The line it follows: that of the version it runs. */
  channel: Channel;
  /** Whether it takes new releases by itself now. */
  auto?: boolean;
  /** The release its owner chose to update to, when not the newest. */
  chosen?: Manifest;
  /** The oldest major version among its hubs; an unknown hub uses the
   *  current kernel's major. Automatic updates do not pass this limit. */
  hub_major?: number;
  looked: Looked;
  network?: UpdateNetwork;
  offer?: Manifest;
  available?: Manifest;
  automatic?: string;
  above_hub?: boolean;
  fetching?: Fetching;
  failed?: { version: string; target: string; why: string; reason?: FailureReason };
  /** When the node tries the failed release again by itself. */
  retry_at?: string;
  kept?: { version: string; target: string; path: string }[];
  app?: { version: string; target: string };
  kernel_target: string;
  /** The World program it changes to, while it does, or did not take. */
  renewing?: Renewing;
  /** A release asked from elsewhere that the application here takes. */
  take?: string;
  /** Whether an interface is open on that machine now. */
  open?: boolean;
  /** Its World program can repair it (`repair` names the last repair). */
  repair_supported?: boolean;
  /** It can ask another machine to repair itself. */
  repair_routing?: boolean;
  repair?: RepairReport;
}

/** What an interface asks a machine's node about updates. */
export type UpdateCall =
  | { do: "preferences"; auto: boolean | null; relays: boolean | null }
  | { do: "progress"; stage: UpdateStage }
  | { do: "state" }
  | { do: "look" }
  | { do: "set"; settings: UpdateSettings }
  | { do: "kernel"; version?: string }
  | { do: "choose"; version?: string }
  | { do: "repair"; version: string | null; id: string | null };

export type ReachFailure = "dns" | "connect" | "tls" | "timeout" | "interrupted" | "proxy" | "other";
export type CheckFailure =
  | { kind: "unreachable"; reason: ReachFailure }
  | { kind: "rate_limited"; until: string | null }
  | { kind: "http"; status: number }
  | { kind: "unreadable" | "other" };
export type FailureReason =
  | { kind: "no_package" | "signature" | "disk" | "not_newer" | "other" }
  | { kind: "sources"; sources: { via: "github" | "network"; why: string }[] }
  | { kind: "installer"; code: number | null };
export interface UpdateProblem { reason: FailureReason; why: string }
export type UpdateStage =
  | { kind: "idle" | "unknown" }
  | { kind: "fetching"; version: string; via: "github" | "network" | null; received: number; total: number | null }
  | { kind: "ready" | "waiting_idle" | "waiting_app" | "waiting_sign_in" | "installing" | "administrator"; version: string }
  | { kind: "renewing"; version: string; phase: import("./renewal").RenewalPhase }
  | { kind: "repairing"; version: string; layer: string }
  | { kind: "failed"; version: string; problem: UpdateProblem };
export type UpdateKind = "app" | "headless" | "managed" | "phone";
export type ActionReason = "not_owner" | "visitor" | "phone" | "too_old" | "development" | "offline" | "unknown_version" | "missing_machine" | "no_package" | "not_newer" | "unavailable" | "other";
export interface UpdatePermission { allowed: boolean; reason: ActionReason | null }
export interface UpdateActions {
  update: UpdatePermission;
  choose: UpdatePermission;
  settings: UpdatePermission;
  schedule: UpdatePermission;
  cancel: UpdatePermission;
  /** Absent from a hub before repairs. */
  repair?: UpdatePermission;
}
export interface MachineUpdate {
  machine: string;
  owner: string;
  group: string | null;
  kind: UpdateKind;
  online: boolean;
  reported: string | null;
  kernel: string | null;
  app: { version: string; target: string } | null;
  target: string | null;
  channel: Channel;
  auto: boolean | null;
  auto_chosen: boolean;
  relays: boolean | null;
  github: boolean | null;
  github_at: string | null;
  check_failure: CheckFailure | null;
  checking: boolean;
  stage: UpdateStage;
  due: Due | null;
  offer: Manifest | null;
  actions: UpdateActions;
  repair_supported?: boolean;
  /** A repair the hub keeps for it until it has been made. */
  repair_due?: RepairDue;
  repair?: RepairReport;
}

/** A repair the hub keeps for a machine until it has been made. */
export interface RepairDue {
  id: string;
  version: string;
  by: string;
  at: string;
  manifest?: Manifest;
  problem?: UpdateProblem;
  attempts?: number;
  retry_at?: string;
  observed?: string;
}
export interface UpdateVersionChoice {
  version: string;
  published: string | null;
  summary: string | null;
  manifest: Manifest | null;
  problem: UpdateProblem | null;
}
export interface UpdatesView {
  hub: string;
  stable: Manifest | null;
  beta: Manifest | null;
  releases: Manifest[];
  versions: UpdateVersionChoice[];
  policy: Policy | null;
  machines: MachineUpdate[];
  available: number;
  /** The hub keeps repairs for machines. */
  repair_supported?: boolean;
}
export type UpdatesViewCall =
  | { do: "view" }
  | { do: "release"; version: string }
  | { do: "schedule"; machines: string[]; version: string | null }
  | { do: "repair"; machines: string[]; version: string | null }
  | { do: "cancel_repair"; machines: string[] }
  | { do: "policy"; minimum: string | null; managed_auto: boolean };
export type UpdatesViewReply =
  | { kind: "release"; choice: UpdateVersionChoice }
  | { kind: "view"; view: UpdatesView }
  | { kind: "scheduled"; results: { machine: string; done: boolean; reason: ActionReason | null; why: string | null }[] }
  | { kind: "done" };

export interface UpdateSeam {
  state(): Promise<UpdateState>;
  look(): Promise<UpdateState>;
  set(settings: UpdateSettings): Promise<UpdateState>;
  fetch(version: string): Promise<UpdateState>;
  /** What this application offers to update to from now on: `version`, or
   *  the newest on its line again. */
  choose(version: string | null): Promise<UpdateState>;
  /** Installs the version that is here; false when a phone first sent the
   *  user to allow installing apps. A computer starts again as the new
   *  version and does not come back from this. */
  install(version: string): Promise<boolean>;
  /** Has another machine update its World program itself, to the newest
   *  release on its line: what a machine before 1.0.1 beta 3 takes. */
  machine(name: string): Promise<UpdateState>;
  /** Asks another machine about its updates. */
  remote(name: string, call: UpdateCall): Promise<UpdateState>;
  /** Tells this device's node what only the interface knows of its update,
   *  as waiting for the work under way; `idle` clears it. */
  progress(stage: UpdateStage): Promise<UpdateState>;
  /** Repairs this device's Fastctx, layer by layer; answers as soon as the
   *  repair is under way. */
  repair(): Promise<UpdateState>;
}

export const updates: UpdateSeam = {
  state: () => invoke<UpdateState>("update_state"),
  progress: (stage) => invoke<UpdateState>("update_progress", { stage }),
  look: () => invoke<UpdateState>("update_look"),
  set: (settings) => invoke<UpdateState>("update_set", { settings }),
  fetch: (version) => invoke<UpdateState>("update_fetch", { version }),
  choose: (version) => invoke<UpdateState>("update_choose", { version }),
  install: (version) => invoke<boolean>("update_install", { version }),
  machine: (name) => invoke<UpdateState>("update_machine", { machineName: name }),
  remote: (name, call) => invoke<UpdateState>("machine_updates", { machineName: name, update: call }),
  repair: () => invoke<UpdateState>("update_repair", { version: null }),
};

/** Whether `version` is a test release. */
export function isTest(version: string): boolean {
  return version.includes("-");
}

/** The newest release `state` knows of on the line of `running`: a test
 *  release takes test releases and stable ones, a stable one stable ones
 *  alone; none when nothing newer is known. */
export function newestKnown(state: UpdateState | null | undefined, running: string): string | null {
  if (!state) return null;
  const test = isTest(running);
  const known = [state.offer, state.looked.stable, state.looked.beta, state.network?.stable, state.network?.beta, ...(state.looked.newer ?? [])];
  let best: string | null = null;
  for (const manifest of known) {
    if (!manifest || (!test && isTest(manifest.version))) continue;
    if (compareVersions(manifest.version, running) > 0 && (!best || compareVersions(manifest.version, best) > 0)) best = manifest.version;
  }
  return best;
}

/** A release's major version. */
export function majorOf(version: string): number {
  return Number(version.split(".")[0] ?? 0);
}

/** Semver precedence of two release versions: negative when `a` is older. */
export function compareVersions(a: string, b: string): number {
  const parse = (version: string) => {
    const [core, pre] = version.split("-", 2) as [string, string | undefined];
    return { core: core.split(".").map(Number), pre: pre ? pre.split(".") : [] };
  };
  const x = parse(a);
  const y = parse(b);
  for (let i = 0; i < 3; i++) {
    const d = (x.core[i] ?? 0) - (y.core[i] ?? 0);
    if (d) return d;
  }
  if (!x.pre.length || !y.pre.length) return y.pre.length - x.pre.length;
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i];
    const q = y.pre[i];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    const numeric = /^\d+$/.test(p) && /^\d+$/.test(q);
    const d = numeric ? Number(p) - Number(q) : p < q ? -1 : p > q ? 1 : 0;
    if (d) return d;
  }
  return 0;
}
