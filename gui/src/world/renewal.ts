// The local `fastctx renew status --dir <root>` result is independent of
// the business IPC protocol, including when the node refuses that protocol.

import { invoke } from "@tauri-apps/api/core";

export interface NodeBuild { version: string; protocol: number; digest: string }
export interface VersionMismatch {
  node_version: string; node_protocol: number;
  client_version: string; client_protocol: number;
}
export interface RunningNode {
  build: NodeBuild;
  executable: string;
  process: { pid: number; token: string };
  yielding: boolean;
  bridge: boolean;
  supervised: boolean;
}
export type RenewalPhase = "staged" | "preparing" | "waiting_for_idle" | "awaiting_administrator"
  | "awaiting_legacy_exit" | "updating_guardian" | "returning_to_service" | "complete" | "failed";
export interface RenewalStatus {
  node: RunningNode | null;
  mismatch: VersionMismatch | null;
  pending: NodeBuild | null;
  phase: RenewalPhase | null;
  error: string | null;
}
export type MaintenanceRequest = { maintenance: 1; action: "status" | "yield" | "cancel" };
export type MaintenanceResponse = { kind: "status"; node: RunningNode } | { kind: "done" }
  | { kind: "refused"; reason: string };
export type VersionRefusal = { kind: "refused"; reason: string; mismatch?: VersionMismatch };

/** Which World this computer runs, and how putting a new one in place
 *  stands; answered even when the node refuses this interface. */
export function renewalState(): Promise<RenewalStatus> {
  return invoke<RenewalStatus>("renewal_state");
}

/** Asks the node to hand over to the version put in place. */
export function renewHere(): Promise<void> {
  return invoke("renew_here");
}

/** The administrator's step, once it is reached. */
export function renewGuardianHere(): Promise<void> {
  return invoke("renew_guardian_here");
}
