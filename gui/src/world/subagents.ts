// An account's Claude subagents (`world-user-config.md`, 子代理), as
// `fastctx_types::profile::subagents` keeps them: the agents its Claude
// sessions may send work to, and a few words on how to use them. Changed one
// at a time over the account's profile version; an account that set none
// follows the defaults its Fastctx carries.

import { invoke } from "@tauri-apps/api/core";

/** One subagent, in Claude Code's own field names; any field the page does
 *  not edit is kept as it is. */
export interface SubagentSpec {
  name: string;
  description: string;
  prompt: string;
  tools?: string[];
  disallowedTools?: string[];
  model?: string;
  effort?: string | number;
  maxTurns?: number;
  initialPrompt?: string;
  background?: boolean;
  omitClaudeMd?: boolean;
  color?: string;
  [field: string]: unknown;
}

export interface Subagents {
  agents: SubagentSpec[];
  instructions: string;
}

export interface SubagentView {
  hub: string;
  account: string;
  /** Whether the hub keeps subagents: an older one does not. */
  supported: boolean;
  version: number;
  /** Whether the account set its own; the defaults otherwise. */
  configured: boolean;
  configuration: Subagents;
}

export type SubagentOperation =
  | { operation: "list" }
  /** A new one where `previous` is none; else that one, renamed if the
   *  name changed. */
  | { operation: "save"; version: number; previous: string | null; agent: SubagentSpec }
  | { operation: "delete"; version: number; name: string }
  | { operation: "instructions"; version: number; text: string }
  | { operation: "reset"; version: number };

/** What a name may be, as Claude Code registers it. */
export const SUBAGENT_NAME = /^[A-Za-z0-9_][A-Za-z0-9_-]{0,63}$/;

/** The efforts a subagent may be set to. */
export const SUBAGENT_EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;

export function profileSubagents(hub: string | null, account: string | null, operation: SubagentOperation): Promise<SubagentView> {
  return invoke<SubagentView>("profile_subagents", { hub, account, operation });
}
