// Moving the hub to another machine, as the node answers it (`fastctx_types::hub_move`).

export type HubMoveRequest =
  | { request: "targets" }
  | { request: "start"; to: string; addresses: string[] }
  | { request: "status" }
  | { request: "retry" }
  | { request: "cancel" }
  | { request: "stop_forwarding"; id: string }
  | { request: "delete_old"; id: string };

export type HubMoveReply =
  | { kind: "targets"; machines: MoveTarget[]; port: number | null }
  | { kind: "moves"; moves: HubMove[] };

export interface MoveTarget {
  name: string;
  online: boolean;
  os?: string;
  form?: "desktop" | "laptop" | "server";
  /** It goes to sleep on its own, as a laptop does. */
  sleeps: boolean;
  interfaces?: InterfaceAddress[];
  /** The address the hub sees its connection come from. */
  seen?: string;
  /** The hub runs on it now. */
  hosts_hub: boolean;
  /** Why it cannot take the hub now; absent when it can. */
  unavailable?: string;
}

export interface InterfaceAddress {
  interface: string;
  address: string;
  physical: boolean;
}

/** In order; each is the step under way, the ones before it done. */
export type MoveStage =
  | "preparing"
  | "service"
  | "reaching"
  | "copying"
  | "switching"
  | "following"
  | "done"
  | "cancelled";

export interface HubMove {
  id: string;
  to: string;
  addresses: string[];
  from: string[];
  by: string;
  started: string;
  stage: MoveStage;
  /** Why the move does not get past `stage`, as last tried. */
  error?: string;
  /** What waits for someone at the new place. */
  waiting?: { kind: "service"; dir: string; os: string };
  switched?: string;
  following?: Following;
}

export interface Following {
  /** Machines that have not followed to the new place yet. */
  machines: string[];
  devices: string[];
  old: OldPlace;
  old_connected: boolean;
}

export type OldPlace =
  | { state: "forwarding" }
  | { state: "stopped"; at: string; early: boolean }
  | { state: "deleting"; at: string }
  | { state: "deleted"; at: string };
