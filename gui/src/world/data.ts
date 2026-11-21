// What the World keeps on this machine, and deleting it: the JSON form of
// `fastctx_types::data` (the settings page 本机数据, `fastctx data`). Each
// machine manages only its own; the request never goes to another machine.

import { invoke } from "@tauri-apps/api/core";
import type { AgentName, MachineName, Timestamp } from "./wire";

/** `kept`: what was last measured, at once (`unmeasured` while nothing was);
 *  an earlier node measures. */
export type DataCall =
  | { do: "list"; kept?: boolean }
  | { do: "delete"; names: string[] }
  | { do: "output_days"; days: number | null }
  | { do: "clear_cache" }
  | { do: "copies" }
  | { do: "set_copies"; hub: string; account: string; on: boolean }
  | { do: "app_opened"; hub: string; account: string; opened: number };

export type DataReply =
  | { data: "cache_cleared"; bytes: number }
  | ({ data: "listed" } & Space)
  | { data: "unmeasured" }
  | { data: "deleted"; deletions: Deletion[] }
  | { data: "output_days"; days: number; output_retention?: string }
  | { data: "cache_cleared"; bytes: number }
  | { data: "copies"; accounts: CopiesOf[] };

/** Whether an account logged in here takes in its copies of the
 *  conversations living on other machines (`fastctx data copies`). */
export interface CopiesOf {
  hub: string;
  account: string;
  /** When the account was opened, in milliseconds. */
  opened: number;
  on: boolean;
  /** What set it so: `install`, `app` or `owner`. */
  source?: string;
  at?: Timestamp;
  paused_scopes: number;
  /** About how many bytes those take there. */
  paused_bytes?: number;
}

/** What a machine's owner asks from afar of the copies it holds
 *  (`RemoteCall::Copies`). */
export type CopiesAsk =
  | { do: "report" }
  | { do: "set"; hub: string; account: string; on: boolean }
  | { do: "clear"; hub: string; account: string; apply?: boolean; only?: string[] };

/** What letting go an account's copies came to, or would. */
export interface Cleared {
  applied: boolean;
  scopes: number;
  bytes: number;
  kept: [string, string][];
  /** The copies let go, or that would be: what letting go after a count is
   *  limited to. */
  keys?: string[];
  /** Why letting go stopped short after some went. */
  failed?: string | null;
}

export type CopiesAnswer = { copies: "report"; accounts: CopiesOf[] } | ({ copies: "cleared" } & Cleared);

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
  output_retention?: string;
  claude_logins?: ClaudeKeeping[];
  /** Entries that could not be read and are not counted, and the first of
   *  them with why. */
  unread?: { entries: number; first?: string };
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
  /** `finishing` says why some of a deleted one is still being cleared, which its machine goes on with by itself. */
  outcome: { outcome: "deleted"; bytes: number; finishing?: string } | { outcome: "refused"; why: string } | { outcome: "failed"; why: string };
}

/** Asks this machine's node about the data it keeps. */
export function data(call: DataCall): Promise<DataReply> {
  return invoke<DataReply>("data", { call });
}
