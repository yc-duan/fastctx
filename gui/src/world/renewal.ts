// The local `fastctx renew status --dir <root>` result is independent of
// the business IPC protocol, including when the node refuses that protocol.

import { invoke } from "@tauri-apps/api/core";

/** A build of fastctx: its package version, protocol, content digest, and
 *  when its source was made, which orders builds of the same version. */
export interface NodeBuild { version: string; protocol: number; digest: string; built?: number | null }
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
  client?: NodeBuild | null;
  installed?: NodeBuild | null;
  commands?: PathCommand[];
  node: RunningNode | null;
  mismatch: VersionMismatch | null;
  pending: NodeBuild | null;
  phase: RenewalPhase | null;
  error: string | null;
}
export interface PathCommand {
  lookup: "windows" | "bash" | "unix" | "codex";
  path: string;
  executable: string | null;
  build: NodeBuild | null;
  is_world: boolean | null;
  legacy_unapply: boolean;
  error: string | null;
}

export function sameBuild(left: NodeBuild | null | undefined, right: NodeBuild | null | undefined): boolean {
  return Boolean(left && right && left.version === right.version && left.protocol === right.protocol && left.digest === right.digest);
}

export function buildLabel(build: NodeBuild | null | undefined): string {
  return build ? `${build.version} / IPC ${build.protocol} / ${build.digest}` : "?";
}

/** Where the node stands against the carried build: newer, older, or of an
 *  order not known. The same version is ordered by when its source was made;
 *  a build that records it is later than one that does not. */
export function buildOrder(carried: NodeBuild | null | undefined, node: NodeBuild): "older" | "newer" | "different" {
  if (carried && /^\d+\.\d+\.\d+$/.test(carried.version) && /^\d+\.\d+\.\d+$/.test(node.version)) {
    const left = carried.version.split(".").map(BigInt);
    const right = node.version.split(".").map(BigInt);
    for (let i = 0; i < 3; i++) if (left[i] !== right[i]) return left[i] < right[i] ? "newer" : "older";
    const [mine, theirs] = [carried.built ?? null, node.built ?? null];
    if (mine !== theirs) return mine === null ? "newer" : theirs === null || theirs < mine ? "older" : "newer";
  }
  return "different";
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
