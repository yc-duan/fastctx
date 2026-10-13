// What an agent's login was used for (`world-agent-logins.md`, 用量), as its
// machine's node answers it: the accounts its conversations worked for, the
// tokens and Claude Code's own cost by the hour over a year, what the
// current week went to, and the login's windows as read through that week.
// Amounts of dollars are in millionths of a dollar.

import type { HubAccount } from "./accounts";
import type { LimitWindow } from "./agents";
import type { Timestamp } from "./wire";

export interface UsageSum {
  tokens: number;
  /** Absent where the agent keeps no cost ledger (Codex). */
  usd?: number;
}

export interface UsagePerson {
  /** Null is the user of the machine itself: its own sessions work for
   *  whoever is logged in there. */
  account: HubAccount | null;
  /** What it used in the week, as its weekly allowance counts it. */
  week: UsageSum;
}

export interface UsageHour extends UsageSum {
  /** The hour's start. */
  at: Timestamp;
  /** Its place in the report's people. */
  person: number;
}

export interface Tokens {
  input?: number;
  cache_read?: number;
  cache_write?: number;
  output?: number;
  reasoning?: number;
}

export interface KindsUsd {
  input: number;
  cache_read: number;
  cache_write: number;
  output: number;
}

export interface ModelUse extends UsageSum {
  model: string;
}

export interface UsageWeek {
  start: Timestamp | null;
  end: Timestamp | null;
  /** Whether the week is the login's own seven-day window; otherwise the
   *  seven days up to now. */
  window: boolean;
  models: ModelUse[];
  tokens: Tokens;
  kinds_usd?: KindsUsd;
}

export interface UsageReading {
  at: Timestamp;
  windows: LimitWindow[];
}

export interface UsageReport {
  people: UsagePerson[];
  hours: UsageHour[];
  week: UsageWeek;
  readings: UsageReading[];
}
