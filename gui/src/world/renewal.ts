// The local `fastctx renew status --dir <root>` result is independent of
// the business IPC protocol, including when the node refuses that protocol.
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
