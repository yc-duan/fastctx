// How the shell reaches the World (`fastctx-types/src/device.rs`): through
// this computer's own node, or as a device that carries its own key and
// asks the World's machines, as a phone does.

import type { MachineName } from "./wire";
import type { LinkState } from "./world";

export type ConnectionMode = "local" | "device";

/** A device's standing; no key or invitation is ever in it. */
export interface DeviceState {
  name: MachineName | null;
  world: string | null;
  link: LinkState;
}

export interface ConnectionState {
  mode: ConnectionMode;
  device: DeviceState | null;
}
