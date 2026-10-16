// New versions of the application (`world-upgrade.md`, 新版本自己往后怎样升级),
// as this computer's node or this phone's device keeps them, or another
// machine's node: the device's choices, what GitHub and the network said,
// and the package on its way or here. Installing a package that is here
// starts the new version.

import { invoke } from "@tauri-apps/api/core";

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
}

export interface UpdateNetwork {
  stable?: Manifest;
  beta?: Manifest;
  policy: Policy;
  machines?: Record<string, Running>;
  /** The version the hub runs; none from a hub before 1.0.1 beta 3. */
  hub?: string;
  due?: Record<string, Due>;
}

/** A release as GitHub lists it. */
export interface Release {
  version: string;
  published?: string;
  prerelease?: boolean;
  manifest?: string;
}

export interface Looked {
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
  /** The newest major version the hubs it is on run, which automatic
   *  updates do not pass. */
  hub_major?: number;
  looked: Looked;
  network?: UpdateNetwork;
  offer?: Manifest;
  fetching?: Fetching;
  failed?: { version: string; target: string; why: string };
  kept?: { version: string; target: string; path: string }[];
  app?: { version: string; target: string };
  kernel_target: string;
  /** The World program it changes to, while it does, or did not take. */
  renewing?: Renewing;
  /** A release asked from elsewhere that the application here takes. */
  take?: string;
  /** Whether an interface is open on that machine now. */
  open?: boolean;
}

/** What an interface asks a machine's node about updates. */
export type UpdateCall =
  | { do: "state" }
  | { do: "look" }
  | { do: "set"; settings: UpdateSettings }
  | { do: "kernel"; version?: string }
  | { do: "choose"; version?: string };

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
}

export const updates: UpdateSeam = {
  state: () => invoke<UpdateState>("update_state"),
  look: () => invoke<UpdateState>("update_look"),
  set: (settings) => invoke<UpdateState>("update_set", { settings }),
  fetch: (version) => invoke<UpdateState>("update_fetch", { version }),
  choose: (version) => invoke<UpdateState>("update_choose", { version }),
  install: (version) => invoke<boolean>("update_install", { version }),
  machine: (name) => invoke<UpdateState>("update_machine", { machineName: name }),
  remote: (name, call) => invoke<UpdateState>("machine_updates", { machineName: name, update: call }),
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
