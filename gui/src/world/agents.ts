// The agents each machine can start, as their official programs say: the
// JSON form of `fastctx_types::agents::WorldAgents`. What is not known is
// said so, never taken for a no.

import type { AgentName, MachineName, Timestamp } from "./wire";

export type Fact<T> = { kind: "known"; value: T } | { kind: "unavailable"; reason: string } | { kind: "failed"; reason: string };

export interface WorldAgents {
  machine: MachineName;
  machines: MachineAgents[];
}

export type MachineAgents =
  | { kind: "answered"; reply: AgentsReply }
  | { kind: "offline"; machine: MachineName; since: Timestamp | null }
  | { kind: "no_answer"; machine: MachineName; reason: string };

export interface AgentsReply {
  machine: MachineName;
  checked_at: Timestamp;
  elapsed_ms: number;
  agents: AgentFacts[];
}

export interface AgentFacts {
  agent: AgentName;
  program: Fact<AgentProgram | null>;
  version: Fact<string>;
  launch: Fact<LaunchCheck>;
  login: Fact<LoginStatus>;
  models: Fact<AgentModel[]>;
  integration: Fact<Integration>;
  timings: { step: string; elapsed_ms: number }[];
}

export interface AgentProgram {
  path: string;
  source: "desktop_app" | "path";
  exists: boolean;
}

export type LaunchCheck = { kind: "codex_app_server"; program: string; pid: number } | { kind: "claude_command" };

export type LoginStatus = { kind: "logged_in"; method: string | null } | { kind: "logged_out" };

export interface AgentModel {
  id: string;
  /** The value the World takes when opening a conversation with it. */
  model: string;
  display_name: string;
  description: string;
  is_default: boolean;
  default_reasoning_effort: string;
  reasoning_efforts: { effort: string; description: string }[];
}

export type Integration = "absent" | "out" | "in" | "old";
