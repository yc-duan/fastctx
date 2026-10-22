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
 *  order not known. Versions are ordered as releases are, a test release
 *  (`1.0.1-beta.9`) before the release it leads to; the same version is
 *  ordered by when its source was made, a build that records it later than
 *  one that does not. */
export function buildOrder(carried: NodeBuild | null | undefined, node: NodeBuild): "older" | "newer" | "different" {
  const order = carried ? versionOrder(carried.version, node.version) : null;
  if (order !== null && order !== 0) return order < 0 ? "newer" : "older";
  if (order === 0) {
    const [mine, theirs] = [carried!.built ?? null, node.built ?? null];
    if (mine !== theirs) return mine === null ? "newer" : theirs === null || theirs < mine ? "older" : "newer";
  }
  return "different";
}

/** How release `left` stands to `right`, by semantic versioning: below
 *  zero, before it; null where either is not a release's version. */
function versionOrder(left: string, right: string): number | null {
  const shape = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/;
  const [a, b] = [shape.exec(left), shape.exec(right)];
  if (!a || !b) return null;
  for (let i = 1; i <= 3; i++) {
    const [x, y] = [BigInt(a[i]!), BigInt(b[i]!)];
    if (x !== y) return x < y ? -1 : 1;
  }
  const [x, y] = [a[4], b[4]];
  if (x === y) return 0;
  // A release comes after every test release of it.
  if (x === undefined) return 1;
  if (y === undefined) return -1;
  const [xs, ys] = [x.split("."), y.split(".")];
  for (let i = 0; i < Math.max(xs.length, ys.length); i++) {
    const [p, q] = [xs[i], ys[i]];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    if (p === q) continue;
    const [pn, qn] = [/^\d+$/.test(p), /^\d+$/.test(q)];
    if (pn && qn) return BigInt(p) < BigInt(q) ? -1 : 1;
    if (pn !== qn) return pn ? -1 : 1;
    return p < q ? -1 : 1;
  }
  return 0;
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

