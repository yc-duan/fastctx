// How the shell reaches the World (`fastctx-types/src/device.rs`): through
// this computer's own node, or as a device that carries its own key and
// asks the machines of the accounts it is logged in to, as a phone does.

import type { AccountName, HubName } from "./accounts";
import type { MachineName } from "./wire";
import type { LinkState } from "./world";

export type ConnectionMode = "local" | "device";

/** A device's standing; no key or token is ever in it. */
export interface DeviceState {
  /** Its name, the same on every hub; null before its first login. */
  name: MachineName | null;
  hubs: DeviceHub[];
}

/** One hub the device is logged in to. */
export interface DeviceHub {
  hub: HubName;
  link: LinkState;
  logins: AccountName[];
}

export interface ConnectionState {
  mode: ConnectionMode;
  device: DeviceState | null;
}
