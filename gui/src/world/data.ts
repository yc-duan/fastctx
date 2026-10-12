// What the World keeps on this machine, and deleting it: the JSON form of
// `fastctx_types::data` (the settings page 本机数据, `fastctx data`). Each
// machine manages only its own; the request never goes to another machine.

import { invoke } from "@tauri-apps/api/core";
import type { AgentName, MachineName, Timestamp } from "./wire";

export type DataCall = { do: "list" } | { do: "delete"; names: string[] } | { do: "output_days"; days: number | null } | { do: "clear_cache" };

export type DataReply =
  | { data: "cache_cleared"; bytes: number }
  | ({ data: "listed" } & Space)
  | { data: "deleted"; deletions: Deletion[] }
  | { data: "output_days"; days: number }
  | { data: "cache_cleared"; bytes: number };

/** The days full command output may be kept for, to choose from. */
export const OUTPUT_DAYS = [1, 3, 7, 14, 30];

export interface Space {
  machine: MachineName;
  at: Timestamp;
  root: string;
  root_bytes: number;
  kinds: Kinds;
  conversations: ConversationSpace[];
  output_days: number;
  claude_logins?: ClaudeKeeping[];
}

export interface Kinds {
  conversations: number;
  host_records: number;
  command_output: number;
  cache: number;
  program: number;
  other: number;
}

export type Group = "listed" | "removed" | "elsewhere";

export interface ConversationSpace {
  name: string;
  group: Group;
  agent: AgentName;
  title?: string;
  town?: string;
  lives_on?: MachineName;
  last_activity?: Timestamp;
  bytes: number;
  parts: Parts;
  unopened?: boolean;
  /** Why it cannot be deleted now: what to do first. */
  held?: string;
}

export interface Parts {
  records: number;
  command_output: number;
  host: HostRecord;
}

/** The host's own record of it. For Claude Code, `days` is how long its
 *  login keeps a session nobody touched, as that login's settings say. */
export type HostRecord = { kept: "none" } | { kept: "kept"; bytes: number; days?: number } | { kept: "gone_by_claude"; days?: number; bytes?: number } | { kept: "missing"; bytes?: number };

export interface ClaudeKeeping {
  home?: string;
  days?: number;
  set: boolean;
  unreadable?: string;
}

export interface Deletion {
  name: string;
  outcome: { outcome: "deleted"; bytes: number } | { outcome: "refused"; why: string } | { outcome: "failed"; why: string };
}

/** Asks this machine's node about the data it keeps. */
export function data(call: DataCall): Promise<DataReply> {
  return invoke<DataReply>("data", { call });
}
