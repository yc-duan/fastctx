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

/** One agent in one of its homes on a machine: a home is one login, so a
 *  machine with two Codex accounts answers twice for Codex. */
export interface AgentFacts {
  agent: AgentName;
  /** The home these facts are of; an older World knew only the default. */
  home?: AgentHome;
  /** Who is logged in there, as the agent itself says. */
  account?: Fact<AgentAccount>;
  /** Its usage limits, as the agent last read them on its own machine. */
  limits?: Fact<AgentLimits>;
  program: Fact<AgentProgram | null>;
  version: Fact<string>;
  launch: Fact<LaunchCheck>;
  login: Fact<LoginStatus>;
  models: Fact<AgentModel[]>;
  integration: Fact<Integration>;
  /** Whether the World can start it there without approvals: Claude needs
   *  the user's consent once on that machine. An older World does not say. */
  unattended?: Fact<boolean>;
  timings: { step: string; elapsed_ms: number }[];
}

/** Where an agent keeps one login: Codex's `CODEX_HOME`, Claude Code's
 *  `CLAUDE_CONFIG_DIR`. The default one is what the agent uses when nothing
 *  names another; others the World found in the user's home, or the user
 *  added. */
export interface AgentHome {
  dir: string;
  found: "default" | "found" | "added";
}

/** What the user changes of the homes the World knows on a machine. */
export type HomeChange =
  | { change: "add"; agent: AgentName; dir: string }
  | { change: "hide"; agent: AgentName; dir: string }
  | { change: "show"; agent: AgentName; dir: string };

/** Who is logged in to a home, as far as the agent says: an API key or a
 *  long-lived token names no one. */
export interface AgentAccount {
  email?: string;
  organization?: string;
  /** The subscription, as the agent names it: "pro", "max", …. */
  plan?: string;
}

/** An account's usage limits and when they were read: no window is no
 *  reading, never 0% used. */
export interface AgentLimits {
  observed_at: Timestamp;
  windows: LimitWindow[];
  /** What the account goes on with once a window is used up. */
  credits?: { balance?: string; unlimited: boolean };
}

export interface LimitWindow {
  /** The agent's own name: Claude Code's "five_hour", "seven_day",
   *  "spend_limit"; Codex's limit's name. */
  name: string;
  /** How long the window is, when the agent says. */
  minutes?: number;
  /** Used, in tenths of a percent; it can pass 1000. */
  used_tenths: number;
  resets_at?: Timestamp;
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
  /** The context window it works in unless told otherwise, in tokens,
   *  where its host says; older nodes do not. */
  context_window?: number;
  /** The most its context window may be set to. */
  max_context_window?: number;
}

export type Integration = "absent" | "out" | "in" | "old";
