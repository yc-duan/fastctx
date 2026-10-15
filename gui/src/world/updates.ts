// New versions of the application (`world-upgrade.md`, 新版本自己往后怎样升级),
// as this computer's node or this phone's device keeps them: the device's
// choices, what GitHub and the network said, and the package on its way or
// here. Installing a package that is here starts the new version.

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
  auto: boolean;
  /** Test releases; absent follows the network. */
  beta?: boolean;
  relays: boolean;
}

export interface Running {
  kernel: string;
  app?: string;
  github?: boolean;
}

export interface UpdateNetwork {
  stable?: Manifest;
  beta?: Manifest;
  policy: { channel: Channel; minimum?: string };
  machines?: Record<string, Running>;
}

export interface Looked {
  at?: string;
  github?: boolean;
  why?: string;
  stable?: Manifest;
  beta?: Manifest;
}

export interface Fetching {
  version: string;
  target: string;
  via: "github" | "network";
  received: number;
  total?: number;
}

export interface UpdateState {
  settings: UpdateSettings;
  channel: Channel;
  looked: Looked;
  network?: UpdateNetwork;
  offer?: Manifest;
  fetching?: Fetching;
  failed?: { version: string; target: string; why: string };
  kept?: { version: string; target: string; path: string }[];
  app?: { version: string; target: string };
  kernel_target: string;
}

export interface UpdateSeam {
  state(): Promise<UpdateState>;
  look(): Promise<UpdateState>;
  set(settings: UpdateSettings): Promise<UpdateState>;
  fetch(version: string): Promise<UpdateState>;
  /** Installs the version that is here; false when a phone first sent the
   *  user to allow installing apps. A computer starts again as the new
   *  version and does not come back from this. */
  install(version: string): Promise<boolean>;
  /** Has another machine update its World program itself. */
  machine(name: string): Promise<UpdateState>;
}

export const updates: UpdateSeam = {
  state: () => invoke<UpdateState>("update_state"),
  look: () => invoke<UpdateState>("update_look"),
  set: (settings) => invoke<UpdateState>("update_set", { settings }),
  fetch: (version) => invoke<UpdateState>("update_fetch", { version }),
  install: (version) => invoke<boolean>("update_install", { version }),
  machine: (name) => invoke<UpdateState>("update_machine", { machineName: name }),
};

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
